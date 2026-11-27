use super::Reply;
use crate::os::ServiceCleanup;
use crate::os::files::{accept_extensions, read_file};
use image::{ExtendedColorType, ImageEncoder, codecs::png::PngEncoder};
use octara_model::ask::{Answer, Ask, AskId, ClipboardItem, ClipboardKinds, ServiceError, ServiceId, ServiceOutcome, ServiceRequest};
use octara_model::input::HostReply;
use std::cell::OnceCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak, mpsc};
use std::thread;
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::Window;

type ClipboardJob = Box<dyn FnOnce(&Mutex<arboard::Clipboard>) + Send>;

pub(super) struct Clipboard {
    native: Option<Arc<Mutex<arboard::Clipboard>>>,
    worker: OnceCell<Option<mpsc::Sender<ClipboardJob>>>,
}

impl Clipboard {
    pub(super) fn new() -> Self {
        Self { native: arboard::Clipboard::new().ok().map(|clipboard| Arc::new(Mutex::new(clipboard))), worker: OnceCell::new() }
    }

    pub(super) fn with_native<T>(&self, operation: impl FnOnce(&mut arboard::Clipboard) -> T) -> Option<T> {
        Some(operation(&mut self.native.as_ref()?.lock().expect("desktop clipboard")))
    }

    fn run(&self, job: ClipboardJob) -> Result<(), ServiceError> {
        let native = self.native.as_ref().ok_or(ServiceError::Unavailable)?;
        let worker = self.worker.get_or_init(|| {
            let (send, receive) = mpsc::channel::<ClipboardJob>();
            let native = native.clone();
            // Keep the clipboard alive for its Linux selection and run asynchronous requests in order.
            thread::Builder::new()
                .name("octara-clipboard".into())
                .spawn(move || {
                    for job in receive {
                        job(&native);
                    }
                })
                .ok()
                .map(|_| send)
        });
        worker.as_ref().ok_or(ServiceError::Failed)?.send(job).map_err(|_| ServiceError::Failed)
    }
}

pub(super) struct Services {
    clipboard: Rc<Clipboard>,
    reply: Reply,
    picker: Weak<AtomicBool>,
    asks: HashMap<AskId, Weak<AtomicBool>>,
}

impl Services {
    pub(super) fn new(clipboard: Rc<Clipboard>, reply: impl Fn(HostReply) + Send + Sync + 'static) -> Self {
        Self { clipboard, reply: Arc::new(reply), picker: Weak::new(), asks: HashMap::new() }
    }

    pub(super) fn request(&mut self, window: &Window, id: ServiceId, request: ServiceRequest) -> Option<ServiceCleanup> {
        let result = match request {
            ServiceRequest::ReadClipboard { .. } | ServiceRequest::WriteClipboardText { .. } => {
                let reply = self.reply.clone();
                let live = Arc::new(AtomicBool::new(true));
                let cleanup = Arc::downgrade(&live);
                self.clipboard
                    .run(Box::new(move |native| {
                        let outcome = {
                            let mut native = native.lock().expect("desktop clipboard");
                            if !live.load(Ordering::SeqCst) {
                                return;
                            }
                            clipboard_request(&mut native, request).unwrap_or_else(ServiceOutcome::Error)
                        };
                        complete(live, reply, HostReply::ServiceReply { id, outcome });
                    }))
                    .map(|()| cleanup)
            }
            ServiceRequest::PickFile { max_bytes, accept } => {
                let mut dialog = file_dialog(window);
                let extensions = accept_extensions(&accept);
                if !extensions.is_empty() {
                    dialog = dialog.add_filter("Accepted files", &extensions);
                }
                self.pick(move |live| {
                    let selected = dialog.pick_file();
                    let outcome = selected.as_deref().map(|path| read_file(path, max_bytes, live)).transpose();
                    let outcome = outcome.map_or_else(ServiceOutcome::Error, ServiceOutcome::FilePicked);
                    HostReply::ServiceReply { id, outcome }
                })
            }
        };
        match result {
            Ok(live) => Some(Box::new(move || cancel(live))),
            Err(error) => {
                (self.reply)(HostReply::ServiceReply { id, outcome: ServiceOutcome::Error(error) });
                Some(Box::new(|| {}))
            }
        }
    }

    pub(super) fn show_ask(&mut self, window: &Window, id: AskId, ask: Ask) {
        self.asks.retain(|_, live| live.strong_count() > 0);
        let dismissed = Answer::dismissed(&ask);
        let fallback = dismissed.clone();
        let dialog = file_dialog(window);
        let result = self.pick(move |_| {
            let answer = match ask {
                Ask::Paths(options) => {
                    let dialog = if let Some(title) = options.prompt { dialog.set_title(title) } else { dialog };
                    let paths = match (options.directories, options.multiple) {
                        (true, true) => dialog.pick_folders(),
                        (true, false) => dialog.pick_folder().map(|path| vec![path]),
                        (false, true) => dialog.pick_files(),
                        (false, false) => dialog.pick_file().map(|path| vec![path]),
                    };
                    Answer::Paths(paths.filter(|paths| !paths.is_empty()))
                }
                Ask::NewPath { directory, name } => {
                    let dialog = dialog.set_directory(directory);
                    let dialog = if let Some(name) = name { dialog.set_file_name(name) } else { dialog };
                    Answer::NewPath(dialog.save_file())
                }
                Ask::Prompt { .. } | Ask::Menu { .. } => dismissed,
            };
            HostReply::AskAnswer { id, answer }
        });
        match result {
            Ok(live) => {
                self.asks.insert(id, live);
            }
            Err(_) => (self.reply)(HostReply::AskAnswer { id, answer: fallback }),
        }
    }

    pub(super) fn withdraw_ask(&mut self, id: AskId) {
        if let Some(live) = self.asks.remove(&id) {
            cancel(live);
        }
    }

    fn pick(&mut self, run: impl FnOnce(&AtomicBool) -> HostReply + Send + 'static) -> Result<Weak<AtomicBool>, ServiceError> {
        if self.picker.strong_count() > 0 {
            return Err(ServiceError::Busy);
        }
        let live = Arc::new(AtomicBool::new(true));
        let token = Arc::downgrade(&live);
        let reply = self.reply.clone();
        thread::Builder::new()
            .name("octara-file-picker".into())
            .spawn(move || {
                if live.load(Ordering::SeqCst) {
                    let event = run(&live);
                    complete(live, reply, event);
                }
            })
            .map_err(|_| ServiceError::Failed)?;
        self.picker = token.clone();
        Ok(token)
    }
}

impl Drop for Services {
    fn drop(&mut self) {
        if let Some(live) = self.picker.upgrade() {
            live.store(false, Ordering::SeqCst);
        }
    }
}

fn complete(live: Arc<AtomicBool>, reply: Reply, event: HostReply) {
    let deliver = live.swap(false, Ordering::SeqCst);
    drop(live);
    if deliver {
        reply(event);
    }
}

fn cancel(live: Weak<AtomicBool>) {
    if let Some(live) = live.upgrade() {
        live.store(false, Ordering::SeqCst);
    }
}

fn file_dialog(window: &Window) -> rfd::FileDialog {
    let dialog = rfd::FileDialog::new();
    // XIDs and HWNDs are independent of the borrowed view. A Wayland parent would keep
    // a raw surface pointer after the window can close.
    match window.window_handle().map(|handle| handle.as_raw()) {
        Ok(RawWindowHandle::Xlib(_) | RawWindowHandle::Xcb(_) | RawWindowHandle::Win32(_)) => dialog.set_parent(window),
        _ => dialog,
    }
}

fn clipboard_request(native: &mut arboard::Clipboard, request: ServiceRequest) -> Result<ServiceOutcome, ServiceError> {
    match request {
        ServiceRequest::ReadClipboard { accept } => read_clipboard(native, accept).map(ServiceOutcome::Clipboard),
        ServiceRequest::WriteClipboardText { text } => {
            native.set_text(text).map(|()| ServiceOutcome::ClipboardWritten).map_err(clipboard_error)
        }
        ServiceRequest::PickFile { .. } => Err(ServiceError::Unavailable),
    }
}

pub(super) fn read_clipboard(native: &mut arboard::Clipboard, accept: ClipboardKinds) -> Result<Vec<ClipboardItem>, ServiceError> {
    let mut items = Vec::new();
    let mut error = None;
    for result in [
        accept.contains(ClipboardKinds::TEXT).then(|| native.get_text().map(ClipboardItem::Text)),
        accept.contains(ClipboardKinds::FILES).then(|| native.get().file_list().map(ClipboardItem::Files)),
        accept.contains(ClipboardKinds::IMAGE).then(|| native.get_image().and_then(encode_image)),
    ]
    .into_iter()
    .flatten()
    {
        match result {
            Ok(item) => items.push(item),
            Err(arboard::Error::ContentNotAvailable) => {}
            Err(reason) => error = Some(clipboard_error(reason)),
        }
    }
    if items.is_empty()
        && let Some(error) = error
    {
        return Err(error);
    }
    Ok(items)
}

fn encode_image(image: arboard::ImageData<'_>) -> Result<ClipboardItem, arboard::Error> {
    let width = u32::try_from(image.width).map_err(|_| arboard::Error::ConversionFailure)?;
    let height = u32::try_from(image.height).map_err(|_| arboard::Error::ConversionFailure)?;
    if image.width.checked_mul(image.height).and_then(|pixels| pixels.checked_mul(4)) != Some(image.bytes.len()) {
        return Err(arboard::Error::ConversionFailure);
    }
    let mut bytes = Vec::new();
    PngEncoder::new(&mut bytes)
        .write_image(&image.bytes, width, height, ExtendedColorType::Rgba8)
        .map_err(|_| arboard::Error::ConversionFailure)?;
    Ok(ClipboardItem::Image { media_type: "image/png".into(), bytes })
}

fn clipboard_error(error: arboard::Error) -> ServiceError {
    match error {
        arboard::Error::ClipboardNotSupported => ServiceError::Unavailable,
        _ => ServiceError::Failed,
    }
}
