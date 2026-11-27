use super::Reply;
use crate::os::ServiceCleanup;
use image::{ExtendedColorType, ImageEncoder, codecs::png::PngEncoder};
use octara_model::ask::{ClipboardItem, ClipboardKinds, ServiceError, ServiceId, ServiceOutcome, ServiceRequest};
use octara_model::input::HostReply;
use std::cell::OnceCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak, mpsc};
use std::thread;

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
}

impl Services {
    pub(super) fn new(clipboard: Rc<Clipboard>, reply: impl Fn(HostReply) + Send + Sync + 'static) -> Self {
        Self { clipboard, reply: Arc::new(reply) }
    }

    pub(super) fn request(&mut self, id: ServiceId, request: ServiceRequest) -> Option<ServiceCleanup> {
        let reply = self.reply.clone();
        let live = Arc::new(AtomicBool::new(true));
        let cleanup = Arc::downgrade(&live);
        let result = self
            .clipboard
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
            .map(|()| cleanup);
        match result {
            Ok(live) => Some(Box::new(move || cancel(live))),
            Err(error) => {
                (self.reply)(HostReply::ServiceReply { id, outcome: ServiceOutcome::Error(error) });
                Some(Box::new(|| {}))
            }
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

fn clipboard_request(native: &mut arboard::Clipboard, request: ServiceRequest) -> Result<ServiceOutcome, ServiceError> {
    match request {
        ServiceRequest::ReadClipboard { accept } => read_clipboard(native, accept).map(ServiceOutcome::Clipboard),
        ServiceRequest::WriteClipboardText { text } => {
            native.set_text(text).map(|()| ServiceOutcome::ClipboardWritten).map_err(clipboard_error)
        }
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
