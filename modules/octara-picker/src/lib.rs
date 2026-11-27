//! User-initiated file, folder, save destination, and media selection.

use std::any::Any;
use std::path::{Component, Path, PathBuf};
#[cfg(feature = "platform")]
use std::sync::Arc;
#[cfg(feature = "platform")]
use std::sync::atomic::{AtomicU8, Ordering};

use octara::module::{CallTicket, ModuleCall, ModuleCallCx, ModuleError, ModuleId, OctaraModule};
#[cfg(feature = "platform")]
use octara::module::{HostSurface, HostSurfaceKind, ModuleDispatcher};

#[cfg(any(target_os = "macos", target_os = "ios"))]
#[cfg(feature = "platform")]
mod apple;
#[cfg(any(
    target_os = "windows",
    target_os = "linux",
    target_os = "freebsd",
    target_os = "dragonfly",
    target_os = "netbsd",
    target_os = "openbsd"
))]
#[cfg(feature = "platform")]
mod desktop;
#[cfg(target_os = "ios")]
#[cfg(feature = "platform")]
mod ios;
#[cfg(target_os = "macos")]
#[cfg(feature = "platform")]
mod macos;
#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "windows",
    target_os = "linux",
    target_os = "freebsd",
    target_os = "dragonfly",
    target_os = "netbsd",
    target_os = "openbsd"
))]
#[cfg(feature = "platform")]
mod native;
#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
#[cfg(feature = "platform")]
mod web;

pub const ID: ModuleId = ModuleId("octara.picker");

/// Opens a file picker. An empty `accept` allows all file types.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PickFiles {
    pub title: Option<String>,
    pub directory: Option<PathBuf>,
    /// Comma-separated file extensions (including the leading dot) and MIME types.
    pub accept: String,
    pub multiple: bool,
    /// Maximum bytes per file. Setting a limit also requests bytes on native hosts.
    pub max_bytes: Option<usize>,
}

/// A file selected by the user. A successful selection has a path, bytes, or both.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PickedFile {
    /// A local file path when available. Imported iOS documents are temporary copies.
    pub path: Option<PathBuf>,
    pub name: String,
    /// The available MIME type hint, or an empty string if unknown.
    pub media_type: String,
    pub bytes: Option<Vec<u8>>,
}

/// Opens a picker for one folder.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PickFolder {
    pub title: Option<String>,
    pub directory: Option<PathBuf>,
    /// Maximum combined file size when a host supplies folder contents in memory.
    pub max_bytes: Option<usize>,
}

#[derive(Clone, Debug)]
pub enum PickedFolder {
    Path(PickedPath),
    /// Browsers provide file contents and names relative to the chosen folder.
    Files {
        name: String,
        files: Vec<FolderFile>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FolderFile {
    /// The file's slash-separated name relative to the chosen folder.
    pub relative_path: String,
    pub file: PickedFile,
}

/// Opens a picker for a filesystem destination. On iOS the user chooses the
/// parent folder and `name` is required. Browsers return [`ModuleError::Unavailable`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PickSavePath {
    pub title: Option<String>,
    pub directory: Option<PathBuf>,
    pub name: Option<String>,
}

/// A selected filesystem path. Keep this value alive while accessing a
/// security-scoped directory or destination on iOS.
#[derive(Clone, Debug)]
pub struct PickedPath {
    path: PathBuf,
    #[cfg(all(feature = "platform", target_os = "ios"))]
    _scope: Option<Arc<ios::SecurityScope>>,
}

impl PickedPath {
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl AsRef<Path> for PickedPath {
    fn as_ref(&self) -> &Path {
        self.path()
    }
}

impl From<PathBuf> for PickedPath {
    fn from(path: PathBuf) -> Self {
        Self {
            path,
            #[cfg(all(feature = "platform", target_os = "ios"))]
            _scope: None,
        }
    }
}

/// Opens the system photo library on iOS and a media file chooser elsewhere.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PickMedia {
    pub kind: MediaKind,
    pub multiple: bool,
    /// Maximum bytes per file. Setting a limit also requests bytes on desktop.
    pub max_bytes: Option<usize>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MediaKind {
    Photos,
    Videos,
    #[default]
    PhotosAndVideos,
}

#[cfg(any(
    target_os = "windows",
    target_os = "linux",
    target_os = "freebsd",
    target_os = "dragonfly",
    target_os = "netbsd",
    target_os = "openbsd",
    all(target_arch = "wasm32", target_os = "unknown")
))]
#[cfg(feature = "platform")]
impl MediaKind {
    fn accept(self) -> &'static str {
        match self {
            Self::Photos => "image/*",
            Self::Videos => "video/*",
            Self::PhotosAndVideos => "image/*,video/*",
        }
    }
}

macro_rules! picker_calls {
    ($($call:ident => ($method:literal, $output:ty)),+ $(,)?) => {
        $(impl ModuleCall for $call {
            const MODULE: ModuleId = ID;
            const METHOD: &'static str = $method;
            const SENSITIVE: bool = true;
            type Output = $output;
        })+
    };
}

picker_calls! {
    PickFiles => ("pick_files", Vec<PickedFile>),
    PickFolder => ("pick_folder", PickedFolder),
    PickSavePath => ("pick_save_path", PickedPath),
    PickMedia => ("pick_media", Vec<PickedFile>),
}

/// Creates the system picker module for installation with `AppDefinition::with_module`.
/// Enable the default `platform` feature to run system pickers.
pub fn module() -> impl OctaraModule {
    Picker::default()
}

/// Creates a deterministic picker that cancels known calls. Use
/// `HeadlessDriver::stub_module_call` to supply selections in a test.
pub fn mock() -> impl OctaraModule {
    Picker { mock: true, ..Default::default() }
}

type Result<T> = std::result::Result<T, ModuleError>;
#[cfg(feature = "platform")]
type Dismiss = Option<Box<dyn FnOnce()>>;

#[cfg_attr(not(feature = "platform"), allow(dead_code))]
enum Request {
    Files(PickFiles),
    Folder(PickFolder),
    Save(PickSavePath),
    Media(PickMedia),
}

impl Request {
    fn decode(value: Box<dyn Any>) -> Result<Self> {
        let value = match value.downcast::<PickFiles>() {
            Ok(value) => return Ok(Self::Files(*value)),
            Err(value) => value,
        };
        let value = match value.downcast::<PickFolder>() {
            Ok(value) => return Ok(Self::Folder(*value)),
            Err(value) => value,
        };
        let value = match value.downcast::<PickSavePath>() {
            Ok(value) => return Ok(Self::Save(*value)),
            Err(value) => value,
        };
        value.downcast::<PickMedia>().map(|value| Self::Media(*value)).map_err(|_| ModuleError::InvalidArgs("unknown picker call".into()))
    }

    fn validate(&self) -> Result<()> {
        if let Self::Save(request) = self
            && let Some(name) = &request.name
            && !is_filename(name)
        {
            return Err(ModuleError::InvalidArgs("the save name must be a single filename".into()));
        }
        Ok(())
    }
}

fn is_filename(name: &str) -> bool {
    !name.contains(['/', '\\', '\0']) && matches!(Path::new(name).components().next(), Some(Component::Normal(_)))
}

#[cfg(feature = "platform")]
const WORKING: u8 = 0;
#[cfg(feature = "platform")]
const CANCELLED: u8 = 1;
#[cfg(feature = "platform")]
const FINISHED: u8 = 2;

#[cfg(feature = "platform")]
#[derive(Clone)]
struct Job {
    ticket: CallTicket,
    dispatcher: ModuleDispatcher,
    state: Arc<AtomicU8>,
}

#[cfg(feature = "platform")]
impl Job {
    fn check_live(&self) -> Result<()> {
        if self.state.load(Ordering::Acquire) == WORKING { Ok(()) } else { Err(ModuleError::Cancelled) }
    }

    fn finish<C: ModuleCall<Output: Send>>(&self, result: Result<C::Output>) {
        if self.state.swap(FINISHED, Ordering::AcqRel) == WORKING {
            let _ = self.dispatcher.resolve::<C>(self.ticket, result);
        }
    }
}

#[cfg(feature = "platform")]
struct Pending {
    job: Job,
    dismiss: Dismiss,
}

#[cfg(feature = "platform")]
impl Pending {
    fn cancel(&mut self) {
        if self.job.state.compare_exchange(WORKING, CANCELLED, Ordering::AcqRel, Ordering::Acquire).is_ok()
            && let Some(dismiss) = self.dismiss.take()
        {
            dismiss();
            self.job.state.store(FINISHED, Ordering::Release);
        }
    }
}

#[cfg(feature = "platform")]
impl Drop for Pending {
    fn drop(&mut self) {
        self.cancel();
    }
}

#[derive(Default)]
struct Picker {
    mock: bool,
    #[cfg(feature = "platform")]
    pending: Option<Pending>,
}

impl Picker {
    fn start(&mut self, ticket: CallTicket, request: Request, cx: &ModuleCallCx<'_>) -> Result<()> {
        if self.mock {
            return Err(ModuleError::Cancelled);
        }
        #[cfg(feature = "platform")]
        {
            if self.pending.as_ref().is_some_and(|pending| pending.job.state.load(Ordering::Acquire) != FINISHED) {
                return Err(ModuleError::Busy);
            }
            let job = Job { ticket, dispatcher: cx.dispatcher(), state: Arc::new(AtomicU8::new(WORKING)) };
            let dismiss = start_platform(request, cx.host_kind(), cx.surface(), job.clone())?;
            self.pending = Some(Pending { job, dismiss });
            Ok(())
        }
        #[cfg(not(feature = "platform"))]
        {
            let _ = (ticket, request, cx);
            Err(ModuleError::Unavailable)
        }
    }
}

impl OctaraModule for Picker {
    fn id(&self) -> ModuleId {
        ID
    }

    fn call(&mut self, ticket: CallTicket, value: Box<dyn Any>, cx: &mut ModuleCallCx<'_>) {
        let result = Request::decode(value).and_then(|request| {
            request.validate()?;
            self.start(ticket, request, cx)
        });
        if let Err(error) = result {
            let _ = cx.reply_erased(Err(error));
        }
    }

    #[cfg(feature = "platform")]
    fn cancel(&mut self, ticket: CallTicket) {
        if let Some(pending) = &mut self.pending
            && pending.job.ticket == ticket
        {
            pending.cancel();
        }
    }

    #[cfg(feature = "platform")]
    fn shutdown(&mut self) {
        self.pending = None;
    }
}

#[cfg(feature = "platform")]
fn start_platform(request: Request, kind: HostSurfaceKind, surface: Option<HostSurface<'_>>, job: Job) -> Result<Dismiss> {
    match kind {
        #[cfg(target_os = "macos")]
        HostSurfaceKind::MacOs => macos::start(request, surface, job),
        #[cfg(target_os = "ios")]
        HostSurfaceKind::Ios => ios::start(request, surface, job),
        #[cfg(any(
            target_os = "windows",
            target_os = "linux",
            target_os = "freebsd",
            target_os = "dragonfly",
            target_os = "netbsd",
            target_os = "openbsd"
        ))]
        HostSurfaceKind::Windows | HostSurfaceKind::Linux => desktop::start(request, surface, job),
        #[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
        HostSurfaceKind::Web => web::start(request, job),
        _ => {
            let _ = (request, surface, job);
            Err(ModuleError::Unavailable)
        }
    }
}

#[cfg(feature = "platform")]
fn too_large(max_bytes: usize) -> ModuleError {
    ModuleError::Failed(format!("selected data exceeds the {max_bytes} byte limit"))
}
