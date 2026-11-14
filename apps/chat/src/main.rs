//! The AI chat reference app in a window: `cargo run -p octara-chat`. Conversations are kept in
//! the platform's application data directory (`OCTARA_CHAT_DIR` overrides it).

use octara_chat::engine::{Model, Scripted};
use octara_chat::network::{NetworkConfig, NetworkModel};
use octara_chat::{Options, root, store::Store};
use std::rc::Rc;
use std::time::Duration;

fn setting(name: &str) -> Result<Option<String>, String> {
    match std::env::var(name) {
        Ok(value) => Ok(Some(value)),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(_) => Err(format!("{name} must contain valid Unicode")),
    }
}

fn models() -> Result<Vec<Rc<dyn Model>>, String> {
    let endpoint = setting("OCTARA_CHAT_ENDPOINT")?;
    let model = setting("OCTARA_CHAT_MODEL")?;
    let api_key = setting("OCTARA_CHAT_API_KEY")?;
    let timeout = setting("OCTARA_CHAT_TIMEOUT_SECS")?;
    let mut models: Vec<Rc<dyn Model>> = Vec::new();
    if endpoint.is_some() || model.is_some() || api_key.is_some() || timeout.is_some() {
        let endpoint = endpoint.ok_or("OCTARA_CHAT_ENDPOINT is required for a network model")?;
        let model = model.ok_or("OCTARA_CHAT_MODEL is required for a network model")?;
        let seconds = timeout.as_deref().unwrap_or("30").parse::<u64>().ok().filter(|n| (1..=600).contains(n));
        let timeout = Duration::from_secs(seconds.ok_or("OCTARA_CHAT_TIMEOUT_SECS must be between 1 and 600")?);
        models.push(Rc::new(NetworkModel::new(NetworkConfig { endpoint, model, api_key, timeout })?));
    }
    models.push(Rc::new(Scripted::default()));
    Ok(models)
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
    octara_shell::run_with(octara_shell::Options { title: "Octara Chat".into(), size: (1000.0, 720.0), ..Default::default() }, move || {
        root(options.clone())
    });
    std::process::ExitCode::SUCCESS
}
