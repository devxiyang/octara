use super::{NativeEvent, Protocol, Shared, input::Seat, window::WindowState};
use crate::os::{FileDropPhase, OsHost, PlatformEvent, PlatformSender, ServiceCleanup, ShellDriver};
use octara::runtime::WindowId;
use octara_model::ask::{ClipboardItem, ClipboardKinds, ServiceError, ServiceId, ServiceOutcome, ServiceRequest};
use octara_model::geometry::Point;
use octara_model::input::{DragItem, HostReply};
use rustix::event::{PollFd, PollFlags, Timespec, poll};
use rustix::fs::{OFlags, fcntl_getfl, fcntl_setfl};
use smithay_client_toolkit::data_device_manager::{ReadPipe, data_offer::receive};
use smithay_client_toolkit::shell::WaylandSurface;
use std::cell::{Cell, RefCell};
use std::fs::File;
use std::io::{ErrorKind, Read, Write};
use std::os::fd::{AsFd, OwnedFd};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use wayland_client::backend::ObjectId;
use wayland_client::protocol::{
    wl_data_device::WlDataDevice, wl_data_device_manager::DndAction, wl_data_offer::WlDataOffer, wl_data_source::WlDataSource,
    wl_seat::WlSeat,
};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};

use super::super::mime::{clipboard_formats, drag_formats, files_from_uri_list};

const URI_LIST: &str = "text/uri-list";
const PNG: &str = "image/png";
const TEXT_TYPES: &[&str] = &["text/plain;charset=utf-8", "UTF8_STRING", "text/plain"];
const TRANSFER_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_TRANSFER: usize = 64 * 1024 * 1024;

type Formats = Vec<(&'static str, Vec<u8>)>;

pub(super) struct DataSeat {
    native: Option<WlDataDevice>,
    unclaimed: Vec<Offer>,
    selection: Option<Offer>,
    local: Option<(Source, Vec<ClipboardItem>)>,
    outgoing: Option<Source>,
    drag: Option<Rc<IncomingDrag>>,
}

impl DataSeat {
    pub(super) fn new(shared: &Shared, seat: &WlSeat) -> Self {
        let native = shared.data_device.as_ref().map(|manager| manager.get_data_device(seat, &shared.queue, seat.id()));
        Self { native, unclaimed: Vec::new(), selection: None, local: None, outgoing: None, drag: None }
    }

    pub(super) fn leave(&mut self, shared: &Shared) {
        if let Some(drag) = self.drag.take() {
            drag.key.emit(shared, FileDropPhase::Left, drag.position.get());
        }
    }

    fn claim(&mut self, offer: WlDataOffer) -> Option<Offer> {
        let index = self.unclaimed.iter().position(|item| item.0 == offer)?;
        Some(self.unclaimed.swap_remove(index))
    }
}

impl Drop for DataSeat {
    fn drop(&mut self) {
        if let Some(native) = &self.native
            && native.version() >= 2
        {
            native.release();
        }
    }
}

struct Source(WlDataSource);

struct SourceData {
    seat: ObjectId,
    formats: Arc<Formats>,
}

impl Source {
    fn new(shared: &Shared, seat: &WlSeat, formats: Formats) -> Option<Self> {
        let manager = shared.data_device.as_ref()?;
        let source = manager.create_data_source(&shared.queue, SourceData { seat: seat.id(), formats: Arc::new(formats) });
        for (mime, _) in source.data::<SourceData>()?.formats.iter() {
            source.offer((*mime).to_owned());
        }
        Some(Self(source))
    }
}

impl Drop for Source {
    fn drop(&mut self) {
        self.0.destroy();
    }
}

struct Offer(WlDataOffer);

#[derive(Default)]
struct OfferData {
    types: Mutex<Vec<String>>,
    action: Mutex<Option<DndAction>>,
}

impl Offer {
    fn mime(&self, requested: &[&str]) -> Option<String> {
        let types = self.0.data::<OfferData>()?.types.lock().ok()?;
        requested.iter().find_map(|mime| types.iter().find(|value| value.eq_ignore_ascii_case(mime)).cloned())
    }

    fn can_finish_drag(&self) -> bool {
        self.0.version() < 3
            || self.0.data::<OfferData>().and_then(|data| data.action.lock().ok().and_then(|action| *action)) == Some(DndAction::Copy)
    }
}

impl Drop for Offer {
    fn drop(&mut self) {
        self.0.destroy();
    }
}

pub(super) struct IncomingDrag {
    key: DragTarget,
    offer: RefCell<Option<Offer>>,
    mime: String,
    serial: u32,
    position: Cell<Point>,
    accepted: Cell<bool>,
}

#[derive(Clone, Copy)]
struct DragTarget(WindowId);

impl DragTarget {
    fn event(self, phase: FileDropPhase, position: Point, paths: Vec<std::path::PathBuf>) -> PlatformEvent {
        PlatformEvent::FileDrop { window: self.0, phase, position: Some(position), paths }
    }

    fn emit(self, shared: &Shared, phase: FileDropPhase, position: Point) {
        shared.emit(self.event(phase, position, Vec::new()));
    }
}

pub(super) enum DragUpdate {
    Target { drag: Rc<IncomingDrag>, position: Point },
    Drop { drag: Rc<IncomingDrag>, position: Point },
}

impl DragUpdate {
    pub(super) fn apply(self, shared: &Shared, host: &mut dyn OsHost, driver: &mut dyn ShellDriver) -> Result<(), String> {
        match self {
            Self::Target { drag, position } => {
                let accepted = driver.event(host, drag.key.event(FileDropPhase::Entered, position, Vec::new()))?;
                if let Some(offer) = drag.offer.borrow().as_ref()
                    && drag.accepted.replace(accepted) != accepted
                {
                    offer.0.accept(drag.serial, accepted.then(|| drag.mime.clone()));
                }
            }
            Self::Drop { drag, position } => {
                let Some(offer) = drag.offer.take().filter(|offer| drag.accepted.get() && offer.can_finish_drag()) else {
                    return Ok(());
                };
                let Ok(pipe) = receive(&offer.0, drag.mime.clone()) else { return Ok(()) };
                let target = drag.key;
                let sender = shared.sender.clone();
                let _ = thread::Builder::new().name("octara-wayland-drop".into()).spawn(move || {
                    let paths = read_transfer(pipe, Instant::now() + TRANSFER_TIMEOUT)
                        .map_or_else(|_| Vec::new(), |bytes| files_from_uri_list(&bytes));
                    if offer.0.version() >= 3 && !paths.is_empty() {
                        offer.0.finish();
                    }
                    drop(offer);
                    // A protocol request made by this worker also needs the UI thread to flush it.
                    if paths.is_empty() {
                        sender.send_event(PlatformEvent::UserWake);
                    } else {
                        sender.send_event(target.event(FileDropPhase::Dropped, position, paths));
                    }
                });
            }
        }
        Ok(())
    }
}

impl Shared {
    pub(super) fn start_drag(&self, origin: &WindowState, items: Vec<DragItem>) -> bool {
        let Ok(formats) = drag_formats(&items) else { return false };
        if self.data_device.is_none() || origin.closed.get() {
            return false;
        }
        let Some((native, serial)) = self.take_grab(origin.id) else { return false };
        let Some(seat) = self.seat(&native.id()) else { return false };
        let mut seat = seat.borrow_mut();
        let Some(device) = seat.data.native.as_ref() else { return false };
        let Some(source) = Source::new(self, &native, formats) else { return false };
        if source.0.version() >= 3 {
            source.0.set_actions(DndAction::Copy);
        }
        device.start_drag(Some(&source.0), origin.native.wl_surface(), None, serial);
        seat.data.outgoing = Some(source);
        true
    }

    fn clipboard_seat(&self, window: Option<WindowId>) -> Option<Rc<RefCell<Seat>>> {
        self.seats
            .borrow()
            .values()
            .filter_map(|seat| {
                let state = seat.borrow();
                state.data.native.as_ref()?;
                let (_, surface) = state.keyboard.focus.as_ref().filter(|(id, _)| window.is_none_or(|requested| requested == *id))?;
                self.window(surface)?;
                let last_input = state.user_serial.as_ref().map(|event| event.time);
                Some(((state.clipboard_serial.is_some(), last_input), seat.clone()))
            })
            .max_by_key(|(priority, _)| *priority)
            .map(|(_, seat)| seat)
    }

    pub(super) fn read_clipboard(&self, accept: ClipboardKinds) -> Result<Vec<ClipboardItem>, ServiceError> {
        let seat = self.clipboard_seat(None).ok_or(ServiceError::Unavailable)?;
        let seat = seat.borrow();
        if let Some((_, items)) = &seat.data.local {
            return Ok(filter_local(items, accept));
        }
        // Remote clipboard sources must be serviced asynchronously by this Wayland connection.
        Err(ServiceError::Unavailable)
    }

    pub(super) fn write_clipboard(&self, items: &[ClipboardItem]) -> Result<(), ServiceError> {
        self.set_selection(None, items)
    }

    fn set_selection(&self, window: Option<WindowId>, items: &[ClipboardItem]) -> Result<(), ServiceError> {
        if self.data_device.is_none() {
            return Err(ServiceError::Unavailable);
        }
        let seat = self.clipboard_seat(window).ok_or(ServiceError::ActivationRequired)?;
        let mut seat = seat.borrow_mut();
        let serial = seat.clipboard_serial.ok_or(ServiceError::ActivationRequired)?;
        let formats = clipboard_formats(items)?;
        let source = Source::new(self, &seat.native, formats).ok_or(ServiceError::Unavailable)?;
        seat.data.native.as_ref().ok_or(ServiceError::Unavailable)?.set_selection(Some(&source.0), serial);
        seat.data.local = Some((source, items.to_vec()));
        Ok(())
    }

    pub(super) fn clipboard_service(
        &self,
        window: WindowId,
        sender: &PlatformSender,
        id: ServiceId,
        request: ServiceRequest,
    ) -> Option<ServiceCleanup> {
        let sender = sender.clone();
        let live = Arc::new(AtomicBool::new(true));
        let send = {
            let live = live.clone();
            move |result: Result<ServiceOutcome, ServiceError>| {
                if live.swap(false, Ordering::AcqRel) {
                    let outcome = result.unwrap_or_else(ServiceOutcome::Error);
                    crate::os::reply(&sender, window, HostReply::ServiceReply { id, outcome });
                }
            }
        };
        match request {
            ServiceRequest::WriteClipboard { items } => {
                send(self.set_selection(Some(window), &items).map(|()| ServiceOutcome::ClipboardWritten))
            }
            ServiceRequest::ReadClipboard { accept } => {
                let result = self.clipboard_pipes(window, accept);
                match result {
                    Ok(ClipboardRead::Local(items)) => send(Ok(ServiceOutcome::Clipboard(items))),
                    Ok(ClipboardRead::Remote(pipes)) => {
                        let report = send.clone();
                        if thread::Builder::new()
                            .name("octara-wayland-clipboard".into())
                            .spawn(move || {
                                send(read_items(pipes).map(ServiceOutcome::Clipboard));
                            })
                            .is_err()
                        {
                            report(Err(ServiceError::Failed));
                        }
                    }
                    Err(error) => send(Err(error)),
                }
            }
        }
        Some(Box::new(move || live.store(false, Ordering::Release)))
    }

    fn clipboard_pipes(&self, window: WindowId, accept: ClipboardKinds) -> Result<ClipboardRead, ServiceError> {
        let seat = self.clipboard_seat(Some(window)).ok_or(ServiceError::Unavailable)?;
        let seat = seat.borrow();
        if let Some((_, items)) = &seat.data.local {
            return Ok(ClipboardRead::Local(filter_local(items, accept)));
        }
        let Some(offer) = &seat.data.selection else { return Ok(ClipboardRead::Local(Vec::new())) };
        let mut pipes = Vec::new();
        for (kind, types) in [(ClipboardKinds::TEXT, TEXT_TYPES), (ClipboardKinds::FILES, &[URI_LIST]), (ClipboardKinds::IMAGE, &[PNG])] {
            if accept.contains(kind)
                && let Some(mime) = offer.mime(types)
            {
                let pipe = receive(&offer.0, mime).map_err(|_| ServiceError::Failed)?;
                pipes.push((kind, pipe));
            }
        }
        Ok(ClipboardRead::Remote(pipes))
    }
}

enum ClipboardRead {
    Local(Vec<ClipboardItem>),
    Remote(Vec<(ClipboardKinds, ReadPipe)>),
}

fn filter_local(items: &[ClipboardItem], accept: ClipboardKinds) -> Vec<ClipboardItem> {
    items
        .iter()
        .filter(|item| {
            accept.contains(match item {
                ClipboardItem::Text(_) => ClipboardKinds::TEXT,
                ClipboardItem::Files(_) => ClipboardKinds::FILES,
                ClipboardItem::Image { .. } => ClipboardKinds::IMAGE,
            })
        })
        .cloned()
        .collect()
}

fn read_items(pipes: Vec<(ClipboardKinds, ReadPipe)>) -> Result<Vec<ClipboardItem>, ServiceError> {
    let deadline = Instant::now() + TRANSFER_TIMEOUT;
    let mut items = Vec::new();
    let mut failed = false;
    for (kind, pipe) in pipes {
        let bytes = match read_transfer(pipe, deadline) {
            Ok(bytes) => bytes,
            Err(_) => {
                failed = true;
                continue;
            }
        };
        if kind == ClipboardKinds::TEXT {
            match String::from_utf8(bytes) {
                Ok(text) => items.push(ClipboardItem::Text(text)),
                Err(_) => failed = true,
            }
        } else if kind == ClipboardKinds::FILES {
            let files = files_from_uri_list(&bytes);
            if !files.is_empty() {
                items.push(ClipboardItem::Files(files));
            }
        } else if !bytes.is_empty() {
            items.push(ClipboardItem::Image { media_type: PNG.into(), bytes });
        }
    }
    if items.is_empty() && failed { Err(ServiceError::Failed) } else { Ok(items) }
}

impl Dispatch<WlDataDevice, ObjectId> for Protocol {
    fn event(
        state: &mut Self,
        _: &WlDataDevice,
        event: <WlDataDevice as Proxy>::Event,
        data: &ObjectId,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use wayland_client::protocol::wl_data_device::Event::*;

        let Some(seat) = state.shared.seat(data) else { return };
        let mut seat = seat.borrow_mut();
        if matches!(event, Selection { .. }) {
            seat.clipboard_serial = None;
        }
        let seat = &mut seat.data;
        match event {
            DataOffer { id } => seat.unclaimed.push(Offer(id)),
            Selection { id } => seat.selection = id.and_then(|offer| seat.claim(offer)),
            Enter { serial, surface, x, y, id } => {
                seat.leave(&state.shared);
                let Some(offer) = id.and_then(|offer| seat.claim(offer)) else { return };
                let Some(window) = state.shared.window(&surface) else { return };
                let Some(mime) = offer.mime(&[URI_LIST]) else { return };
                if offer.0.version() >= 3 {
                    offer.0.set_actions(DndAction::Copy, DndAction::Copy);
                }
                let position = Point::new(x as f32, y as f32);
                let drag = Rc::new(IncomingDrag {
                    key: DragTarget(window.id),
                    offer: RefCell::new(Some(offer)),
                    mime,
                    serial,
                    position: Cell::new(position),
                    accepted: Cell::new(false),
                });
                seat.drag = Some(drag.clone());
                state.shared.events.borrow_mut().push_back(NativeEvent::Drag(DragUpdate::Target { drag, position }));
            }
            Motion { x, y, .. } => {
                if let Some(drag) = &seat.drag {
                    let position = Point::new(x as f32, y as f32);
                    drag.position.set(position);
                    state.shared.events.borrow_mut().push_back(NativeEvent::Drag(DragUpdate::Target { drag: drag.clone(), position }));
                }
            }
            Leave => seat.leave(&state.shared),
            Drop => {
                if let Some(drag) = seat.drag.take() {
                    let position = drag.position.get();
                    state.shared.events.borrow_mut().push_back(NativeEvent::Drag(DragUpdate::Drop { drag, position }));
                }
            }
            _ => {}
        }
    }

    wayland_client::event_created_child!(Protocol, WlDataDevice, [0 => (WlDataOffer, OfferData::default())]);
}

impl Dispatch<WlDataOffer, OfferData> for Protocol {
    fn event(_: &mut Self, _: &WlDataOffer, event: <WlDataOffer as Proxy>::Event, data: &OfferData, _: &Connection, _: &QueueHandle<Self>) {
        use wayland_client::protocol::wl_data_offer::Event::*;

        match event {
            Offer { mime_type } => data.types.lock().expect("Wayland MIME types").push(mime_type),
            Action { dnd_action } => *data.action.lock().expect("Wayland drop action") = dnd_action.into_result().ok(),
            _ => {}
        }
    }
}

impl Dispatch<WlDataSource, SourceData> for Protocol {
    fn event(
        state: &mut Self,
        source: &WlDataSource,
        event: <WlDataSource as Proxy>::Event,
        data: &SourceData,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use wayland_client::protocol::wl_data_source::Event::*;

        match event {
            Send { mime_type, fd } => {
                if let Some(index) = data.formats.iter().position(|(mime, _)| *mime == mime_type) {
                    let formats = data.formats.clone();
                    let _ = thread::Builder::new().name("octara-wayland-data-source".into()).spawn(move || {
                        let _ = write_transfer(fd, &formats[index].1);
                    });
                }
            }
            Cancelled | DndFinished => {
                if let Some(seat) = state.shared.seat(&data.seat) {
                    let mut seat = seat.borrow_mut();
                    if seat.data.local.as_ref().is_some_and(|(local, _)| &local.0 == source) {
                        seat.data.local = None;
                        seat.clipboard_serial = None;
                    }
                    if seat.data.outgoing.as_ref().is_some_and(|drag| &drag.0 == source) {
                        seat.data.outgoing = None;
                    }
                }
            }
            _ => {}
        }
    }
}

fn wait_for(fd: &impl AsFd, flags: PollFlags, deadline: Instant) -> std::io::Result<()> {
    loop {
        let timeout = deadline.saturating_duration_since(Instant::now());
        if timeout.is_zero() {
            return Err(ErrorKind::TimedOut.into());
        }
        let timeout = Timespec { tv_sec: timeout.as_secs() as i64, tv_nsec: i64::from(timeout.subsec_nanos()) };
        match poll(&mut [PollFd::new(fd, flags)], Some(&timeout)) {
            Ok(0) => return Err(ErrorKind::TimedOut.into()),
            Ok(_) => return Ok(()),
            Err(rustix::io::Errno::INTR) => {}
            Err(error) => return Err(error.into()),
        }
    }
}

fn read_transfer(mut pipe: ReadPipe, deadline: Instant) -> std::io::Result<Vec<u8>> {
    fcntl_setfl(&pipe, fcntl_getfl(&pipe)? | OFlags::NONBLOCK)?;
    let mut bytes = Vec::new();
    let mut buffer = [0; 8192];
    loop {
        wait_for(&pipe, PollFlags::IN, deadline)?;
        match pipe.read(&mut buffer) {
            Ok(0) => return Ok(bytes),
            Ok(count) if bytes.len() + count <= MAX_TRANSFER => bytes.extend_from_slice(&buffer[..count]),
            Ok(_) => return Err(ErrorKind::FileTooLarge.into()),
            Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::Interrupted) => {}
            Err(error) => return Err(error),
        }
    }
}

fn write_transfer(fd: OwnedFd, mut bytes: &[u8]) -> std::io::Result<()> {
    fcntl_setfl(&fd, fcntl_getfl(&fd)? | OFlags::NONBLOCK)?;
    let mut file = File::from(fd);
    let deadline = Instant::now() + TRANSFER_TIMEOUT;
    while !bytes.is_empty() {
        wait_for(&file, PollFlags::OUT, deadline)?;
        match file.write(bytes) {
            Ok(0) => return Err(ErrorKind::WriteZero.into()),
            Ok(count) => bytes = &bytes[count..],
            Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::Interrupted) => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{TRANSFER_TIMEOUT, read_transfer, write_transfer};
    use smithay_client_toolkit::data_device_manager::ReadPipe;
    use std::os::fd::OwnedFd;
    use std::os::unix::net::UnixStream;
    use std::time::Instant;

    #[test]
    fn transfer_drains_payload_larger_than_the_socket_buffer() {
        let (read, write) = UnixStream::pair().unwrap();
        let read = ReadPipe::from(OwnedFd::from(read));
        let bytes = vec![0x7c; 512 * 1024];
        std::thread::scope(|scope| {
            scope.spawn(|| write_transfer(write.into(), &bytes).unwrap());
            assert_eq!(read_transfer(read, Instant::now() + TRANSFER_TIMEOUT).unwrap(), bytes);
        });
    }
}
