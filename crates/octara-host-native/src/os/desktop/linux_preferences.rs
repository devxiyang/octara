use crate::ShellEvent;
use futures_lite::{StreamExt, future};
use octara_model::theme::ColorScheme;
use std::collections::HashMap;
use std::thread::{self, JoinHandle};
use winit::event_loop::EventLoopProxy;
use zbus::zvariant::OwnedValue;

const APPEARANCE: &str = "org.freedesktop.appearance";
const GNOME_INTERFACE: &str = "org.gnome.desktop.interface";

type Settings = HashMap<String, HashMap<String, OwnedValue>>;

pub(super) struct Observer {
    cancel: async_channel::Sender<()>,
    thread: Option<JoinHandle<()>>,
}

impl Observer {
    pub(super) fn start(proxy: EventLoopProxy<ShellEvent>) -> std::io::Result<Self> {
        // The portal may need D-Bus activation. Publish its first snapshot without blocking window startup.
        let (cancel, stopped) = async_channel::bounded(1);
        let thread = thread::Builder::new().name("octara-system-preferences".into()).spawn(move || {
            future::block_on(future::race(
                async {
                    let _ = stopped.recv().await;
                },
                async {
                    let _ = watch(&proxy).await;
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

async fn watch(proxy: &EventLoopProxy<ShellEvent>) -> zbus::Result<()> {
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
    publish(proxy, &settings);
    while let Some(change) = changes.next().await {
        if let Ok((namespace, key, value)) = change.body().deserialize::<(String, String, OwnedValue)>()
            && matches!(namespace.as_str(), APPEARANCE | GNOME_INTERFACE)
        {
            settings.entry(namespace).or_default().insert(key, value);
            publish(proxy, &settings);
        }
    }
    Ok(())
}

fn publish(proxy: &EventLoopProxy<ShellEvent>, settings: &Settings) {
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
    let _ = proxy.send_event(ShellEvent::SystemPreferencesChanged { color_scheme, reduced_motion });
}
