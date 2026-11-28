use crate::os::{PlatformEvent, PlatformSender};
use futures_lite::{StreamExt, future};
use octara_model::theme::ColorScheme;
use std::collections::HashMap;
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use zbus::zvariant::OwnedValue;

const APPEARANCE: &str = "org.freedesktop.appearance";
const GNOME_INTERFACE: &str = "org.gnome.desktop.interface";

type Settings = HashMap<String, HashMap<String, OwnedValue>>;

pub(super) fn locale() -> Option<Arc<str>> {
    let locale = ["LC_ALL", "LC_MESSAGES", "LANG"].into_iter().filter_map(|key| std::env::var(key).ok()).find(|value| !value.is_empty())?;
    let (tag, modifier) = locale.split_once('@').unwrap_or((&locale, ""));
    let tag = tag.split('.').next()?;
    if tag.is_empty() || matches!(tag, "C" | "POSIX") {
        return None;
    }
    let tag = tag.replace('_', "-");
    let script = match modifier {
        "latin" => "Latn",
        "cyrillic" => "Cyrl",
        _ => return Some(Arc::from(tag)),
    };
    let (language, rest) = tag.split_once('-').unwrap_or((&tag, ""));
    Some(Arc::from(if rest.is_empty() { format!("{language}-{script}") } else { format!("{language}-{script}-{rest}") }))
}

pub(super) struct Observer {
    cancel: async_channel::Sender<()>,
    thread: Option<JoinHandle<()>>,
}

impl Observer {
    pub(super) fn start(sender: PlatformSender) -> std::io::Result<Self> {
        // The portal may need D-Bus activation. Publish its first snapshot without blocking window startup.
        let (cancel, stopped) = async_channel::bounded(1);
        let thread = thread::Builder::new().name("octara-system-preferences".into()).spawn(move || {
            future::block_on(future::race(
                async {
                    let _ = stopped.recv().await;
                },
                async {
                    let _ = watch(&sender).await;
                },
            ));
        })?;
        Ok(Self { cancel, thread: Some(thread) })
    }
}

impl Drop for Observer {
    fn drop(&mut self) {
        self.cancel.close();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

async fn watch(sender: &PlatformSender) -> zbus::Result<()> {
    let connection = zbus::Connection::session().await?;
    let portal = zbus::Proxy::new(
        &connection,
        "org.freedesktop.portal.Desktop",
        "/org/freedesktop/portal/desktop",
        "org.freedesktop.portal.Settings",
    )
    .await?;
    let mut changes = portal.receive_signal("SettingChanged").await?;
    let namespaces = [APPEARANCE, GNOME_INTERFACE];
    let mut settings: Settings = portal.call("ReadAll", &(namespaces.as_slice(),)).await?;
    publish(sender, &settings);
    while let Some(change) = changes.next().await {
        if let Ok((namespace, key, value)) = change.body().deserialize::<(String, String, OwnedValue)>()
            && matches!(namespace.as_str(), APPEARANCE | GNOME_INTERFACE)
        {
            settings.entry(namespace).or_default().insert(key, value);
            publish(sender, &settings);
        }
    }
    Ok(())
}

fn publish(sender: &PlatformSender, settings: &Settings) {
    let appearance = settings.get(APPEARANCE);
    let color_scheme = appearance
        .and_then(|values| values.get("color-scheme"))
        .and_then(|value| u32::try_from(value).ok())
        .map(|scheme| if scheme == 1 { ColorScheme::Dark } else { ColorScheme::Light });
    let reduced_motion = appearance
        .and_then(|values| values.get("reduced-motion"))
        .and_then(|value| u32::try_from(value).ok())
        .map(|motion| motion == 1)
        .or_else(|| {
            // Older GNOME portals expose the toolkit animation preference.
            let value = settings.get(GNOME_INTERFACE)?.get("enable-animations")?;
            bool::try_from(value).ok().map(|enabled| !enabled)
        });
    let high_contrast = appearance.and_then(|values| values.get("contrast")).and_then(|value| u32::try_from(value).ok()).map(|v| v == 1);
    let text_scale = settings
        .get(GNOME_INTERFACE)
        .and_then(|values| values.get("text-scaling-factor"))
        .and_then(|value| f64::try_from(value).ok())
        .map(|value| value as f32)
        .filter(|scale| scale.is_finite() && *scale > 0.0);
    sender.send_event(PlatformEvent::SystemPreferencesChanged { color_scheme, reduced_motion, text_scale, high_contrast });
}
