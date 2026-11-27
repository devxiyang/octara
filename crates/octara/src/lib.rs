//! Author views and applications using explicit reactive capabilities.

extern crate self as octara;

mod app;
mod authoring;
pub mod declarative;
pub mod module {
    //! Explicit application modules and platform view extension points.
    pub use octara_model::identity::NativeId;
    pub use octara_model::native::{
        NativeChange, NativeMeasureRequest, NativePlacement, NativeSlot, NativeSnapshot, NativeSnapshotRequest, TypedNativeCommand,
        TypedNativeEvent, TypedNativeProps,
    };
    pub use octara_runtime::module::*;
}
pub mod view;

pub use app::{App, AppDefinition, Window, WindowActions, WindowDefinition};
pub use octara_model as model;
pub use octara_model::diagnostics;
pub use octara_runtime as runtime;
pub use octara_tree as tree;
pub use view::*;

pub use authoring::*;
pub use octara_model::{
    capture::{CaptureSource, FrameReceipt, PaintFallback, PaintOutcome, PaintSample},
    identity::{EditorSessionId, TargetGuard},
    input::{BackPhase, ContactTarget, EditorEvent, HostReply, InteractionEvent, PointerId, PointerKind, WheelPhase},
    paint::{DisplayList, Item as PaintItem, Picture},
    text::{AtomicRange, CommitKind, TextEdit},
};
pub use octara_runtime::cycle::{AppError, CycleReceipt};
pub use octara_runtime::media::{CancellationToken, LoadedMedia, MediaFuture, MediaLoader, MediaProgress, MediaRequest, VideoAdapter};
pub use octara_runtime::{AppRuntime, CommitOutput, HostCommand, HostEngine, Input, ProjectError, ProjectFeedback, WindowRuntime};
pub use octara_tree::{
    NodeId, Tree, TreeDelta,
    editor::{EditorBackend, EditorRichText, EditorRope, EditorSession, EditorState},
};

/// Imports for authoring views with Octara.
pub mod prelude {
    pub use crate::authoring::*;
    pub use crate::view::*;
    pub use crate::{
        App, AppDefinition, AppError, AtomicRange, EditorBackend, EditorRichText, EditorRope, EditorState, Store, Window, WindowActions,
        WindowDefinition, ui,
    };
}
