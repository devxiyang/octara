use super::XResult;
use crate::os::ServiceCleanup;
use crate::os::linux::mime::{PNG, TEXT, URIS, UTF8, clipboard_formats, files_from_uri_list};
use calloop::{EventLoop, Interest, Mode, PostAction, channel, generic::Generic};
use octara_model::ask::{ClipboardItem, ClipboardKinds, ServiceError, ServiceId, ServiceOutcome, ServiceRequest};
use octara_model::input::HostReply;
use rustix::event::{PollFd, PollFlags, Timespec, poll};
use std::collections::HashMap;
use std::os::fd::AsFd;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};
use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ChangeWindowAttributesAux, ConnectionExt as _, CreateWindowAux, EventMask, GetPropertyReply, PropMode, Property,
    SelectionNotifyEvent, SelectionRequestEvent, Timestamp, Window, WindowClass,
};
use x11rb::wrapper::ConnectionExt as _;
use x11rb::xcb_ffi::XCBConnection;
use x11rb::{CURRENT_TIME, NONE};

const TRANSFER_TIMEOUT: Duration = Duration::from_secs(5);
const HANDOVER_TIMEOUT: Duration = Duration::from_secs(1);
const MAX_TRANSFER: usize = 64 * 1024 * 1024;

x11rb::atom_manager! {
    Atoms: AtomsCookie {
        CLIPBOARD, CLIPBOARD_MANAGER, SAVE_TARGETS, TARGETS, TIMESTAMP, MULTIPLE, ATOM_PAIR, INCR, UTF8_STRING, TEXT,
        OCTARA_SELECTION, OCTARA_TIMESTAMP,
        PLAIN: b"text/plain",
        PLAIN_UTF8: b"text/plain;charset=utf-8",
        URI_LIST: b"text/uri-list",
        PNG: b"image/png",
    }
}

type Job = Box<dyn FnOnce(&mut Selection) + Send>;

pub(super) struct Clipboard {
    worker: Option<channel::Sender<Job>>,
}

impl Clipboard {
    pub(super) fn new() -> Self {
        Self { worker: start_worker().ok() }
    }

    pub(super) fn request(
        &self,
        id: ServiceId,
        request: ServiceRequest,
        reply: impl FnOnce(HostReply) + Send + 'static,
    ) -> Option<ServiceCleanup> {
        let worker = self.worker.as_ref()?;
        let live = Arc::new(AtomicBool::new(true));
        let active = live.clone();
        let job: Job = Box::new(move |selection| {
            if !active.load(Ordering::Acquire) {
                return;
            }
            let result = match request {
                ServiceRequest::ReadClipboard { accept } => selection.read(accept).map(ServiceOutcome::Clipboard),
                ServiceRequest::WriteClipboard { items } => selection.write(&items).map(|()| ServiceOutcome::ClipboardWritten),
            };
            if active.swap(false, Ordering::AcqRel) {
                reply(HostReply::ServiceReply { id, outcome: result.unwrap_or_else(ServiceOutcome::Error) });
            }
        });
        worker.send(job).ok()?;
        Some(Box::new(move || live.store(false, Ordering::Release)))
    }

    fn call<T: Send + 'static>(
        &self,
        job: impl FnOnce(&mut Selection) -> Result<T, ServiceError> + Send + 'static,
    ) -> Result<T, ServiceError> {
        let worker = self.worker.as_ref().ok_or(ServiceError::Unavailable)?;
        let (send, receive) = mpsc::sync_channel(1);
        worker
            .send(Box::new(move |selection| {
                let _ = send.send(job(selection));
            }))
            .map_err(|_| ServiceError::Unavailable)?;
        receive.recv().map_err(|_| ServiceError::Unavailable)?
    }

    pub(super) fn read(&self, accept: ClipboardKinds) -> Result<Vec<ClipboardItem>, ServiceError> {
        self.call(move |selection| selection.read(accept))
    }

    pub(super) fn write(&self, items: Vec<ClipboardItem>) -> Result<(), ServiceError> {
        self.call(move |selection| selection.write(&items))
    }

    pub(super) fn persist(&self) {
        let Some(worker) = &self.worker else { return };
        let (send, receive) = mpsc::sync_channel(1);
        let deadline = Instant::now() + HANDOVER_TIMEOUT;
        if worker
            .send(Box::new(move |selection| {
                let _ = selection.persist(deadline);
                let _ = send.send(());
            }))
            .is_ok()
        {
            let _ = receive.recv_timeout(HANDOVER_TIMEOUT);
        }
    }
}

fn start_worker() -> XResult<channel::Sender<Job>> {
    let selection = Selection::new()?;
    let socket = selection.connection.as_fd().try_clone_to_owned()?;
    let (send, jobs) = channel::channel::<Job>();
    let (ready, started) = mpsc::sync_channel(1);
    std::thread::Builder::new().name("octara-x11-clipboard".into()).spawn(move || {
        let Ok(mut events) = EventLoop::<Worker>::try_new() else { return };
        let handle = events.handle();
        // X11 replies may buffer events even when the socket no longer polls readable.
        // Always drain the connection after servicing jobs that make synchronous X requests.
        if handle.insert_source(Generic::new(socket, Interest::READ, Mode::Level), |_, _, _| Ok(PostAction::Continue)).is_err()
            || handle
                .insert_source(jobs, |event, (), worker| match event {
                    channel::Event::Msg(job) => job(&mut worker.selection),
                    channel::Event::Closed => worker.closed = true,
                })
                .is_err()
        {
            return;
        }
        let _ = ready.send(());
        let mut worker = Worker { closed: false, selection };
        while !worker.closed {
            let Ok(timeout) = worker.selection.service_events() else { break };
            if events.dispatch(timeout, &mut worker).is_err() {
                break;
            }
        }
    })?;
    started.recv()?;
    Ok(send)
}

struct Worker {
    closed: bool,
    selection: Selection,
}

#[derive(Clone)]
struct Payload {
    type_: Atom,
    data: Arc<[u8]>,
}

struct Owned {
    timestamp: Timestamp,
    targets: Vec<(Atom, Payload)>,
}

struct Outgoing {
    payload: Payload,
    offset: usize,
    deadline: Instant,
}

struct Selection {
    connection: XCBConnection,
    root: Window,
    owner: Window,
    atoms: Atoms,
    chunk_size: usize,
    owned: Option<Owned>,
    outgoing: HashMap<(Window, Atom), Outgoing>,
}

impl Selection {
    fn new() -> XResult<Self> {
        let (connection, screen) = XCBConnection::connect(None)?;
        let root = connection.setup().roots[screen].root;
        let owner = selection_window(&connection, root)?;
        let atoms = Atoms::new(&connection)?.reply()?;
        // Use the setup limit so transfers interoperate without relying on BIG-REQUESTS.
        let chunk_size = usize::from(connection.setup().maximum_request_length) * 4 - 24;
        Ok(Self { connection, root, owner, atoms, chunk_size, owned: None, outgoing: HashMap::new() })
    }

    fn write(&mut self, items: &[ClipboardItem]) -> Result<(), ServiceError> {
        let formats = clipboard_formats(items)?;
        let mut targets = Vec::new();
        for (mime, data) in formats {
            let target = match mime {
                PNG => self.atoms.PNG,
                URIS => self.atoms.URI_LIST,
                TEXT => self.atoms.PLAIN,
                UTF8 => self.atoms.PLAIN_UTF8,
                _ => continue,
            };
            let data: Arc<[u8]> = data.into();
            targets.push((target, Payload { type_: target, data: data.clone() }));
            if target == self.atoms.PLAIN_UTF8 {
                let utf8 = Payload { type_: self.atoms.UTF8_STRING, data: data.clone() };
                targets.push((self.atoms.UTF8_STRING, utf8.clone()));
                targets.push((self.atoms.TEXT, utf8));
                let latin1: Option<Vec<_>> =
                    std::str::from_utf8(&data).expect("UTF-8 text offer").chars().map(|ch| u8::try_from(ch).ok()).collect();
                if let Some(bytes) = latin1 {
                    targets.push((AtomEnum::STRING.into(), Payload { type_: AtomEnum::STRING.into(), data: bytes.into() }));
                }
            }
        }
        self.claim(targets).map_err(|_| ServiceError::Failed)
    }

    fn claim(&mut self, targets: Vec<(Atom, Payload)>) -> XResult<()> {
        let deadline = Instant::now() + TRANSFER_TIMEOUT;
        let timestamp = self.timestamp(deadline)?;
        self.connection.set_selection_owner(self.owner, self.atoms.CLIPBOARD, timestamp)?.check()?;
        if self.connection.get_selection_owner(self.atoms.CLIPBOARD)?.reply()?.owner != self.owner {
            return Err("X11 clipboard ownership was refused".into());
        }
        self.owned = Some(Owned { timestamp, targets });
        Ok(())
    }

    fn timestamp(&mut self, deadline: Instant) -> XResult<Timestamp> {
        let window = self.owner;
        let atom = self.atoms.OCTARA_TIMESTAMP;
        self.connection.change_property8(PropMode::REPLACE, window, atom, AtomEnum::INTEGER, &[])?.check()?;
        self.wait_for(deadline, |event| match event {
            Event::PropertyNotify(event) if event.window == window && event.atom == atom && event.state == Property::NEW_VALUE => {
                Some(event.time)
            }
            _ => None,
        })
    }

    fn persist(&mut self, deadline: Instant) -> XResult<()> {
        let atoms = self.atoms;
        if Instant::now() >= deadline
            || self.owned.is_none()
            || self.connection.get_selection_owner(atoms.CLIPBOARD)?.reply()?.owner != self.owner
            || self.connection.get_selection_owner(atoms.CLIPBOARD_MANAGER)?.reply()?.owner == NONE
        {
            return Ok(());
        }
        let requestor = self.owner;
        let time = self.timestamp(deadline)?;
        // The manager retrieves all formats before confirming the transfer.
        self.connection.convert_selection(requestor, atoms.CLIPBOARD_MANAGER, atoms.SAVE_TARGETS, atoms.OCTARA_SELECTION, time)?.check()?;
        self.wait_for(deadline, |event| match event {
            Event::SelectionNotify(event)
                if event.requestor == requestor && event.selection == atoms.CLIPBOARD_MANAGER && event.target == atoms.SAVE_TARGETS =>
            {
                Some(())
            }
            _ => None,
        })
    }

    fn read(&mut self, accept: ClipboardKinds) -> Result<Vec<ClipboardItem>, ServiceError> {
        self.read_selection(accept).map_err(|_| ServiceError::Failed)
    }

    fn read_selection(&mut self, accept: ClipboardKinds) -> XResult<Vec<ClipboardItem>> {
        let owner = self.connection.get_selection_owner(self.atoms.CLIPBOARD)?.reply()?.owner;
        if owner == NONE || accept.is_empty() {
            return Ok(Vec::new());
        }
        if owner == self.owner {
            return read_formats(self.atoms, accept, |target| {
                let Some(payload) = self.owned.as_ref().and_then(|owned| owned.targets.iter().find(|(atom, _)| *atom == target)) else {
                    return Ok(None);
                };
                Ok(Some((payload.1.type_, payload.1.data.to_vec())))
            });
        }

        // A new requestor for each read prevents a delayed reply from satisfying a later request.
        let window = selection_window(&self.connection, self.root)?;
        let deadline = Instant::now() + TRANSFER_TIMEOUT;
        let result = self.read_remote(window, accept, deadline);
        self.connection.destroy_window(window)?.check()?;
        if self.connection.get_selection_owner(self.atoms.CLIPBOARD)?.reply()?.owner != owner {
            return Err("X11 clipboard changed while reading".into());
        }
        result
    }

    fn read_remote(&mut self, window: Window, accept: ClipboardKinds, deadline: Instant) -> XResult<Vec<ClipboardItem>> {
        let atoms = self.atoms;
        let offered =
            self.convert(window, atoms.TARGETS, deadline)?.filter(|data| data.type_ == Atom::from(AtomEnum::ATOM) && data.format == 32);
        read_formats(atoms, accept, |target| {
            if offered.as_ref().is_some_and(|data| !data.atoms().any(|atom| atom == target)) {
                return Ok(None);
            }
            let data = self.convert(window, target, deadline)?;
            Ok(data.filter(|data| data.format == 8).map(|data| (data.type_, data.bytes)))
        })
    }

    fn convert(&mut self, window: Window, target: Atom, deadline: Instant) -> XResult<Option<Received>> {
        let property = self.atoms.OCTARA_SELECTION;
        let selection = self.atoms.CLIPBOARD;
        self.connection.convert_selection(window, selection, target, property, CURRENT_TIME)?.check()?;
        let notify = self.wait_for(deadline, |event| match event {
            Event::SelectionNotify(event)
                if event.requestor == window
                    && event.selection == selection
                    && event.target == target
                    && (event.property == property || event.property == NONE) =>
            {
                Some(event.property)
            }
            _ => None,
        })?;
        if notify == NONE {
            return Ok(None);
        }

        let initial = self.get_property(window, property, MAX_TRANSFER)?;
        if initial.type_ != self.atoms.INCR {
            return Ok(Some(Received { type_: initial.type_, format: initial.format, bytes: initial.value }));
        }

        if initial.format != 32 || initial.value32().and_then(|mut values| values.next()).is_none_or(|min| min as usize > MAX_TRANSFER) {
            return Err("Invalid X11 clipboard INCR header".into());
        }
        let mut received = None::<Received>;
        loop {
            self.wait_for(deadline, |event| match event {
                Event::PropertyNotify(event) if event.window == window && event.atom == property && event.state == Property::NEW_VALUE => {
                    Some(())
                }
                _ => None,
            })?;
            let remaining = MAX_TRANSFER - received.as_ref().map_or(0, |data| data.bytes.len());
            let chunk = self.get_property(window, property, remaining)?;
            let data = received.get_or_insert_with(|| Received { type_: chunk.type_, format: chunk.format, bytes: Vec::new() });
            if (chunk.type_, chunk.format) != (data.type_, data.format) || chunk.format == 0 || chunk.type_ == self.atoms.INCR {
                return Err("Invalid X11 clipboard INCR chunk".into());
            }
            if chunk.value.is_empty() {
                return Ok(received);
            }
            data.bytes.extend(chunk.value);
        }
    }

    fn get_property(&self, window: Window, property: Atom, max_bytes: usize) -> XResult<GetPropertyReply> {
        let data = self.connection.get_property(true, window, property, AtomEnum::ANY, 0, max_bytes.div_ceil(4) as u32)?.reply()?;
        if data.bytes_after != 0 || data.value.len() > max_bytes || data.type_ == NONE {
            return Err("Invalid or oversized X11 clipboard property".into());
        }
        Ok(data)
    }

    fn wait_for<T>(&mut self, deadline: Instant, find: impl Fn(&Event) -> Option<T>) -> XResult<T> {
        self.connection.flush()?;
        while Instant::now() < deadline {
            if let Some(event) = self.connection.poll_for_event()? {
                if let Some(value) = find(&event) {
                    return Ok(value);
                }
                self.dispatch(event);
                continue;
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            let timeout = Timespec { tv_sec: remaining.as_secs() as i64, tv_nsec: remaining.subsec_nanos().into() };
            let mut fds = [PollFd::new(&self.connection, PollFlags::IN)];
            match poll(&mut fds, Some(&timeout)) {
                Ok(_) | Err(rustix::io::Errno::INTR) => {}
                Err(error) => return Err(error.into()),
            }
        }
        Err("X11 clipboard transfer timed out".into())
    }

    fn service_events(&mut self) -> XResult<Option<Duration>> {
        const EVENT_BATCH: usize = 256;
        let mut drained = false;
        for _ in 0..EVENT_BATCH {
            let Some(event) = self.connection.poll_for_event()? else {
                drained = true;
                break;
            };
            self.dispatch(event);
        }
        let now = Instant::now();
        let mut expired = Vec::new();
        self.outgoing.retain(|(window, _), transfer| {
            if transfer.deadline > now {
                return true;
            }
            expired.push(*window);
            false
        });
        for window in expired {
            self.unwatch_idle(window);
        }
        let next_transfer = self.outgoing.values().map(|transfer| transfer.deadline.saturating_duration_since(now)).min();
        Ok(if drained { next_transfer } else { Some(Duration::ZERO) })
    }

    fn dispatch(&mut self, event: Event) {
        match event {
            Event::SelectionRequest(request) if request.owner == self.owner && request.selection == self.atoms.CLIPBOARD => {
                self.serve(request);
            }
            Event::SelectionClear(event) if event.owner == self.owner && event.selection == self.atoms.CLIPBOARD => {
                if self.owned.as_ref().is_some_and(|owned| !older(event.time, owned.timestamp)) {
                    self.owned = None;
                }
            }
            Event::PropertyNotify(event) if event.state == Property::DELETE => self.send_chunk((event.window, event.atom)),
            Event::DestroyNotify(event) => self.outgoing.retain(|(window, _), _| *window != event.window),
            _ => {}
        }
    }

    fn serve(&mut self, request: SelectionRequestEvent) {
        let property = if request.property == NONE { request.target } else { request.property };
        let result = if self.owned.as_ref().is_some_and(|owned| request.time == CURRENT_TIME || !older(request.time, owned.timestamp)) {
            if request.target == self.atoms.MULTIPLE && request.property != NONE {
                self.multiple(request.requestor, property)
            } else {
                self.serve_target(request.requestor, request.target, property)
            }
        } else {
            Err("X11 clipboard request outside its ownership interval".into())
        };
        let notify = SelectionNotifyEvent {
            response_type: x11rb::protocol::xproto::SELECTION_NOTIFY_EVENT,
            sequence: 0,
            time: request.time,
            requestor: request.requestor,
            selection: request.selection,
            target: request.target,
            property: if result.is_ok() { property } else { NONE },
        };
        let _ = self.connection.send_event(false, request.requestor, EventMask::NO_EVENT, notify);
        let _ = self.connection.flush();
    }

    fn multiple(&mut self, window: Window, property: Atom) -> XResult<()> {
        let list = self.connection.get_property(false, window, property, self.atoms.ATOM_PAIR, 0, 4096)?.reply()?;
        let Some(mut pairs) = list.value32().filter(|_| list.type_ == self.atoms.ATOM_PAIR && list.bytes_after == 0).map(Vec::from_iter)
        else {
            return Err("Invalid X11 clipboard MULTIPLE request".into());
        };
        let (conversions, remainder) = pairs.as_chunks_mut::<2>();
        if !remainder.is_empty() {
            return Err("Invalid X11 clipboard MULTIPLE pairs".into());
        }
        for [target, destination] in conversions {
            if *destination == property || self.serve_target(window, *target, *destination).is_err() {
                *destination = NONE;
            }
        }
        self.connection.change_property32(PropMode::REPLACE, window, property, self.atoms.ATOM_PAIR, &pairs)?.check()?;
        Ok(())
    }

    fn serve_target(&mut self, window: Window, target: Atom, property: Atom) -> XResult<()> {
        let owned = self.owned.as_ref().ok_or("No X11 clipboard ownership")?;
        if property == NONE || self.outgoing.contains_key(&(window, property)) {
            return Err("Unavailable X11 clipboard request property".into());
        }
        if target == self.atoms.TARGETS {
            let targets: Vec<_> = [self.atoms.TARGETS, self.atoms.TIMESTAMP, self.atoms.MULTIPLE]
                .into_iter()
                .chain(owned.targets.iter().map(|(atom, _)| *atom))
                .collect();
            self.connection.change_property32(PropMode::REPLACE, window, property, AtomEnum::ATOM, &targets)?.check()?;
        } else if target == self.atoms.TIMESTAMP {
            self.connection.change_property32(PropMode::REPLACE, window, property, AtomEnum::INTEGER, &[owned.timestamp])?.check()?;
        } else {
            let payload = &owned.targets.iter().find(|(atom, _)| *atom == target).ok_or("Unavailable X11 clipboard target")?.1;
            if payload.data.len() <= self.chunk_size {
                self.connection.change_property8(PropMode::REPLACE, window, property, payload.type_, &payload.data)?.check()?;
            } else {
                let payload = payload.clone();
                if window != self.owner && !self.outgoing.keys().any(|(active, _)| *active == window) {
                    self.watch(window, EventMask::PROPERTY_CHANGE | EventMask::STRUCTURE_NOTIFY)?;
                }
                let size = u32::try_from(payload.data.len()).unwrap_or(u32::MAX);
                if let Err(error) = self
                    .connection
                    .change_property32(PropMode::REPLACE, window, property, self.atoms.INCR, &[size])
                    .map_err(Into::into)
                    .and_then(|cookie| cookie.check().map_err(Into::into))
                {
                    self.unwatch_idle(window);
                    return Err(error);
                }
                self.outgoing.insert((window, property), Outgoing { payload, offset: 0, deadline: Instant::now() + TRANSFER_TIMEOUT });
            }
        }
        Ok(())
    }

    fn watch(&self, window: Window, mask: EventMask) -> XResult<()> {
        self.connection.change_window_attributes(window, &ChangeWindowAttributesAux::new().event_mask(mask))?.check()?;
        Ok(())
    }

    fn unwatch_idle(&self, window: Window) {
        if window != self.owner && !self.outgoing.keys().any(|(active, _)| *active == window) {
            let _ = self.watch(window, EventMask::NO_EVENT);
        }
    }

    fn send_chunk(&mut self, key @ (window, property): (Window, Atom)) {
        let Some(transfer) = self.outgoing.get_mut(&key) else { return };
        let end = transfer.offset.saturating_add(self.chunk_size).min(transfer.payload.data.len());
        let data = &transfer.payload.data[transfer.offset..end];
        let stored = self
            .connection
            .change_property8(PropMode::APPEND, window, property, transfer.payload.type_, data)
            .is_ok_and(|cookie| cookie.check().is_ok());
        if stored && !data.is_empty() {
            transfer.offset = end;
            transfer.deadline = Instant::now() + TRANSFER_TIMEOUT;
        } else {
            self.outgoing.remove(&key);
            self.unwatch_idle(window);
        }
    }
}

fn older(time: Timestamp, reference: Timestamp) -> bool {
    time.wrapping_sub(reference) >= 1 << 31
}

fn selection_window(connection: &XCBConnection, root: Window) -> XResult<Window> {
    let window = connection.generate_id()?;
    connection
        .create_window(
            x11rb::COPY_DEPTH_FROM_PARENT,
            window,
            root,
            0,
            0,
            1,
            1,
            0,
            WindowClass::INPUT_OUTPUT,
            0,
            &CreateWindowAux::new().event_mask(EventMask::PROPERTY_CHANGE),
        )?
        .check()?;
    Ok(window)
}

struct Received {
    type_: Atom,
    format: u8,
    bytes: Vec<u8>,
}

impl Received {
    fn atoms(&self) -> impl Iterator<Item = Atom> + '_ {
        self.bytes.as_chunks::<4>().0.iter().copied().map(Atom::from_ne_bytes)
    }
}

fn read_formats(
    atoms: Atoms,
    accept: ClipboardKinds,
    mut get: impl FnMut(Atom) -> XResult<Option<(Atom, Vec<u8>)>>,
) -> XResult<Vec<ClipboardItem>> {
    let mut items = Vec::new();
    let mut failure = None;
    let mut load = |targets: &[Atom], decode: fn(Atom, Vec<u8>) -> XResult<Option<ClipboardItem>>| {
        for &target in targets {
            match get(target).and_then(|data| data.map_or(Ok(None), |(type_, bytes)| decode(type_, bytes))) {
                Ok(Some(item)) => {
                    items.push(item);
                    return;
                }
                Ok(None) => {}
                Err(error) => failure = Some(error),
            }
        }
    };
    if accept.contains(ClipboardKinds::TEXT) {
        load(&[atoms.UTF8_STRING, atoms.PLAIN_UTF8, atoms.PLAIN, atoms.TEXT, AtomEnum::STRING.into()], |type_, bytes| {
            let text = if type_ == Atom::from(AtomEnum::STRING) {
                bytes.into_iter().map(char::from).collect::<String>()
            } else {
                String::from_utf8(bytes)?
            };
            Ok(Some(ClipboardItem::Text(text)))
        });
    }
    if accept.contains(ClipboardKinds::FILES) {
        load(&[atoms.URI_LIST], |_, bytes| {
            let paths = files_from_uri_list(&bytes);
            Ok((!paths.is_empty()).then_some(ClipboardItem::Files(paths)))
        });
    }
    if accept.contains(ClipboardKinds::IMAGE) {
        load(&[atoms.PNG], |_, bytes| Ok((!bytes.is_empty()).then_some(ClipboardItem::Image { media_type: PNG.into(), bytes })));
    }
    if items.is_empty()
        && let Some(error) = failure
    {
        return Err(error);
    }
    Ok(items)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{
        ExtendedColorType, ImageEncoder,
        codecs::png::{CompressionType, FilterType, PngEncoder},
    };

    #[test]
    #[ignore = "requires an isolated X11 display and replaces its clipboard"]
    fn x11_clipboard_transfers_text_files_and_png_between_clients() {
        let owner = Clipboard::new();
        let requestor = Clipboard::new();
        let threshold = owner.call(|selection| Ok(selection.chunk_size)).unwrap();
        let text = ClipboardItem::Text("Clipboard \u{2603}\n".repeat(threshold / 10 + 1));
        let files = ClipboardItem::Files(vec!["/tmp/clipboard #1".into()]);
        let mut png = Vec::new();
        PngEncoder::new_with_quality(&mut png, CompressionType::Uncompressed, FilterType::NoFilter)
            .write_image(&vec![128; 512 * 512 * 4], 512, 512, ExtendedColorType::Rgba8)
            .unwrap();
        assert!(png.len() > threshold);
        let image = ClipboardItem::Image { media_type: PNG.into(), bytes: png };
        let kinds = ClipboardKinds::TEXT | ClipboardKinds::FILES | ClipboardKinds::IMAGE;
        owner.write(vec![image.clone(), files.clone(), text.clone()]).unwrap();

        let (send, receive) = mpsc::sync_channel(1);
        let id = ServiceId(1);
        let _cleanup =
            requestor.request(id, ServiceRequest::ReadClipboard { accept: kinds }, move |reply| send.send(reply).unwrap()).unwrap();
        let reply = receive.recv_timeout(TRANSFER_TIMEOUT + Duration::from_secs(1)).unwrap();
        let HostReply::ServiceReply { id: actual, outcome } = reply else { panic!("expected clipboard service reply") };
        assert_eq!(actual, id);
        assert_eq!(outcome, ServiceOutcome::Clipboard(vec![text, files, image]));

        let invalid = ClipboardItem::Image { media_type: PNG.into(), bytes: vec![0; 8] };
        assert_eq!(owner.write(vec![invalid]), Err(ServiceError::Failed));
        assert_eq!(requestor.read(kinds).unwrap().len(), 3);

        let replacement = ClipboardItem::Text("Replacement".into());
        requestor.write(vec![replacement.clone()]).unwrap();
        assert_eq!(owner.read(kinds).unwrap(), vec![replacement]);
    }
}
