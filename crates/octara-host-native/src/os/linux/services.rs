use crate::os::ServiceCleanup;
use image::{ExtendedColorType, ImageEncoder, codecs::png::PngEncoder};
use octara_model::ask::{ClipboardItem, ClipboardKinds, ServiceError, ServiceId, ServiceOutcome, ServiceRequest};
use octara_model::input::HostReply;
use std::cell::OnceCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak, mpsc};
use std::thread;

type Reply = Arc<dyn Fn(HostReply) + Send + Sync>;

type ClipboardJob = Box<dyn FnOnce(&Mutex<ClipboardState>) + Send>;

pub(super) struct Clipboard {
    state: Option<Arc<Mutex<ClipboardState>>>,
    worker: OnceCell<Option<mpsc::Sender<ClipboardJob>>>,
}

struct ClipboardState {
    native: arboard::Clipboard,
    mixed: super::linux_clipboard::MixedClipboard,
}

impl Clipboard {
    pub(super) fn new() -> Self {
        let state = arboard::Clipboard::new()
            .ok()
            .map(|native| Arc::new(Mutex::new(ClipboardState { native, mixed: super::linux_clipboard::MixedClipboard::new() })));
        Self { state, worker: OnceCell::new() }
    }

    pub(super) fn with_native<T>(&self, operation: impl FnOnce(&mut arboard::Clipboard) -> T) -> Option<T> {
        Some(operation(&mut self.state.as_ref()?.lock().expect("desktop clipboard").native))
    }

    pub(super) fn write(&self, items: &[ClipboardItem]) -> Result<(), ServiceError> {
        self.state.as_ref().ok_or(ServiceError::Unavailable)?.lock().expect("desktop clipboard").write(items)
    }

    fn run(&self, job: ClipboardJob) -> Result<(), ServiceError> {
        let state = self.state.as_ref().ok_or(ServiceError::Unavailable)?;
        let worker = self.worker.get_or_init(|| {
            let (send, receive) = mpsc::channel::<ClipboardJob>();
            let state = state.clone();
            // Keep the clipboard alive for its Linux selection and run asynchronous requests in order.
            thread::Builder::new()
                .name("octara-clipboard".into())
                .spawn(move || {
                    for job in receive {
                        job(&state);
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
            .run(Box::new(move |state| {
                let outcome = {
                    let mut state = state.lock().expect("desktop clipboard");
                    if !live.load(Ordering::SeqCst) {
                        return;
                    }
                    state.request(request).unwrap_or_else(ServiceOutcome::Error)
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

impl ClipboardState {
    fn request(&mut self, request: ServiceRequest) -> Result<ServiceOutcome, ServiceError> {
        match request {
            ServiceRequest::ReadClipboard { accept } => read_clipboard(&mut self.native, accept).map(ServiceOutcome::Clipboard),
            ServiceRequest::WriteClipboard { items } => self.write(&items).map(|()| ServiceOutcome::ClipboardWritten),
        }
    }

    fn write(&mut self, items: &[ClipboardItem]) -> Result<(), ServiceError> {
        let result = match items {
            [ClipboardItem::Text(text)] => self.native.set_text(text.as_str()),
            [ClipboardItem::Image { media_type, bytes }] if media_type == "image/png" => self.native.set_image(decode_png(bytes)?),
            [ClipboardItem::Files(paths)] if !paths.is_empty() => self.native.set().file_list(paths),
            _ => return self.mixed.write(items),
        };
        result.map_err(clipboard_error)
    }
}

fn decode_png(bytes: &[u8]) -> Result<arboard::ImageData<'static>, ServiceError> {
    let rgba = load_png(bytes)?.into_rgba8();
    Ok(arboard::ImageData { width: rgba.width() as usize, height: rgba.height() as usize, bytes: rgba.into_raw().into() })
}

pub(super) fn load_png(bytes: &[u8]) -> Result<image::DynamicImage, ServiceError> {
    image::load_from_memory_with_format(bytes, image::ImageFormat::Png).map_err(|_| ServiceError::Failed)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clipboard_png_decodes_to_rgba_and_malformed_data_fails() {
        let rgba = [45, 120, 190, 224];
        let source = arboard::ImageData { width: 1, height: 1, bytes: rgba.as_slice().into() };
        let ClipboardItem::Image { bytes, .. } = encode_image(source).unwrap() else { unreachable!() };
        let image = decode_png(&bytes).unwrap();
        assert_eq!((image.width, image.height), (1, 1));
        assert_eq!(image.bytes.as_ref(), rgba);
        assert!(matches!(decode_png(&bytes[..8]), Err(ServiceError::Failed)));
    }

    #[test]
    #[ignore = "requires an isolated X11 or Wayland data-control session and writes its clipboard"]
    fn linux_clipboard_service_reads_both_formats_after_one_write() {
        use std::time::Duration;

        let (send, receive) = mpsc::channel();
        let mut services = Services::new(Rc::new(Clipboard::new()), move |reply| send.send(reply).unwrap());
        let mut call = |id, request| {
            let id = ServiceId(id);
            let _cleanup = services.request(id, request).unwrap();
            let HostReply::ServiceReply { id: reply_id, outcome } = receive.recv_timeout(Duration::from_secs(10)).unwrap() else {
                panic!("expected a clipboard service reply");
            };
            assert_eq!(reply_id, id);
            outcome
        };

        let text = ClipboardItem::Text("Clipboard text \u{2603}".into());
        let rgba = [45, 120, 190, 224];
        let image = encode_image(arboard::ImageData { width: 1, height: 1, bytes: rgba.as_slice().into() }).unwrap();
        let read = ServiceRequest::ReadClipboard { accept: ClipboardKinds::TEXT | ClipboardKinds::IMAGE };
        let written = call(1, ServiceRequest::WriteClipboard { items: vec![image.clone(), text.clone()] });
        assert_eq!(written, ServiceOutcome::ClipboardWritten);
        let expected = ServiceOutcome::Clipboard(vec![text.clone(), image]);
        assert_eq!(call(2, read.clone()), expected);

        let malformed = ClipboardItem::Image { media_type: "image/png".into(), bytes: vec![] };
        let failed = call(3, ServiceRequest::WriteClipboard { items: vec![text, malformed] });
        assert_eq!(failed, ServiceOutcome::Error(ServiceError::Failed));
        assert_eq!(call(4, read.clone()), expected);

        let replacement = ClipboardItem::Text("Replacement text".into());
        let written = call(5, ServiceRequest::WriteClipboard { items: vec![replacement.clone()] });
        assert_eq!(written, ServiceOutcome::ClipboardWritten);
        assert_eq!(call(6, read), ServiceOutcome::Clipboard(vec![replacement]));
    }
}
