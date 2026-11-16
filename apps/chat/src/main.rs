//! The AI chat reference app in a window: `cargo run -p octara-chat`. Conversations are kept in
//! the platform's application data directory (`OCTARA_CHAT_DIR` overrides it).

use octara_chat::engine::Scripted;
use octara_chat::network::{NetworkConfig, NetworkModel};
use octara_chat::{ModelEntry, Options, root, store::Store};
use std::rc::Rc;
use std::time::Duration;

fn setting(name: &str) -> Result<Option<String>, String> {
    match std::env::var(name) {
        Ok(value) => Ok(Some(value)),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(_) => Err(format!("{name} must contain valid Unicode")),
    }
}

fn models() -> Result<Vec<ModelEntry>, String> {
    let config = network_config(setting)?;
    let mut models = Vec::new();
    if let Some(config) = config {
        models.push(ModelEntry::new("network", Rc::new(NetworkModel::new(config)?)));
    }
    models.push(ModelEntry::new("scripted", Rc::new(Scripted::default())));
    Ok(models)
}

fn network_config(get: impl Fn(&str) -> Result<Option<String>, String>) -> Result<Option<NetworkConfig>, String> {
    let endpoint = get("OCTARA_CHAT_ENDPOINT")?;
    let model = get("OCTARA_CHAT_MODEL")?;
    let api_key = get("OCTARA_CHAT_API_KEY")?;
    let timeout = get("OCTARA_CHAT_TIMEOUT_SECS")?;
    let seconds = timeout.as_deref().unwrap_or("60").parse::<u64>().ok().filter(|n| (1..=600).contains(n));
    let timeout = Duration::from_secs(seconds.ok_or("OCTARA_CHAT_TIMEOUT_SECS must be between 1 and 600")?);
    if endpoint.is_some() || model.is_some() || api_key.is_some() {
        let endpoint = endpoint.ok_or("OCTARA_CHAT_ENDPOINT is required for a network model")?;
        let model = model.ok_or("OCTARA_CHAT_MODEL is required for a network model")?;
        // Never forward the OpenRouter credential to a user-supplied endpoint.
        return Ok(Some(NetworkConfig { endpoint, model, api_key, timeout }));
    }
    Ok(get("OPENROUTER_API_KEY")?.map(|key| NetworkConfig {
        endpoint: "https://openrouter.ai/api/v1/chat/completions".into(),
        model: "deepseek/deepseek-v4.1-flash".into(),
        api_key: Some(key),
        timeout,
    }))
}

fn main() -> std::process::ExitCode {
    let models = match models() {
        Ok(models) => models,
        Err(error) => {
            eprintln!("octara-chat: {error}");
            return std::process::ExitCode::FAILURE;
        }
    };
    let options = move || Options { models: models.clone(), store: Store::default_dir().map(Store::new), ..Options::default() };
    octara_shell::run_with(octara_shell::Options { title: "Octara Chat".into(), size: (1180.0, 820.0), ..Default::default() }, move || {
        use octara::prelude::*;
        spawn(async {
            set_theme(
                Theme::light()
                    .color(Token::Background, rgb(0xfdfcf9))
                    .color(Token::Surface, rgb(0xf2f1ec))
                    .color(Token::Border, rgb(0xe3e3da))
                    .color(Token::Text, rgb(0x242d28))
                    .color(Token::TextMuted, rgb(0x69726b))
                    .color(Token::Accent, rgb(0x27634c))
                    .color(Token::AccentHover, rgb(0x1e503d))
                    .color(Token::Focus, rgb(0x347c60)),
            );
        });
        root(options.clone())
    });
    std::process::ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(values: &[(&str, &str)]) -> Option<NetworkConfig> {
        network_config(|name| Ok(values.iter().find(|(key, _)| *key == name).map(|(_, value)| value.to_string()))).unwrap()
    }

    #[test]
    fn openrouter_preset_uses_only_its_own_endpoint_and_explicit_configuration_wins() {
        assert!(config(&[]).is_none());
        let preset = config(&[("OPENROUTER_API_KEY", "test-only")]).unwrap();
        assert_eq!(preset.endpoint, "https://openrouter.ai/api/v1/chat/completions");
        assert_eq!(preset.model, "deepseek/deepseek-v4.1-flash");
        let custom = config(&[
            ("OPENROUTER_API_KEY", "test-only"),
            ("OCTARA_CHAT_ENDPOINT", "http://127.0.0.1/chat"),
            ("OCTARA_CHAT_MODEL", "local"),
        ])
        .unwrap();
        assert!(custom.api_key.is_none(), "the global credential must not leak to a custom server");
    }
}
