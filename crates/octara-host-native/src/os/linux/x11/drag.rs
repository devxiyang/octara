use super::{Native, XResult, input, window::WindowState};
use crate::os::linux::mime::{drag_formats, files_from_uri_list};
use crate::os::{FileDropPhase, PlatformEvent};
use octara_model::geometry::Point;
use octara_model::input::DragItem;
use std::collections::HashSet;
use std::time::{Duration, Instant};
use x11rb::connection::RequestConnection;
use x11rb::errors::ReplyError;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ChangeWindowAttributesAux, ClientMessageEvent, ConnectionExt, EventMask, GrabMode, GrabStatus, KeyButMask, PropMode,
    Property, PropertyNotifyEvent, SELECTION_NOTIFY_EVENT, SelectionNotifyEvent, SelectionRequestEvent, Timestamp, Window,
};
use x11rb::wrapper::ConnectionExt as _;

pub(super) const VERSION: u32 = 5;
const MIN_VERSION: u32 = 3;
const TRANSFER_LIMIT: usize = 64 * 1024 * 1024;
const STATUS_TIMEOUT: Duration = Duration::from_secs(2);
const TRANSFER_TIMEOUT: Duration = Duration::from_secs(10);
fn grab_events() -> EventMask {
    EventMask::BUTTON_PRESS | EventMask::BUTTON_RELEASE | EventMask::POINTER_MOTION
}

pub(super) struct DragTarget {
    source: Window,
    version: u32,
    offers_files: bool,
    position: Option<Point>,
    accepted: bool,
    transfer: Option<IncomingTransfer>,
}

struct IncomingTransfer {
    deadline: Instant,
    time: Timestamp,
    incremental: Option<Vec<u8>>,
}

impl DragTarget {
    fn entered(native: &Native, [source, flags, a, b, c]: [u32; 5]) -> Option<Self> {
        let version = flags >> 24;
        if !(MIN_VERSION..=VERSION).contains(&version) || source == x11rb::NONE {
            return None;
        }
        let types = if flags & 1 == 1 {
            native.property32(source, native.atoms.XdndTypeList, AtomEnum::ATOM, 256).unwrap_or_default()
        } else {
            vec![a, b, c]
        };
        Some(Self {
            source,
            version,
            offers_files: types.contains(&native.atoms.URI_LIST),
            position: None,
            accepted: false,
            transfer: None,
        })
    }

    fn finish(&self, native: &Native, window: Window, accepted: bool) {
        let success = self.version >= 5 && accepted;
        let action = if success { native.atoms.XdndActionCopy } else { x11rb::NONE };
        send(native, self.source, self.source, native.atoms.XdndFinished, [window, u32::from(success), action, 0, 0]);
    }
}

#[derive(Default)]
pub(super) struct DragSource {
    current: Option<Outgoing>,
    // XdndStatus carries the target XID, without a position or session sequence number.
    // Keep unanswered positions after leave and cancellation so later visits cannot consume their replies.
    stale_status: HashSet<(Window, Window)>,
}

struct Outgoing {
    owner: Window,
    time: Timestamp,
    formats: Vec<(Atom, Vec<u8>)>,
    target: Option<Site>,
    latest: Option<Position>,
    waiting_status: bool,
    accepted: bool,
    release: Option<Timestamp>,
    dropped: bool,
    deadline: Option<Instant>,
    keyboard_grabbed: bool,
    streams: Vec<OutgoingTransfer>,
    accept_cursor: u32,
    reject_cursor: u32,
}

struct OutgoingTransfer {
    requestor: Window,
    property: Atom,
    format: usize,
    offset: usize,
    // Preserve other listeners on windows belonging to this X connection.
    previous_mask: EventMask,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Site {
    window: Window,
    proxy: Window,
    version: u32,
}

#[derive(Clone, Copy)]
struct Position {
    x: i16,
    y: i16,
    time: Timestamp,
}

impl DragSource {
    pub fn start(&mut self, native: &Native, state: &WindowState, items: &[DragItem]) -> XResult<bool> {
        if self.current.is_some() || !state.primary_down.get() {
            return Ok(false);
        }
        let Ok(offers) = drag_formats(items) else { return Ok(false) };
        let conn = &native.conn;
        let atoms = &native.atoms;
        let mut formats = Vec::with_capacity(offers.len() + 2);
        for (mime, payload) in offers {
            let atom = match mime {
                "text/uri-list" => atoms.URI_LIST,
                "text/plain" => atoms.TEXT_PLAIN,
                "text/plain;charset=utf-8" => atoms.TEXT_UTF8,
                _ => conn.intern_atom(false, mime.as_bytes())?.reply()?.atom,
            };
            if payload.len() > TRANSFER_LIMIT {
                return Ok(false);
            }
            if atom == atoms.TEXT_UTF8 {
                if let Ok(text) = std::str::from_utf8(&payload)
                    && let Ok(latin1) = text.chars().map(|character| u8::try_from(u32::from(character))).collect::<Result<Vec<_>, _>>()
                {
                    formats.push((AtomEnum::STRING.into(), latin1));
                }
                formats.push((atoms.UTF8_STRING, payload.clone()));
            }
            formats.push((atom, payload));
        }
        let pointer = conn.query_pointer(state.xid)?.reply()?;
        if !pointer.same_screen || !pointer.mask.contains(KeyButMask::BUTTON1) {
            return Ok(false);
        }

        let owner = state.xid;
        let time = native.timestamp.get();
        let types: Vec<_> = formats.iter().map(|(atom, _)| *atom).collect();
        conn.change_property32(PropMode::REPLACE, owner, atoms.XdndTypeList, AtomEnum::ATOM, &types)?.check()?;
        conn.change_property32(PropMode::REPLACE, owner, atoms.XdndActionList, AtomEnum::ATOM, &[atoms.XdndActionCopy])?.check()?;
        if conn.grab_pointer(false, owner, grab_events(), GrabMode::ASYNC, GrabMode::ASYNC, x11rb::NONE, x11rb::NONE, time)?.reply()?.status
            != GrabStatus::SUCCESS
        {
            return Ok(false);
        }
        let selection = (|| -> XResult<bool> {
            conn.set_selection_owner(owner, atoms.XdndSelection, time)?.check()?;
            Ok(conn.get_selection_owner(atoms.XdndSelection)?.reply()?.owner == owner)
        })();
        if !matches!(selection, Ok(true)) {
            let _ = conn.ungrab_pointer(x11rb::CURRENT_TIME);
            return selection;
        }
        let keyboard_grabbed = conn
            .grab_keyboard(false, owner, time, GrabMode::ASYNC, GrabMode::ASYNC)
            .ok()
            .and_then(|cookie| cookie.reply().ok())
            .is_some_and(|reply| reply.status == GrabStatus::SUCCESS);
        let cursor = |name| native.cursor.as_ref().and_then(|handle| handle.load_cursor(conn, name).ok()).unwrap_or(x11rb::NONE);
        let mut outgoing = Outgoing {
            owner,
            time,
            formats,
            target: None,
            latest: None,
            accepted: false,
            waiting_status: false,
            release: None,
            dropped: false,
            deadline: None,
            keyboard_grabbed,
            streams: Vec::new(),
            accept_cursor: cursor("copy"),
            reject_cursor: cursor("no-drop"),
        };
        outgoing.update_cursor(native);
        outgoing.motion(native, Position { x: pointer.root_x, y: pointer.root_y, time }, &mut self.stale_status);
        self.current = Some(outgoing);
        Ok(true)
    }

    pub fn cancel_window(&mut self, native: &Native, xid: Window) {
        if self.current.as_ref().is_some_and(|drag| drag.owner == xid || drag.target.is_some_and(|target| target.window == xid)) {
            self.cancel(native);
        }
        self.stale_status.retain(|&(owner, target)| owner != xid && target != xid);
    }

    fn cancel(&mut self, native: &Native) {
        if let Some(outgoing) = self.current.take() {
            outgoing.cleanup(native, &mut self.stale_status);
        }
    }

    fn event(&mut self, native: &Native, keyboard: &input::Keyboard, event: &Event) -> bool {
        let atoms = &native.atoms;
        let stale = match event {
            Event::ClientMessage(event) if event.format == 32 && event.type_ == atoms.XdndStatus => {
                self.stale_status.remove(&(event.window, event.data.as_data32()[0]))
            }
            _ => false,
        };
        let Some(drag) = self.current.as_mut() else { return stale };
        match event {
            Event::MotionNotify(event) if event.event == drag.owner && drag.tracking() => {
                native.timestamp.set(event.time);
                drag.motion(native, Position { x: event.root_x, y: event.root_y, time: event.time }, &mut self.stale_status);
            }
            Event::ButtonRelease(event) if event.event == drag.owner && drag.tracking() && event.detail == 1 => {
                native.timestamp.set(event.time);
                if let Some(window) = native.window(drag.owner) {
                    window.primary_down.set(false);
                }
                drag.release = Some(event.time);
                drag.ungrab(native);
                if drag.position_blocked(&self.stale_status) {
                    drag.deadline = Some(Instant::now() + STATUS_TIMEOUT);
                } else if !drag.drop_on_target(native) {
                    self.cancel(native);
                }
            }
            Event::ButtonPress(event) | Event::ButtonRelease(event) if event.event == drag.owner && drag.tracking() => {}
            Event::KeyPress(event) if drag.tracking() && keyboard.keysym(native, event) == xkbcommon_dl::keysyms::Escape => {
                self.cancel(native)
            }
            Event::KeyPress(_) | Event::KeyRelease(_) if drag.tracking() && drag.keyboard_grabbed => {}
            Event::ClientMessage(event) if event.window == drag.owner && event.format == 32 && event.type_ == atoms.XdndStatus => {
                let [target, flags, _, _, action] = event.data.as_data32();
                if !drag.dropped && (stale || drag.waiting_status) && drag.target.is_some_and(|site| site.window == target) {
                    if !stale {
                        drag.waiting_status = false;
                        drag.accepted = flags & 1 != 0 && action == atoms.XdndActionCopy;
                        drag.update_cursor(native);
                    }
                    if let Some(position) = drag.latest.take() {
                        drag.send_position(native, position);
                    }
                    if drag.release.is_some() && !drag.waiting_status && !drag.drop_on_target(native) {
                        self.cancel(native);
                    }
                }
            }
            Event::ClientMessage(event) if event.window == drag.owner && event.format == 32 && event.type_ == atoms.XdndFinished => {
                if drag.dropped && drag.target.is_some_and(|site| site.window == event.data.as_data32()[0]) {
                    self.cancel(native);
                }
            }
            Event::SelectionClear(event) if event.selection == atoms.XdndSelection && event.owner == drag.owner => {
                if !older_timestamp(event.time, drag.time) {
                    self.cancel(native);
                }
            }
            Event::SelectionRequest(event) if event.selection == atoms.XdndSelection && event.owner == drag.owner => {
                drag.selection_request(native, event);
            }
            Event::PropertyNotify(event) if event.state == Property::DELETE => {
                return drag.send_chunk(native, event);
            }
            _ => return stale,
        }
        true
    }
}

impl Outgoing {
    fn tracking(&self) -> bool {
        self.release.is_none()
    }

    fn position_blocked(&self, stale: &HashSet<(Window, Window)>) -> bool {
        self.waiting_status || self.target.is_some_and(|target| stale.contains(&(self.owner, target.window)))
    }

    fn motion(&mut self, native: &Native, position: Position, stale: &mut HashSet<(Window, Window)>) {
        let atoms = &native.atoms;
        let target = site_at(native, position);
        if target != self.target {
            self.leave(native, stale);
            self.target = target;
            self.waiting_status = false;
            self.accepted = false;
            self.latest = None;
            self.update_cursor(native);
            if let Some(target) = target {
                let mut data = [self.owner, (target.version << 24) | u32::from(self.formats.len() > 3), 0, 0, 0];
                for (slot, (atom, _)) in data[2..].iter_mut().zip(&self.formats) {
                    *slot = *atom;
                }
                send(native, target.proxy, target.window, atoms.XdndEnter, data);
            }
        }
        if self.position_blocked(stale) {
            self.latest = Some(position);
        } else {
            self.send_position(native, position);
        }
    }

    fn send_position(&mut self, native: &Native, position: Position) {
        if let Some(target) = self.target {
            let packed = (u32::from(position.x as u16) << 16) | u32::from(position.y as u16);
            let data = [self.owner, 0, packed, position.time, native.atoms.XdndActionCopy];
            self.waiting_status = send(native, target.proxy, target.window, native.atoms.XdndPosition, data);
            if !self.waiting_status {
                self.accepted = false;
            }
        }
    }

    fn update_cursor(&self, native: &Native) {
        if self.tracking() {
            let cursor = if self.accepted { self.accept_cursor } else { self.reject_cursor };
            let _ = native.conn.change_active_pointer_grab(cursor, x11rb::CURRENT_TIME, grab_events());
        }
    }

    fn ungrab(&mut self, native: &Native) {
        let _ = native.conn.ungrab_pointer(x11rb::CURRENT_TIME);
        if std::mem::take(&mut self.keyboard_grabbed) {
            let _ = native.conn.ungrab_keyboard(x11rb::CURRENT_TIME);
        }
    }

    fn leave(&self, native: &Native, stale: &mut HashSet<(Window, Window)>) {
        if let Some(target) = self.target {
            if self.waiting_status {
                stale.insert((self.owner, target.window));
            }
            send(native, target.proxy, target.window, native.atoms.XdndLeave, [self.owner, 0, 0, 0, 0]);
        }
    }

    fn drop_on_target(&mut self, native: &Native) -> bool {
        let Some(target) = self.target.filter(|_| self.accepted) else { return false };
        let time = self.release.unwrap_or(x11rb::CURRENT_TIME);
        self.dropped = send(native, target.proxy, target.window, native.atoms.XdndDrop, [self.owner, 0, time, 0, 0]);
        self.deadline = Some(Instant::now() + TRANSFER_TIMEOUT);
        self.dropped
    }

    fn cleanup(mut self, native: &Native, stale: &mut HashSet<(Window, Window)>) {
        if self.tracking() {
            self.ungrab(native);
        }
        if !self.dropped {
            self.leave(native, stale);
        }
        for stream in self.streams.drain(..) {
            stream.restore_mask(native);
        }
        for cursor in [self.accept_cursor, self.reject_cursor] {
            if cursor != x11rb::NONE {
                let _ = native.conn.free_cursor(cursor);
            }
        }
        let selection = native.atoms.XdndSelection;
        let conn = &native.conn;
        // A SelectionClear may indicate another client now owns the drag selection.
        if conn.get_selection_owner(selection).ok().and_then(|cookie| cookie.reply().ok()).is_some_and(|reply| reply.owner == self.owner) {
            let _ = conn.set_selection_owner(x11rb::NONE, selection, self.time);
        }
    }

    fn selection_request(&mut self, native: &Native, event: &SelectionRequestEvent) {
        let property = if event.property == x11rb::NONE { event.target } else { event.property };
        let accepted = !older_timestamp(event.time, self.time) && self.write_selection(native, event, property).is_ok();
        let response = SelectionNotifyEvent {
            response_type: SELECTION_NOTIFY_EVENT,
            sequence: 0,
            time: event.time,
            requestor: event.requestor,
            selection: event.selection,
            target: event.target,
            property: if accepted { property } else { x11rb::NONE },
        };
        let _ = native.conn.send_event(false, event.requestor, EventMask::NO_EVENT, response);
    }

    fn write_selection(&mut self, native: &Native, event: &SelectionRequestEvent, property: Atom) -> XResult<()> {
        let conn = &native.conn;
        let atoms = &native.atoms;
        let requestor = event.requestor;
        let target = event.target;
        if target == atoms.TARGETS {
            let mut targets = vec![atoms.TARGETS, atoms.TIMESTAMP];
            targets.extend(self.formats.iter().map(|(atom, _)| *atom));
            conn.change_property32(PropMode::REPLACE, requestor, property, AtomEnum::ATOM, &targets)?.check()?;
        } else if target == atoms.TIMESTAMP {
            conn.change_property32(PropMode::REPLACE, requestor, property, AtomEnum::INTEGER, &[self.time])?.check()?;
        } else {
            let format = self.formats.iter().position(|(atom, _)| *atom == target).ok_or("Unsupported XDND selection target")?;
            let data = &self.formats[format].1;
            if data.len() <= chunk_size(native) {
                conn.change_property8(PropMode::REPLACE, requestor, property, target, data)?.check()?;
            } else {
                // Serialize incremental streams per requestor so its event mask has one owner.
                if self.streams.iter().any(|stream| stream.requestor == requestor) {
                    return Err("Requestor already has an incremental XDND transfer".into());
                }
                let previous_mask = conn.get_window_attributes(requestor)?.reply()?.your_event_mask;
                let mask = previous_mask | EventMask::PROPERTY_CHANGE;
                conn.change_window_attributes(requestor, &ChangeWindowAttributesAux::new().event_mask(mask))?.check()?;
                let stream = OutgoingTransfer { requestor, property, format, offset: 0, previous_mask };
                let result = conn
                    .change_property32(PropMode::REPLACE, requestor, property, atoms.INCR, &[data.len() as u32])
                    .map_err(ReplyError::from)
                    .and_then(|cookie| cookie.check());
                if let Err(error) = result {
                    stream.restore_mask(native);
                    return Err(error.into());
                }
                self.streams.push(stream);
            }
        }
        if self.dropped {
            self.deadline = Some(Instant::now() + TRANSFER_TIMEOUT);
        }
        Ok(())
    }

    fn send_chunk(&mut self, native: &Native, event: &PropertyNotifyEvent) -> bool {
        let Some(index) = self.streams.iter().position(|stream| stream.requestor == event.window && stream.property == event.atom) else {
            return false;
        };
        let stream = &mut self.streams[index];
        let (target, data) = &self.formats[stream.format];
        let end = (stream.offset + chunk_size(native)).min(data.len());
        let chunk = &data[stream.offset..end];
        let sent = native
            .conn
            .change_property8(PropMode::REPLACE, event.window, event.atom, *target, chunk)
            .ok()
            .is_some_and(|cookie| cookie.check().is_ok());
        stream.offset = end;
        if !sent || chunk.is_empty() {
            self.streams.swap_remove(index).restore_mask(native);
        }
        if self.dropped {
            self.deadline = Some(Instant::now() + TRANSFER_TIMEOUT);
        }
        true
    }
}

impl OutgoingTransfer {
    fn restore_mask(&self, native: &Native) {
        let _ = native.conn.change_window_attributes(self.requestor, &ChangeWindowAttributesAux::new().event_mask(self.previous_mask));
    }
}

fn chunk_size(native: &Native) -> usize {
    native.conn.maximum_request_bytes().saturating_sub(64).clamp(1, 64 * 1024)
}

fn older_timestamp(time: Timestamp, reference: Timestamp) -> bool {
    time != x11rb::CURRENT_TIME && (time.wrapping_sub(reference) as i32) < 0
}

fn site_at(native: &Native, position: Position) -> Option<Site> {
    let mut window = native.root;
    for _ in 0..32 {
        if window != native.root
            && let Some(site) = drop_site(native, window)
        {
            return Some(site);
        }
        let child = native.conn.translate_coordinates(native.root, window, position.x, position.y).ok()?.reply().ok()?.child;
        if child == x11rb::NONE {
            break;
        }
        window = child;
    }
    drop_site(native, native.root)
}

fn drop_site(native: &Native, window: Window) -> Option<Site> {
    let atoms = &native.atoms;
    let target = native.property32(window, atoms.XdndProxy, AtomEnum::WINDOW, 1).and_then(|property| property.first().copied());
    let proxy = target
        .filter(|proxy| native.property32(*proxy, atoms.XdndProxy, AtomEnum::WINDOW, 1).as_deref() == Some(&[*proxy]))
        .unwrap_or(window);
    let version = native.property32(proxy, atoms.XdndAware, AtomEnum::ATOM, 1).and_then(|property| property.first().copied());
    if let Some(version) = version.filter(|version| *version >= MIN_VERSION) {
        return Some(Site { window, proxy, version: version.min(VERSION) });
    }
    None
}

fn send(native: &Native, destination: Window, window: Window, message: Atom, data: [u32; 5]) -> bool {
    let event = ClientMessageEvent::new(32, window, message, data);
    native.conn.send_event(false, destination, EventMask::NO_EVENT, event).ok().is_some_and(|cookie| cookie.check().is_ok())
}

pub(super) fn dispatch(
    native: &Native,
    keyboard: &input::Keyboard,
    event: &Event,
    mut emit: impl FnMut(PlatformEvent) -> Result<bool, String>,
) -> Result<bool, String> {
    if native.source.borrow_mut().event(native, keyboard, event) {
        return Ok(true);
    }
    match event {
        Event::ClientMessage(event) if event.format == 32 => {
            let Some(window) = native.window(event.window) else { return Ok(false) };
            target_message(native, &window, event.type_, event.data.as_data32(), &mut emit)
        }
        Event::SelectionNotify(event) if event.selection == native.atoms.XdndSelection && event.target == native.atoms.URI_LIST => {
            let Some(window) = native.window(event.requestor) else { return Ok(false) };
            let pending = window.target.borrow().as_ref().and_then(|target| target.transfer.as_ref()).is_some_and(|transfer| {
                (transfer.time == x11rb::CURRENT_TIME || transfer.time == event.time) && transfer.incremental.is_none()
            });
            if !pending {
                return Ok(false);
            }
            let packet =
                if event.property == native.atoms.OCTARA_XDND_DATA { read_packet(native, window.xid, true) } else { Packet::Error };
            apply_packet(native, &window, packet, &mut emit)?;
            Ok(true)
        }
        Event::PropertyNotify(event) if event.atom == native.atoms.OCTARA_XDND_DATA && event.state == Property::NEW_VALUE => {
            let Some(window) = native.window(event.window) else { return Ok(false) };
            if window
                .target
                .borrow()
                .as_ref()
                .and_then(|target| target.transfer.as_ref())
                .is_some_and(|transfer| transfer.incremental.is_some())
            {
                let packet = read_packet(native, window.xid, false);
                apply_packet(native, &window, packet, &mut emit)?;
            }
            Ok(true)
        }
        _ => Ok(false),
    }
}

fn target_message(
    native: &Native,
    window: &WindowState,
    message: Atom,
    data: [u32; 5],
    emit: &mut impl FnMut(PlatformEvent) -> Result<bool, String>,
) -> Result<bool, String> {
    let atoms = &native.atoms;
    let [source, _, packed, _, _] = data;
    if message == atoms.XdndEnter {
        let previous_source = window.target.borrow().as_ref().map(|target| target.source);
        let can_enter = previous_source.is_none_or(|previous| {
            previous != source && native.conn.get_window_attributes(previous).ok().and_then(|cookie| cookie.reply().ok()).is_none()
        });
        if can_enter {
            if previous_source.is_some() {
                complete_drop(native, window, None, emit)?;
            }
            *window.target.borrow_mut() = DragTarget::entered(native, data);
        }
        return Ok(true);
    }
    let valid = window.target.borrow().as_ref().is_some_and(|target| target.source == source && target.transfer.is_none());
    if !valid {
        return Ok(false);
    }
    if message == atoms.XdndLeave {
        let target = window.target.borrow_mut().take();
        if let Some(position) = target.and_then(|target| target.position) {
            emit(file_event(window, FileDropPhase::Left, Some(position), Vec::new()))?;
        }
        return Ok(true);
    }
    if message == atoms.XdndPosition {
        let position = native
            .conn
            .translate_coordinates(native.root, window.xid, (packed >> 16) as i16, packed as i16)
            .ok()
            .and_then(|cookie| cookie.reply().ok())
            .filter(|position| position.same_screen)
            .map(|position| input::position(position.dst_x, position.dst_y, native.scale.get()));
        let offers_files = window.target.borrow().as_ref().is_some_and(|target| target.offers_files);
        let accepted = if offers_files && let Some(position) = position {
            emit(file_event(window, FileDropPhase::Entered, Some(position), Vec::new()))?
        } else {
            false
        };
        if let Some(target) = window.target.borrow_mut().as_mut() {
            target.position = position.filter(|_| offers_files);
            target.accepted = accepted;
        }
        let action = if accepted { atoms.XdndActionCopy } else { x11rb::NONE };
        send(native, source, source, atoms.XdndStatus, [window.xid, 2 | u32::from(accepted), 0, 0, action]);
        return Ok(true);
    }
    if message == atoms.XdndDrop {
        let time = packed;
        let accepted = window.target.borrow().as_ref().is_some_and(|target| target.accepted);
        if accepted {
            let conn = &native.conn;
            let property = atoms.OCTARA_XDND_DATA;
            let _ = conn.delete_property(window.xid, property);
            let requested = conn
                .convert_selection(window.xid, atoms.XdndSelection, atoms.URI_LIST, property, time)
                .ok()
                .is_some_and(|cookie| cookie.check().is_ok());
            if requested && let Some(target) = window.target.borrow_mut().as_mut() {
                target.transfer = Some(IncomingTransfer { deadline: Instant::now() + TRANSFER_TIMEOUT, time, incremental: None });
                return Ok(true);
            }
        }
        complete_drop(native, window, None, emit)?;
        return Ok(true);
    }
    Ok(false)
}

enum Packet {
    Data(Vec<u8>),
    Incremental(usize),
    Error,
}

fn read_packet(native: &Native, window: Window, initial: bool) -> Packet {
    let atoms = &native.atoms;
    let property = atoms.OCTARA_XDND_DATA;
    let conn = &native.conn;
    let packet = conn
        .get_property(true, window, property, AtomEnum::ANY, 0, (TRANSFER_LIMIT / 4) as u32)
        .ok()
        .and_then(|cookie| cookie.reply().ok());
    match packet {
        Some(reply) if reply.bytes_after == 0 && reply.type_ == atoms.URI_LIST && reply.format == 8 => Packet::Data(reply.value),
        Some(reply) if initial && reply.type_ == atoms.INCR && reply.bytes_after == 0 => {
            match reply.value32().and_then(|mut values| values.next()).map(|len| len as usize) {
                Some(len) if len <= TRANSFER_LIMIT => Packet::Incremental(len),
                _ => Packet::Error,
            }
        }
        _ => {
            let _ = conn.delete_property(window, property);
            Packet::Error
        }
    }
}

fn apply_packet(
    native: &Native,
    window: &WindowState,
    packet: Packet,
    emit: &mut impl FnMut(PlatformEvent) -> Result<bool, String>,
) -> Result<(), String> {
    let result = {
        let mut target = window.target.borrow_mut();
        let Some(transfer) = target.as_mut().and_then(|target| target.transfer.as_mut()) else { return Ok(()) };
        transfer.deadline = Instant::now() + TRANSFER_TIMEOUT;
        match (packet, &mut transfer.incremental) {
            (Packet::Incremental(size), storage @ None) => {
                *storage = Some(Vec::with_capacity(size.min(64 * 1024)));
                return Ok(());
            }
            (Packet::Data(data), None) => Some(data),
            (Packet::Data(data), Some(buffer)) if data.is_empty() => Some(std::mem::take(buffer)),
            (Packet::Data(data), Some(buffer)) if buffer.len() + data.len() <= TRANSFER_LIMIT => {
                buffer.extend_from_slice(&data);
                return Ok(());
            }
            _ => None,
        }
    };
    complete_drop(native, window, result, emit)
}

fn complete_drop(
    native: &Native,
    window: &WindowState,
    bytes: Option<Vec<u8>>,
    emit: &mut impl FnMut(PlatformEvent) -> Result<bool, String>,
) -> Result<(), String> {
    let target = window.target.borrow_mut().take();
    let Some(target) = target else { return Ok(()) };
    let paths = bytes.map_or_else(Vec::new, |bytes| files_from_uri_list(&bytes));
    let result = if !paths.is_empty() {
        emit(file_event(window, FileDropPhase::Dropped, target.position, paths))
    } else if target.position.is_some() {
        emit(file_event(window, FileDropPhase::Left, target.position, Vec::new())).map(|_| false)
    } else {
        Ok(false)
    };
    target.finish(native, window.xid, result.as_ref().is_ok_and(|accepted| *accepted));
    result.map(|_| ())
}

fn file_event(window: &WindowState, phase: FileDropPhase, position: Option<Point>, paths: Vec<std::path::PathBuf>) -> PlatformEvent {
    PlatformEvent::FileDrop { window: window.id, phase, position, paths }
}

pub(super) fn tick(
    native: &Native,
    now: Instant,
    mut emit: impl FnMut(PlatformEvent) -> Result<bool, String>,
) -> Result<Option<Instant>, String> {
    let deadline = {
        let mut source = native.source.borrow_mut();
        let deadline = source.current.as_ref().and_then(|drag| drag.deadline);
        if deadline.is_some_and(|deadline| deadline <= now) {
            source.cancel(native);
            None
        } else {
            deadline
        }
    };
    let mut next = deadline;
    let windows: Vec<_> = native.windows.borrow().values().filter_map(std::rc::Weak::upgrade).collect();
    for window in windows {
        let deadline = window.target.borrow().as_ref().and_then(|target| target.transfer.as_ref()).map(|transfer| transfer.deadline);
        if let Some(deadline) = deadline {
            if deadline <= now {
                complete_drop(native, &window, None, &mut emit)?;
            } else {
                next = next.into_iter().chain([deadline]).min();
            }
        }
    }
    Ok(next)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_timestamps_follow_x11_wraparound() {
        assert!(older_timestamp(u32::MAX - 1, 2));
        assert!(!older_timestamp(2, u32::MAX - 1));
        assert!(!older_timestamp(x11rb::CURRENT_TIME, 2));
    }
}
