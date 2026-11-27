//! A text-only Chat Completions model. HTTP and SSE parsing run outside the UI thread.
//! A completed `stop` choice or `[DONE]` ends the reply; an unfinished EOF is a failure.
//! Titles are derived locally, so naming a conversation never makes another HTTP request.

use crate::engine::{Cancel, Model, Purpose, Request};
use crate::model::{Author, Event};
use octara::prelude::{ScopeId, Sender, WriteCx};
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue};
use reqwest::{Client, StatusCode, Url};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::runtime::{Builder, Runtime};

const MAX_EVENT_BYTES: usize = 1024 * 1024;
const INVALID_EVENT: &str = "The model server sent an invalid streaming event.";
const INCOMPLETE_STREAM: &str = "The model stream ended before its completion marker. The text received so far was kept.";
const EMPTY_RESPONSE: &str = "The model finished without returning any text.";

/// Connection settings, kept out of diagnostics and debug output.
pub struct NetworkConfig {
    /// The complete HTTP(S) Chat Completions URL, including its path.
    pub endpoint: String,
    pub model: String,
    pub api_key: Option<String>,
    /// Connect and per-read idle timeout, not a total duration limit for the reply.
    pub timeout: Duration,
}

/// One reusable client and bounded worker runtime for this model's requests.
pub struct NetworkModel {
    endpoint: Url,
    model: String,
    client: Client,
    runtime: Option<Runtime>,
}

impl NetworkModel {
    pub fn new(config: NetworkConfig) -> Result<Self, String> {
        let endpoint = Url::parse(&config.endpoint).map_err(|_| "The model endpoint must be a complete HTTP or HTTPS URL.")?;
        if !matches!(endpoint.scheme(), "http" | "https")
            || endpoint.host_str().is_none()
            || !endpoint.username().is_empty()
            || endpoint.password().is_some()
            || endpoint.fragment().is_some()
        {
            return Err("Use an HTTP or HTTPS model endpoint without embedded credentials or a fragment.".into());
        }
        let model = config.model.trim().to_owned();
        if model.is_empty() {
            return Err("A model name is required.".into());
        }
        if config.timeout.is_zero() {
            return Err("The model connection timeout must be greater than zero.".into());
        }
        let mut headers = HeaderMap::new();
        headers.insert(ACCEPT, HeaderValue::from_static("text/event-stream"));
        if let Some(key) = config.api_key {
            if key.trim().is_empty() {
                return Err("The model API key cannot be blank.".into());
            }
            let mut authorization =
                HeaderValue::from_str(&format!("Bearer {key}")).map_err(|_| "The model API key is not a valid header value.")?;
            authorization.set_sensitive(true);
            headers.insert(AUTHORIZATION, authorization);
        }
        let client = Client::builder()
            .default_headers(headers)
            .connect_timeout(config.timeout)
            .read_timeout(config.timeout)
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .pool_max_idle_per_host(2)
            .build()
            .map_err(|_| "The HTTP client could not be initialized.")?;
        let runtime = Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("octara-chat-http")
            .enable_all()
            .build()
            .map_err(|_| "The model's network workers could not be started.")?;
        Ok(Self { endpoint, model, client, runtime: Some(runtime) })
    }
}

impl Drop for NetworkModel {
    fn drop(&mut self) {
        if let Some(runtime) = self.runtime.take() {
            // Cancels async I/O without joining worker threads on the UI thread.
            runtime.shutdown_background();
        }
    }
}

impl Model for NetworkModel {
    fn name(&self) -> &str {
        match self.model.as_str() {
            "deepseek/deepseek-v4.1-flash" => "DeepSeek V4.1 Flash",
            _ => &self.model,
        }
    }

    fn respond(&self, _: &mut WriteCx<'_>, _: ScopeId, request: Request, send: Sender<Event>) -> Cancel {
        if request.purpose == Purpose::Title {
            let question = request.history.iter().rev().find(|(author, _)| *author == Author::User).map_or("", |(_, text)| text.as_str());
            let _ = send.send(Event::Text(conversation_title(question)));
            let _ = send.send(Event::Done);
            return Box::new(|| {});
        }

        let (client, endpoint, model) = (self.client.clone(), self.endpoint.clone(), self.model.clone());
        let cancelled = Arc::new(AtomicBool::new(false));
        let stopped = cancelled.clone();
        let task = self.runtime.as_ref().expect("the live model owns its runtime").spawn(async move {
            let result = reply(client, endpoint, model, request, &send, &stopped).await;
            if let Err(error) = result
                && !stopped.load(Ordering::Acquire)
            {
                let _ = send.send(Event::Failed(error));
            }
        });
        Box::new(move || {
            cancelled.store(true, Ordering::Release);
            // Dropping the request future closes pending headers/body reads. Cancellation
            // itself sends no failure; Chat also guards events from retired requests.
            task.abort();
        })
    }
}

fn conversation_title(question: &str) -> String {
    let lead = question.trim().split(['\n', '：', ':', '。', '？', '?', '！', '!']).next().unwrap_or_default();
    let words = lead.split_whitespace().take(5).collect::<Vec<_>>().join(" ");
    let title = words.chars().take(32).collect::<String>();
    let title = title.trim_end_matches('.');
    if title.is_empty() { "New chat".into() } else { title.into() }
}

async fn reply(
    client: Client,
    endpoint: Url,
    model: String,
    request: Request,
    send: &Sender<Event>,
    cancelled: &AtomicBool,
) -> Result<(), String> {
    let generation = request.generation.validate()?;
    let messages: Vec<_> = request
        .history
        .into_iter()
        .map(|(author, text)| json!({"role":match author { Author::User => "user", Author::Assistant => "assistant" }, "content":text}))
        .collect();
    let mut body = json!({"model":model,"messages":messages,"stream":true,"n":1});
    if let Some(value) = generation.temperature {
        body["temperature"] = json!(value);
    }
    if let Some(value) = generation.max_completion_tokens {
        body["max_completion_tokens"] = json!(value);
    }
    let mut response = client.post(endpoint).json(&body).send().await.map_err(network_error)?;
    if !response.status().is_success() {
        return Err(status_error(response.status()));
    }
    let event_stream = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("text/event-stream"));
    if !event_stream {
        return Err("The model server did not return a text/event-stream response.".into());
    }
    let mut decoder = Decoder::default();
    while let Some(chunk) = response.chunk().await.map_err(network_error)? {
        if cancelled.load(Ordering::Acquire) {
            return Ok(());
        }
        if decoder.push(&chunk, |event| !cancelled.load(Ordering::Acquire) && send.send(event).is_ok())? {
            return Ok(());
        }
    }
    decoder.eof().map_err(str::to_owned)
}

// Never format reqwest errors: their Display can contain a URL or sensitive query values.
fn network_error(error: reqwest::Error) -> String {
    if error.is_timeout() {
        "The model connection timed out while waiting for data.".into()
    } else if error.is_connect() {
        "Could not connect to the model server. Check its address and your connection.".into()
    } else {
        "The model connection was interrupted. The text received so far was kept.".into()
    }
}

fn status_error(status: StatusCode) -> String {
    match status.as_u16() {
        401 => "The model server rejected the API key (HTTP 401). Check the configured key.".into(),
        403 => "The model server denied access (HTTP 403). Check the key's permissions and model access.".into(),
        404 => "The model endpoint or model was not found (HTTP 404). Check the endpoint path and model name.".into(),
        429 => "The model server's rate or quota limit was reached (HTTP 429). Try again later or check your quota.".into(),
        500..=599 => format!("The model server is temporarily unavailable (HTTP {}). Try again later.", status.as_u16()),
        300..=399 => "The model endpoint returned a redirect. Configure the final Chat Completions URL directly.".into(),
        _ => format!("The model server rejected the request (HTTP {}). Check the model configuration.", status.as_u16()),
    }
}

#[derive(Deserialize)]
struct CompletionChunk {
    #[serde(default)]
    choices: Vec<Choice>,
    usage: Option<Value>,
    error: Option<Value>,
}

#[derive(Deserialize)]
struct Choice {
    index: u32,
    delta: Delta,
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct Delta {
    role: Option<String>,
    content: Option<String>,
    refusal: Option<String>,
    reasoning: Option<String>,
    reasoning_content: Option<String>,
    tool_calls: Option<Vec<Value>>,
    function_call: Option<Value>,
}

#[derive(Default)]
struct Decoder {
    sse: Sse,
    received_text: bool,
    done: bool,
}

impl Decoder {
    /// `true` means complete, cancelled, or the UI receiver has gone away.
    fn push(&mut self, bytes: &[u8], mut emit: impl FnMut(Event) -> bool) -> Result<bool, &'static str> {
        if self.done {
            return Ok(true);
        }
        let Self { sse, received_text, done } = self;
        *done = sse.push(bytes, |data| completion(data, received_text, &mut emit))?;
        Ok(*done)
    }

    fn eof(&self) -> Result<(), &'static str> {
        if self.done { Ok(()) } else { Err(INCOMPLETE_STREAM) }
    }
}

fn completion(data: &str, received_text: &mut bool, emit: &mut impl FnMut(Event) -> bool) -> Result<bool, &'static str> {
    if data == "[DONE]" {
        if !*received_text {
            return Err(EMPTY_RESPONSE);
        }
        emit(Event::Done);
        return Ok(true);
    }
    let chunk: CompletionChunk = serde_json::from_str(data).map_err(|_| INVALID_EVENT)?;
    if chunk.error.is_some() {
        return Err("The model server reported a streaming error. The text received so far was kept.");
    }
    if chunk.choices.is_empty() {
        return if chunk.usage.is_some_and(|usage| usage.is_object()) { Ok(false) } else { Err(INVALID_EVENT) };
    }
    let mut choices = chunk.choices.into_iter().filter(|choice| choice.index == 0);
    let choice = choices.next().ok_or(INVALID_EVENT)?;
    if choices.next().is_some() {
        return Err(INVALID_EVENT);
    }
    if choice.delta.tool_calls.as_ref().is_some_and(|calls| !calls.is_empty())
        || choice.delta.function_call.is_some()
        || matches!(choice.finish_reason.as_deref(), Some("tool_calls" | "function_call"))
    {
        return Err("This connection supports text replies only. The model requested an unsupported tool call.");
    }
    if choice.delta.role.as_deref().is_some_and(|role| role != "assistant") {
        return Err(INVALID_EVENT);
    }
    if let Some(reasoning) = choice.delta.reasoning.or(choice.delta.reasoning_content).filter(|text| !text.is_empty())
        && !emit(Event::Reasoning(reasoning))
    {
        return Ok(true);
    }
    for text in [choice.delta.content, choice.delta.refusal].into_iter().flatten().filter(|text| !text.is_empty()) {
        *received_text = true;
        if !emit(Event::Text(text)) {
            return Ok(true);
        }
    }
    match choice.finish_reason.as_deref() {
        None => Ok(false),
        Some("stop") => {
            if !*received_text {
                return Err(EMPTY_RESPONSE);
            }
            emit(Event::Done);
            Ok(true)
        }
        Some("length") => Err("The model reached its output limit. The text received so far was kept."),
        Some("content_filter") => Err("The model's content filter stopped the reply. The text received so far was kept."),
        Some(_) => Err("The model returned an unsupported completion reason. The text received so far was kept."),
    }
}

/// WHATWG line framing: CRLF, CR, and LF; optional first-line BOM; data lines joined by LF.
/// Bytes stay undecoded until a complete line exists, so UTF-8 may cross HTTP chunks.
#[derive(Default)]
struct Sse {
    line: Vec<u8>,
    data: Vec<u8>,
    has_data: bool,
    skip_lf: bool,
    started: bool,
    event_bytes: usize,
}

impl Sse {
    fn push(&mut self, bytes: &[u8], mut handle: impl FnMut(&str) -> Result<bool, &'static str>) -> Result<bool, &'static str> {
        for &byte in bytes {
            if self.skip_lf {
                self.skip_lf = false;
                if byte == b'\n' {
                    continue;
                }
            }
            self.event_bytes += 1;
            if self.event_bytes > MAX_EVENT_BYTES {
                return Err("A model streaming event exceeded the 1 MiB buffer limit.");
            }
            if matches!(byte, b'\r' | b'\n') {
                self.skip_lf = byte == b'\r';
                if self.end_line(&mut handle)? {
                    return Ok(true);
                }
            } else {
                self.line.push(byte);
            }
        }
        Ok(false)
    }

    fn end_line(&mut self, handle: &mut impl FnMut(&str) -> Result<bool, &'static str>) -> Result<bool, &'static str> {
        let line = std::mem::take(&mut self.line);
        let line = std::str::from_utf8(&line).map_err(|_| "The model stream contained invalid UTF-8.")?;
        let line = if self.started {
            line
        } else {
            self.started = true;
            line.strip_prefix('\u{feff}').unwrap_or(line)
        };
        if line.is_empty() {
            self.event_bytes = 0;
            if !self.has_data {
                return Ok(false);
            }
            self.has_data = false;
            self.data.pop(); // The last data-line newline is not part of the dispatched event.
            let data = std::mem::take(&mut self.data);
            return handle(std::str::from_utf8(&data).map_err(|_| "The model stream contained invalid UTF-8.")?);
        }
        if line.starts_with(':') {
            return Ok(false);
        }
        let (field, value) = line.split_once(':').unwrap_or((line, ""));
        if field == "data" {
            self.has_data = true;
            self.data.extend_from_slice(value.strip_prefix(' ').unwrap_or(value).as_bytes());
            self.data.push(b'\n');
        }
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(parts: impl IntoIterator<Item = Vec<u8>>) -> (Vec<Event>, Result<(), &'static str>) {
        let mut decoder = Decoder::default();
        let mut events = Vec::new();
        for part in parts {
            match decoder.push(&part, |event| {
                events.push(event);
                true
            }) {
                Ok(true) => return (events, Ok(())),
                Ok(false) => {}
                Err(error) => return (events, Err(error)),
            }
        }
        (events, decoder.eof())
    }

    fn event(delta: Value, finish: Option<&str>) -> String {
        format!("data: {}\n\n", json!({"choices":[{"index":0,"delta":delta,"finish_reason":finish}]}))
    }

    #[test]
    fn utf8_crlf_cr_bom_comments_and_multiple_data_lines_cross_chunks() {
        let source = format!(
            "\u{feff}: comment\r\n\r\ndata: {{\"choices\": [\r\ndata: {{\"index\":0,\"delta\":{{\"content\":\"中文🙂\"}},\"finish_reason\":null}}]}}\r\n\r\n{}data: [DONE]\r\r",
            "data: {\"choices\":[],\"usage\":{\"total_tokens\":3}}\n\n"
        );
        for size in [1, 2, 3, 7, source.len()] {
            let (events, result) = parse(source.as_bytes().chunks(size).map(<[u8]>::to_vec));
            assert_eq!(result, Ok(()), "chunk size {size}");
            assert_eq!(events, [Event::Text("中文🙂".into()), Event::Done]);
        }
    }

    #[test]
    fn stop_finishes_and_refusal_is_text() {
        let source = event(json!({"role":"assistant","content":null,"refusal":"I cannot help with that."}), Some("stop"));
        assert_eq!(parse([source.into_bytes()]), (vec![Event::Text("I cannot help with that.".into()), Event::Done], Ok(())));
        let source = event(json!({"content":"text without tools","tool_calls":[]}), Some("stop"));
        assert_eq!(parse([source.into_bytes()]), (vec![Event::Text("text without tools".into()), Event::Done], Ok(())));
    }

    #[test]
    fn openrouter_and_deepseek_reasoning_remain_separate_from_the_answer() {
        let source = format!(
            "{}{}{}",
            event(json!({"reasoning":"Let me think.", "reasoning_content":"duplicate fallback"}), None),
            event(json!({"reasoning_content":"One more step."}), None),
            event(json!({"content":"The answer."}), Some("stop"))
        );
        assert_eq!(
            parse(source.as_bytes().chunks(3).map(<[u8]>::to_vec)),
            (
                vec![
                    Event::Reasoning("Let me think.".into()),
                    Event::Reasoning("One more step.".into()),
                    Event::Text("The answer.".into()),
                    Event::Done
                ],
                Ok(())
            )
        );
        let source = format!("{}data: [DONE]\n\n", event(json!({"reasoning":"Still thinking"}), None));
        assert_eq!(parse([source.into_bytes()]).1, Err(EMPTY_RESPONSE), "reasoning alone is not a finished answer");
    }

    #[test]
    fn local_titles_keep_the_topic_without_chinese_prompt_instructions() {
        assert_eq!(conversation_title("请用中文解释 Rust 的所有权：列出三个要点。"), "请用中文解释 Rust 的所有权");
        assert_eq!(conversation_title("Explain ownership\nInclude a code example"), "Explain ownership");
        assert_eq!(conversation_title("  "), "New chat");
    }

    #[test]
    fn unfinished_eof_keeps_text_and_does_not_dispatch_a_partial_event() {
        let source = format!("{}data: [DONE]\n", event(json!({"content":"kept"}), None));
        assert_eq!(parse([source.into_bytes()]), (vec![Event::Text("kept".into())], Err(INCOMPLETE_STREAM)));
    }

    #[test]
    fn empty_invalid_and_server_error_events_cannot_succeed() {
        for source in [
            "data: [DONE]\n\n".to_owned(),
            "data:\n\n".to_owned(),
            "data: {}\n\n".to_owned(),
            "data: {\"error\":{\"message\":\"secret-server-body\"}}\n\n".to_owned(),
            format!("{}data: [DONE]\n\n", event(json!({"role":"assistant","content":""}), None)),
            event(json!({}), Some("stop")),
        ] {
            let (events, result) = parse([source.into_bytes()]);
            assert!(events.is_empty());
            let error = result.unwrap_err();
            assert!(!error.contains("secret-server-body"));
        }
    }

    #[test]
    fn truncated_filtered_and_tool_replies_fail_without_losing_prior_text() {
        for ending in [
            event(json!({}), Some("length")),
            event(json!({}), Some("content_filter")),
            event(json!({"tool_calls":[{"function":{"name":"unsupported"}}]}), None),
            event(json!({"function_call":{"name":"unsupported"}}), None),
            event(json!({}), Some("tool_calls")),
        ] {
            let source = format!("{}{ending}", event(json!({"content":"kept"}), None));
            let (events, result) = parse([source.into_bytes()]);
            assert_eq!(events, [Event::Text("kept".into())]);
            assert!(result.is_err());
        }
    }

    #[test]
    fn an_unterminated_event_is_bounded() {
        let source = format!("data: {}", "x".repeat(MAX_EVENT_BYTES));
        let (events, result) = parse([source.into_bytes()]);
        assert!(events.is_empty());
        assert!(result.unwrap_err().contains("buffer limit"));
    }
}
