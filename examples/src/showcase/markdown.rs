//! A local chat sample with a rich Markdown reply and caret-anchored completions.

use octara::prelude::*;
use octara_icons as icons;
use octara_kit::{ButtonVariant, DialogOptions, Size as KitSize, button, collapsible, dialog, parts::editor_style};
use octara_markdown::{
    BlockSlot, BlockSlotRenderer, Completion, LinkPillContent, LinkVariantRule, LinkVariantRules, MarkdownComposer, MarkdownConfig,
    MarkdownRenderer, StreamingEffects, StreamingOptions, TriggerQuery, markdown, markdown_stream,
};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

const QUESTION: &str = "Why does the Moon turn red during a lunar eclipse?";
const GUIDE: &str = "https://science.nasa.gov/eclipses/";
const ANSWER: &str = r#":::think
Follow the path of sunlight through Earth’s atmosphere. Then compare the Moon’s brightness at three points during the eclipse.
:::

### A lunar eclipse in three steps

Earth blocks direct sunlight, but some light bends through its atmosphere. Shorter blue wavelengths scatter more strongly, so the light that reaches the Moon has a warm tint. The [eclipse guide](https://science.nasa.gov/eclipses/) explains where to look.

![Red Moon against a dark sky](eclipse:moon "Moon during totality")

The red disk ![eclipse Moon](eclipse:disk) is easiest to see after your eyes adjust. An eclipse magnitude describes how deeply the Moon enters the shadow. To compare images, use the relative brightness $f=F/F_0$, where $F_0$ is the brightness before eclipse.

$$
m - m_0 = -2.5\log_{10}\left(\frac{F}{F_0}\right)
$$

```brightness-chart
Outside shadow,100
Partial eclipse,58
Totality,16
```

```rust eclipse-magnitude
fn magnitude_change(relative_brightness: f32) -> f32 {
    -2.5 * relative_brightness.log10()
}
```

The chart uses illustrative brightness values. Real totality varies with dust and clouds in Earth’s atmosphere.
"#;

const REVEAL: StreamingEffects =
    StreamingEffects::Gradient { trail_bytes: 24, tint: Some(ColorValue::Token(ColorToken::Accent)), glide_bytes_per_sec: 150.0 };

#[derive(Clone, Copy)]
struct Reply {
    source: TextBuffer,
    running: Signal<bool>,
    resolved: Signal<bool>,
}

impl Reply {
    fn replay(self, w: &mut WriteCx<'_>) {
        if w.get(self.running) {
            return;
        }
        w.set(self.running, true);
        w.set(self.resolved, false);
        let len = w.with_text(self.source, str::len);
        w.splice_text(self.source, TextBufferEdit::Splice { range: 0..len, insert: String::new() });
        w.spawn("stream eclipse reply", move |task| async move {
            const CHARS_PER_TICK: usize = 10;
            let resolve_at =
                ANSWER.split_once(GUIDE).map(|(prefix, _)| (prefix.chars().count() + GUIDE.len() + 1).div_ceil(CHARS_PER_TICK) + 10);
            let chars: Vec<_> = ANSWER.chars().collect();
            for (index, piece) in chars.chunks(CHARS_PER_TICK).enumerate() {
                task.sleep(Duration::from_millis(25)).await;
                task.write(|w| {
                    w.append_text(self.source, &piece.iter().collect::<String>());
                    // Simulate a source lookup completing after the link is already visible.
                    if resolve_at == Some(index) {
                        w.set(self.resolved, true);
                    }
                });
            }
            task.write(|w| w.set(self.running, false));
        });
    }
}

#[ui]
pub fn view(b: &mut BuildCx<'_>) -> El {
    let reply = Reply { source: b.text_buffer(ANSWER), running: b.signal(false), resolved: b.signal(true) };
    let finished = b.memo(move |r| !r.get(reply.running));
    let selected = b.signal(None::<(Arc<str>, Arc<str>)>);
    provide_renderer(b, reply.resolved, selected);
    let lightbox = image_preview(b, selected);
    let question = b.signal(QUESTION.to_owned());
    let composer = MarkdownComposer::new(b, "");
    let submit = move |w: &mut WriteCx<'_>| {
        if w.get(reply.running) {
            return;
        }
        let draft = w.get(composer.markdown());
        if !draft.trim().is_empty() {
            w.set(question, draft);
            composer.set_markdown(w, "");
        }
        reply.replay(w);
    };
    let input = composer.editor_with_completion(
        b,
        editor_style(textarea())
            .test_id("eclipse-composer")
            .w(rel(1.0))
            .label("Message to the demo model")
            .placeholder("Try @Luna or /replay…")
            .rows(1, 4)
            .submit_on_enter()
            .on_submit(submit),
        |_, query| suggestions(query),
        move |w, completion| match completion.action_key() {
            Some("replay") => reply.replay(w),
            Some("question") => composer.set_markdown(w, QUESTION),
            _ => {}
        },
    );
    let viewport = b.viewport();
    let transcript = col()
        .test_id("answer")
        .label("Eclipse conversation")
        .w(rel(1.0))
        .style_with(move |r, p| p.max_h((r.get(viewport).unobscured_size().height - 164.0).clamp(180.0, 500.0)))
        .overflow_y(Overflow::Scroll)
        .follow_tail()
        .gap(sp(4.0))
        .p(sp(4.0))
        .rounded(RadiusToken::Lg)
        .border(1.0)
        .border_color(ColorToken::Border)
        .child(
            col()
                .max_w(rel(0.92))
                .align_self(AlignItems::End)
                .bg(ColorToken::Surface)
                .rounded(RadiusToken::Lg)
                .p(sp(3.0))
                .child(markdown(b, move |r: &mut ReadCx<'_>| r.get(question))),
        )
        .child(markdown_stream(b, reply.source, finished, StreamingOptions { effects: Some(REVEAL), ..StreamingOptions::default() }));
    super::section("Markdown")
        .w(rel(1.0))
        .max_w(680.0)
        .child(
            row()
                .align(AlignItems::Center)
                .gap(sp(2.0))
                .child(text("Local eclipse chat").grow(1.0).text_color(ColorToken::TextMuted).text_size(FontSizeToken::Sm))
                .child(
                    button(
                        move |r: &mut ReadCx<'_>| if r.get(reply.running) { "Streaming…" } else { "Replay answer" },
                        KitSize::Sm,
                        ButtonVariant::Secondary,
                    )
                    .test_id("stream")
                    .disabled(reply.running)
                    .on_press(move |w| reply.replay(w)),
                ),
        )
        .child(transcript)
        .child(
            row().align(AlignItems::End).gap(sp(2.0)).child(input.grow(1.0).basis(0.0)).child(
                button("Send", KitSize::Md, ButtonVariant::Primary).test_id("eclipse-send").disabled(reply.running).on_press(submit),
            ),
        )
        .child(lightbox)
}

fn suggestions(query: &TriggerQuery) -> Vec<Completion> {
    let query_text = query.query.to_lowercase();
    match query.trigger {
        '@' => [("Luna", "user://luna"), ("Kai", "user://kai")]
            .into_iter()
            .filter(|(name, _)| name.to_lowercase().starts_with(&query_text))
            .map(|(name, url)| Completion::new(format!("@{name}"), url))
            .collect(),
        '/' => [("replay", "/replay · Replay the answer"), ("question", "/question · Draft the original question")]
            .into_iter()
            .filter(|(key, _)| key.starts_with(&query_text))
            .map(|(key, title)| Completion::action(title, key))
            .collect(),
        _ => Vec::new(),
    }
}

fn provide_renderer(b: &mut BuildCx<'_>, resolved: Signal<bool>, selected: Signal<Option<(Arc<str>, Arc<str>)>>) {
    let pill = Span {
        color: Some(ColorToken::Accent.into()),
        background: Some(ColorToken::SurfaceSelected.into()),
        padding_x: Some(5.0),
        padding_y: Some(2.0),
        radius: Some(5.0),
        ..Span::default()
    };
    let renderer = MarkdownConfig::chat()
        .link_variant_rules(
            LinkVariantRules::new([LinkVariantRule::new(GUIDE, pill), LinkVariantRule::new("user://", pill)])
                .expect("fixed link patterns fit a DFA"),
        )
        .link_pill_content(move |r, url| {
            if url == GUIDE && r.get(resolved) {
                Some(LinkPillContent {
                    label: Some("NASA Eclipse Guide".into()),
                    icon: Some(icons::BOOK_OPEN.into()),
                    tint: Some(ColorToken::Info.into()),
                })
            } else {
                None
            }
        })
        .on_image_press(move |w, image| {
            if let BlockSlot::Image { url, alt, .. } = image {
                w.set(selected, Some((url.clone(), alt.clone())));
            }
        });
    #[allow(clippy::arc_with_non_send_sync)]
    let renderer: Arc<dyn MarkdownRenderer> = Arc::new(renderer);
    b.provide(renderer);
    b.provide::<Arc<dyn BlockSlotRenderer>>(Arc::new(EclipseSlots));
    b.provide::<Arc<dyn MediaResolver>>(Arc::new(EclipseImages));
}

fn image_preview(b: &mut BuildCx<'_>, image: Signal<Option<(Arc<str>, Arc<str>)>>) -> View {
    let open = b.binding(
        move |r| r.with(image, Option::is_some),
        move |w, open| {
            if !open {
                w.set(image, None);
            }
        },
    );
    dialog("eclipse-lightbox", "Image preview", open, DialogOptions::default(), move |b| {
        let Some((url, alt)) = b.sample(image) else { return col() };
        col()
            .gap(sp(2.0))
            .child(image_source(ImageSource::Uri(url)).w(rel(1.0)).object_fit(ObjectFit::Contain).label(alt.as_ref()))
            .child(text(alt.to_string()).text_size(FontSizeToken::Sm).text_color(ColorToken::TextMuted))
    })
}

struct EclipseSlots;

impl BlockSlotRenderer for EclipseSlots {
    fn render_slot(&self, b: &mut BuildCx<'_>, slot: &BlockSlot) -> Option<El> {
        match slot {
            BlockSlot::Directive { id, name, body, is_complete, .. } if name.as_ref() == "think" => {
                let expanded = b.signal(false);
                let (body, complete) = (*body, *is_complete);
                Some(collapsible(
                    b,
                    &format!("eclipse-think-{id}"),
                    expanded.into(),
                    text(move |r: &mut ReadCx<'_>| if r.get(complete) { "Observation plan" } else { "Planning…" }),
                    move |_| text(move |r: &mut ReadCx<'_>| r.with(body, |body| body.to_string())).p(sp(2.0)),
                ))
            }
            BlockSlot::Code { info, code, is_complete, .. } if info.lang.as_ref() == "brightness-chart" => {
                Some(brightness_chart(*code, *is_complete))
            }
            BlockSlot::Code { info, code, is_complete, .. } if info.info.as_ref() == "rust eclipse-magnitude" => {
                let (code, complete) = (*code, *is_complete);
                Some(
                    artifact("Convert brightness to magnitudes", icons::CODE).child(
                        text(move |r: &mut ReadCx<'_>| r.with(code, |code| code.to_string()))
                            .text_family(FontFamilyToken::Mono)
                            .text_size(FontSizeToken::Sm)
                            .text_reveal(move |r: &mut ReadCx<'_>| REVEAL.reveal(r.with(code, |code| code.len()), r.get(complete))),
                    ),
                )
            }
            BlockSlot::Image { url, alt, .. } if url.as_ref() == "eclipse:moon" => Some(
                image_source(ImageSource::Uri(url.clone()))
                    .w(360.0)
                    .max_w(rel(1.0))
                    .aspect_ratio(540.0 / 188.0)
                    .object_fit(ObjectFit::Contain)
                    .rounded(RadiusToken::Md)
                    .label(alt.as_ref()),
            ),
            _ => None,
        }
    }
}

fn brightness_chart(code: ReadSignal<Arc<str>>, complete: ReadSignal<bool>) -> El {
    artifact("Relative lunar brightness", icons::CHART_COLUMN).child(
        canvas(move |r, c| {
            let rendered = r.get(code);
            let lines = if r.get(complete) { rendered.as_ref() } else { rendered.rsplit_once('\n').map_or("", |(lines, _)| lines) };
            for (index, (stage, value)) in lines
                .lines()
                .filter_map(|line| {
                    let (stage, value) = line.split_once(',')?;
                    let value = value.trim().parse::<f32>().ok().filter(|value| (0.0..=100.0).contains(value))?;
                    Some((stage.trim(), value))
                })
                .take(3)
                .enumerate()
            {
                let y = 33.0 * index as f32;
                let width = c.width();
                c.text(stage, (0.0, y)).size(11.0);
                c.text(format!("{value}%"), (width, y)).align(Align::End).size(11.0);
                c.fill(&Path::rounded_rect(Rect::new(0.0, y + 18.0, width, 6.0), 3.0), ColorToken::Border);
                c.fill(&Path::rounded_rect(Rect::new(0.0, y + 18.0, width * value / 100.0, 6.0), 3.0), ColorToken::Accent);
            }
        })
        .test_id("eclipse-chart")
        .w(rel(1.0))
        .h(94.0)
        .text_color(ColorToken::TextMuted)
        .role(Role::Image)
        .label(move |r: &mut ReadCx<'_>| r.with(code, |code| format!("Relative lunar brightness. {}", code.replace(',', " ")))),
    )
}

fn artifact(title: &str, symbol: icons::LucideIcon) -> El {
    col().gap(sp(3.0)).bg(ColorToken::Surface).rounded(RadiusToken::Md).p(sp(3.0)).child(
        row()
            .align(AlignItems::Center)
            .gap(sp(2.0))
            .child(icon(symbol.into()).size(16.0).text_color(ColorToken::Accent).a11y_hidden())
            .child(text(title).text_size(FontSizeToken::Sm).text_weight(600)),
    )
}

struct EclipseImages;

impl MediaResolver for EclipseImages {
    fn resolve(&self, uri: &str) -> Option<ImageSource> {
        static MOON: OnceLock<Image> = OnceLock::new();
        static DISK: OnceLock<Image> = OnceLock::new();
        let image = match uri {
            "eclipse:moon" => MOON.get_or_init(|| eclipsed_moon(540, 188)),
            "eclipse:disk" => DISK.get_or_init(|| eclipsed_moon(56, 56)),
            _ => return None,
        };
        Some(ImageSource::Pixels(image.clone()))
    }
}

fn eclipsed_moon(width: u32, height: u32) -> Image {
    let craters = [(0.12_f32, -0.15_f32, 0.12_f32), (-0.16, 0.05, 0.16), (0.0, 0.18, 0.1)];
    let mut pixels = Vec::new();
    for y in 0..height {
        for x in 0..width {
            let u = (x as f32 - width as f32 / 2.0) / height as f32;
            let v = y as f32 / height as f32 - 0.5;
            let d = (u * u + v * v).sqrt();
            let hash = x.wrapping_mul(2_240_042_557) ^ y.wrapping_mul(4_111_398_869);
            let color = if d < 0.34 {
                let crater = craters.iter().map(|(x, y, radius)| (1.0 - ((u - x).hypot(v - y) / radius)).max(0.0)).sum::<f32>();
                let light = (0.77 - 0.9 * u + v * 0.3 - crater * 0.46 + (hash % 19) as f32 / 180.0).clamp(0.0, 1.0);
                [(94.0 + 157.0 * light) as u8, (31.0 + 102.0 * light) as u8, (39.0 + 48.0 * light) as u8, 255]
            } else if hash % 503 < 2 {
                [183, 195, 218, 255]
            } else {
                [14, 18, 32, 255]
            };
            pixels.extend(color);
        }
    }
    Image::rgba(width, height, pixels)
}
