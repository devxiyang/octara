use crate::chat::{ModelEntry, Options};
use crate::network::{NetworkConfig, NetworkModel};
use crate::store::Store;
use crate::ui::root;
use octara::app::{AppConfig, AppDefinition, AppMode};
use octara::prelude::{Token, WindowOptions, rgb};
use std::rc::Rc;
use std::time::Duration;

const FIXTURE_UNIX_TIME: u64 = 1_800_000_000;

/// Defines Chat's initial window using the host's platform and text resources.
/// Native startup uses the model and storage settings from the environment. Headless startup
/// uses the scripted model, an in-memory session, and a fixed clock for conversation dates.
pub fn app(mode: AppMode, mut config: AppConfig) -> Result<AppDefinition, String> {
    let mut options = Options::default();
    match mode {
        AppMode::Native => {
            if let Some(network) = network_config(setting)? {
                options.models.insert(0, ModelEntry::new("network", Rc::new(NetworkModel::new(network)?)));
            }
            options.store = Store::default_dir().map(Store::new);
        }
        AppMode::Headless => options.clock = Rc::new(|| FIXTURE_UNIX_TIME),
    }

    if config.theme.name == "light" {
        config.theme = config
            .theme
            .color(Token::Background, rgb(0xfdfcf9))
            .color(Token::Surface, rgb(0xf2f1ec))
            .color(Token::Border, rgb(0xe3e3da))
            .color(Token::Text, rgb(0x242d28))
            .color(Token::TextMuted, rgb(0x677069))
            .color(Token::Accent, rgb(0x27634c))
            .color(Token::AccentHover, rgb(0x1e503d))
            .color(Token::Focus, rgb(0x347c60));
    }

    Ok(AppDefinition::new(config, WindowOptions::new("Octara Chat").size(1180.0, 820.0), move || {
        let options = options.clone();
        root(move || options.clone())
    }))
}

fn setting(name: &str) -> Result<Option<String>, String> {
    match std::env::var(name) {
        Ok(value) => Ok(Some(value)),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(_) => Err(format!("{name} must contain valid Unicode")),
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openrouter_preset_uses_only_its_own_endpoint_and_explicit_configuration_wins() {
        assert!(network_config(|_| Ok(None)).unwrap().is_none());
        let preset = network_config(|name| Ok((name == "OPENROUTER_API_KEY").then(|| "test-only".into()))).unwrap().unwrap();
        assert_eq!(preset.endpoint, "https://openrouter.ai/api/v1/chat/completions");
        assert_eq!(preset.model, "deepseek/deepseek-v4.1-flash");

        let custom = network_config(|name| {
            Ok(match name {
                "OCTARA_CHAT_ENDPOINT" => Some("http://127.0.0.1/chat".into()),
                "OCTARA_CHAT_MODEL" => Some("local".into()),
                "OPENROUTER_API_KEY" => panic!("explicit configuration must not look up the OpenRouter credential"),
                _ => None,
            })
        })
        .unwrap()
        .unwrap();
        assert!(custom.api_key.is_none());
    }
}
