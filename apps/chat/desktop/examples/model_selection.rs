//! The ordinary Chat root with two loopback models for checking model switches against the HTTP fixture.

use octara::prelude::{ScopeId, Sender, WriteCx};
use octara_chat::engine::{Cancel, Model, Request};
use octara_chat::model::Event;
use octara_chat::network::{NetworkConfig, NetworkModel};
use octara_chat::store::Store;
use octara_chat::{ModelEntry, Options, definition};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

struct Named {
    name: &'static str,
    network: NetworkModel,
}

impl Model for Named {
    fn name(&self) -> &str {
        self.name
    }

    fn respond(&self, w: &mut WriteCx<'_>, owner: ScopeId, request: Request, send: Sender<Event>) -> Cancel {
        self.network.respond(w, owner, request, send)
    }
}

fn main() {
    let endpoint = std::env::var("OCTARA_CHAT_ENDPOINT").expect("set OCTARA_CHAT_ENDPOINT to the loopback fixture");
    let url = reqwest::Url::parse(&endpoint).expect("a complete loopback HTTP URL");
    assert!(url.scheme() == "http" && url.host_str() == Some("127.0.0.1"), "this verification example only connects to loopback HTTP");
    let directory = std::env::var_os("OCTARA_CHAT_DIR").map(PathBuf::from).expect("set an isolated OCTARA_CHAT_DIR");
    let models: Vec<_> = [("local-a", "wire-a", "本地模型 A"), ("local-b", "wire-b", "本地模型 B")]
        .into_iter()
        .map(|(id, wire, name)| {
            let network = NetworkModel::new(NetworkConfig {
                endpoint: endpoint.clone(),
                model: wire.into(),
                api_key: None,
                timeout: Duration::from_secs(120),
            })
            .expect("valid local fixture settings");
            ModelEntry::new(id, Rc::new(Named { name, network }))
        })
        .collect();
    let store = Store::new(directory);
    octara_host_native::run_app(definition(Options { models, store: Some(store), ..Options::default() })).unwrap();
}
