//! Optional synchronization with the browser's same-document history.

use js_sys::Array;
use octara::runtime::navigation::{RouteEntry, RouteHistory};
use octara::{App, AppError, Navigator, WindowId};
use wasm_bindgen::{JsCast, JsValue};
use web_sys::{History, Location, Window};

use crate::js_error;

type DecodeRoute<R> = dyn Fn(&str) -> Option<R>;

/// Encodes routes as same-origin absolute paths and decodes browser paths. The path includes
/// the query string and fragment when present.
pub struct RouteCodec<R> {
    encode: Box<dyn Fn(&R) -> String>,
    decode: Box<DecodeRoute<R>>,
}

impl<R> RouteCodec<R> {
    pub fn new(encode: impl Fn(&R) -> String + 'static, decode: impl Fn(&str) -> Option<R> + 'static) -> Self {
        Self { encode: Box::new(encode), decode: Box::new(decode) }
    }
}

pub(crate) trait HistoryBridge {
    fn sync(&mut self, app: &App) -> Result<(), JsValue>;
    fn popstate(&mut self, app: &mut App, window: WindowId, state: JsValue) -> Result<(), AppError>;
}

pub(crate) struct BrowserRoutes<R> {
    navigator: Navigator<R>,
    codec: RouteCodec<R>,
    history: History,
    location: Location,
    session: String,
    entries: Vec<(u64, u64)>, // (navigator position, page identity)
    cursor: usize,
    pending: Option<usize>,
    revision: Option<u64>,
}

impl<R: Clone + 'static> BrowserRoutes<R> {
    pub(crate) fn new(browser: &Window, navigator: Navigator<R>, codec: RouteCodec<R>) -> Result<Self, JsValue> {
        Ok(Self {
            navigator,
            codec,
            history: browser.history()?,
            location: browser.location(),
            session: session(),
            entries: Vec::new(),
            cursor: 0,
            pending: None,
            revision: None,
        })
    }

    pub(crate) fn initial_route(&self, app: &App) -> Result<Option<R>, JsValue> {
        let routes = self.routes(app).ok_or("the navigator's scope has retired")?;
        self.location_route(&routes.entries[routes.cursor].route)
    }

    pub(crate) fn seed(&mut self, app: &App) -> Result<(), JsValue> {
        let routes = self.routes(app).ok_or("the navigator's scope has retired")?;
        // Mirror restored forward entries before returning to the restored current location.
        self.replace(&routes.entries[0])?;
        for entry in &routes.entries[1..] {
            self.push(entry)?;
        }
        self.go(routes.cursor)
    }

    fn routes(&self, app: &App) -> Option<RouteHistory<R>> {
        self.navigator.history(app.runtime().world())
    }

    fn location_route(&self, current: &R) -> Result<Option<R>, JsValue> {
        let path = self.location.pathname()? + &self.location.search()? + &self.location.hash()?;
        if path == (self.codec.encode)(current) {
            return Ok(None);
        }
        Ok((self.codec.decode)(&path))
    }

    fn index(&self, position: u64) -> Option<usize> {
        self.entries.iter().position(|&(current, _)| current == position)
    }

    fn marker(&self, cursor: usize) -> JsValue {
        Array::of3(&"octara:navigation".into(), &self.session.as_str().into(), &(cursor as u32).into()).into()
    }

    fn marker_index(&self, state: JsValue) -> Option<usize> {
        let state = state.dyn_into::<Array>().ok()?;
        if state.get(0).as_string().as_deref() != Some("octara:navigation") || state.get(1).as_string().as_deref() != Some(&self.session) {
            return None;
        }
        let index = state.get(2).as_f64()?;
        (index.fract() == 0.0 && index >= 0.0 && index < self.entries.len() as f64).then_some(index as usize)
    }

    fn replace(&mut self, entry: &RouteEntry<R>) -> Result<(), JsValue> {
        let path = (self.codec.encode)(&entry.route);
        self.history.replace_state_with_url(&self.marker(self.cursor), "", Some(&path))?;
        let value = (entry.position, entry.id);
        if let Some(old) = self.entries.get_mut(self.cursor) {
            *old = value;
        } else {
            self.entries.push(value);
        }
        Ok(())
    }

    fn push(&mut self, entry: &RouteEntry<R>) -> Result<(), JsValue> {
        let path = (self.codec.encode)(&entry.route);
        self.history.push_state_with_url(&self.marker(self.cursor + 1), "", Some(&path))?;
        self.cursor += 1;
        self.entries.truncate(self.cursor);
        self.entries.push((entry.position, entry.id));
        Ok(())
    }

    fn go(&mut self, index: usize) -> Result<(), JsValue> {
        if index != self.cursor {
            let delta = i32::try_from(index as isize - self.cursor as isize).map_err(js_error)?;
            self.history.go_with_delta(delta)?;
            self.pending = Some(index);
        }
        Ok(())
    }
}

impl<R: Clone + 'static> HistoryBridge for BrowserRoutes<R> {
    fn sync(&mut self, app: &App) -> Result<(), JsValue> {
        let revision = self.navigator.history_revision(app.runtime().world());
        if self.pending.is_some() || revision == self.revision {
            return Ok(());
        }
        let Some(routes) = self.routes(app) else { return Ok(()) };
        let active = &routes.entries[routes.cursor];
        if let Some(index) = self.index(active.position) {
            if index != self.cursor {
                return self.go(index);
            }
            if self.entries[index].1 != active.id {
                self.replace(active)?;
            }
        } else {
            let prefix = &routes.entries[..routes.cursor];
            let append = if let Some((route, index)) =
                prefix.iter().enumerate().rev().find_map(|(route, entry)| Some((route, self.index(entry.position)?)))
            {
                if index != self.cursor {
                    return self.go(index);
                }
                if self.entries[index].1 != prefix[route].id {
                    self.replace(&prefix[route])?;
                }
                route + 1
            } else {
                self.replace(&routes.entries[0])?;
                1
            };
            for entry in &routes.entries[append..routes.cursor + 1] {
                self.push(entry)?;
            }
        }
        self.revision = revision;
        Ok(())
    }

    fn popstate(&mut self, app: &mut App, window: WindowId, state: JsValue) -> Result<(), AppError> {
        let Some(routes) = self.routes(app) else { return Ok(()) };
        self.revision = None;
        let pending = self.pending.take();
        if let Some(index) = self.marker_index(state) {
            self.cursor = index;
            if pending == Some(index) {
                return Ok(());
            }
            let position = self.entries[index].0;
            if let Some(target) = routes.entries.iter().position(|entry| entry.position == position) {
                let delta = target as isize - routes.cursor as isize;
                app.update_window(window, |w| self.navigator.go(w, delta))?;
            }
            // If a guard vetoed the move or the position was discarded, sync returns the browser
            // to the navigator's still-active history position.
        } else {
            let current = &routes.entries[routes.cursor];
            match self.location_route(&current.route) {
                Ok(Some(route)) => {
                    app.update_window(window, |w| self.navigator.replace(w, route))?;
                }
                Ok(None) => {}
                Err(error) => web_sys::console::error_1(&error),
            }
            let Some(routes) = self.routes(app) else { return Ok(()) };
            self.entries.clear();
            self.session = session();
            self.cursor = 0;
            if let Err(error) = self.replace(&routes.entries[routes.cursor]) {
                web_sys::console::error_1(&error);
            }
        }
        Ok(())
    }
}

fn session() -> String {
    format!("{}-{}", js_sys::Date::now(), js_sys::Math::random())
}
