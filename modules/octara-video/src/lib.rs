//! Video playback for Octara applications.
//!
//! Install [`module`] with `AppDefinition::with_module`, then author a video with [`video`].
//! This also enables playback in Markdown video blocks. Install [`mock`] in headless tests.
//! Disable the default `platform` feature for a headless build without platform dependencies.

#[cfg(all(feature = "platform", any(target_os = "macos", target_os = "ios")))]
mod apple;
mod mock;
#[cfg(all(feature = "platform", target_arch = "wasm32", target_os = "unknown"))]
mod web;

use octara::module::{
    AirspacePolicy, CallTicket, HostSurface, HostSurfaceKind, ModuleCallCx, ModuleDispatcher, ModuleError, ModuleId, ModuleInstallCx,
    NativeId, NativeSlot, NativeSnapshot, NativeSnapshotRequest, NativeViewEmitter, NativeViewFactory, NativeViewInstance, OctaraModule,
    TypedNativeProps,
};
use octara::{Picture, VideoAdapter, WriteCx};
use std::any::Any;

pub use octara::{VideoSource, video};

pub const ID: ModuleId = ModuleId("octara.video");

/// An imperative playback control for a mounted video.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VideoCommand {
    Play,
    Pause,
}

/// Queue a playback command for a video with the given test ID in this window.
pub fn send(write: &mut WriteCx<'_>, target_test_id: &str, command: VideoCommand) -> bool {
    write.send_typed_native_command(target_test_id, command, None)
}

/// Enable playback on macOS, iOS, or Web with the `platform` feature.
/// If playback is unavailable, the view displays the authored poster.
pub fn module() -> impl OctaraModule {
    VideoModule { mock: false }
}

/// Enable a deterministic, in-memory playback instance in a headless app.
pub fn mock() -> impl OctaraModule {
    VideoModule { mock: true }
}

struct VideoModule {
    mock: bool,
}

impl OctaraModule for VideoModule {
    fn id(&self) -> ModuleId {
        ID
    }

    fn install(&mut self, cx: &mut ModuleInstallCx<'_>) -> Result<(), ModuleError> {
        let host = cx.host_kind();
        let supported = match host {
            HostSurfaceKind::MacOs => !self.mock && cfg!(all(feature = "platform", target_os = "macos")),
            HostSurfaceKind::Ios => !self.mock && cfg!(all(feature = "platform", target_os = "ios")),
            HostSurfaceKind::Web => !self.mock && cfg!(all(feature = "platform", target_arch = "wasm32", target_os = "unknown")),
            HostSurfaceKind::Headless => self.mock,
            _ => false,
        };
        if supported {
            cx.native_views().register(Factory(host))?;
            cx.build().provide(VideoAdapter { kind: ID.0, props: |source| TypedNativeProps::new(source, None) });
        }
        Ok(())
    }

    fn call(&mut self, _: CallTicket, _: Box<dyn Any>, cx: &mut ModuleCallCx<'_>) {
        let _ = cx.reply_erased(Err(ModuleError::Unavailable));
    }
}

struct Factory(HostSurfaceKind);

impl NativeViewFactory for Factory {
    fn kind(&self) -> &'static str {
        ID.0
    }

    fn airspace_policy(&self) -> AirspacePolicy {
        AirspacePolicy::SnapshotOnOcclude
    }

    fn supports_slot(&self, surface: HostSurfaceKind, slot: NativeSlot) -> bool {
        self.0 == surface && slot == NativeSlot::Overlay
    }

    fn create(
        &mut self,
        surface: HostSurface<'_>,
        _: NativeId,
        _: NativeSlot,
        props: Box<dyn Any>,
        _: NativeViewEmitter,
    ) -> Result<Box<dyn NativeViewInstance>, ModuleError> {
        let source = *props.downcast::<VideoSource>().map_err(|_| ModuleError::InvalidArgs("expected VideoSource".into()))?;
        match surface.kind() {
            HostSurfaceKind::Headless => Ok(Box::new(mock::Video::new(source))),
            #[cfg(all(feature = "platform", any(target_os = "macos", target_os = "ios")))]
            HostSurfaceKind::MacOs | HostSurfaceKind::Ios => Ok(Box::new(apple::Video::new(surface, source)?)),
            #[cfg(all(feature = "platform", target_arch = "wasm32", target_os = "unknown"))]
            HostSurfaceKind::Web => Ok(Box::new(web::Video::new(surface, source)?)),
            _ => Err(ModuleError::Unavailable),
        }
    }
}

fn snapshot_unavailable(request: NativeSnapshotRequest, reply: ModuleDispatcher) {
    let picture = Picture::new(1, 1, vec![0; 4]).expect("one transparent pixel");
    let _ = reply.resolve_native_snapshot(request, NativeSnapshot { picture, misses: vec!["Video frame was not captured".into()] });
}

#[cfg(test)]
mod tests;
