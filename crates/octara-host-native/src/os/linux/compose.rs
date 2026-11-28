use crate::text_bridge::HostTextBridge;
use octara_model::input::Modifiers;
use octara_model::text::TextEdit;
use std::ffi::CString;
use std::ptr::{NonNull, null_mut};
use xkbcommon_dl as xkb_api;

#[derive(Clone, Copy)]
pub(super) struct KeySymbol {
    pub value: u32,
    pub modifiers: Modifiers,
}

pub(super) struct Compose {
    library: &'static xkb_api::XkbCommonCompose,
    state: NonNull<xkb_api::xkb_compose_state>,
}

impl Compose {
    pub(super) fn new() -> Option<Self> {
        let xkb = xkb_api::xkbcommon_option()?;
        let library = xkb_api::xkbcommon_compose_option()?;
        let locale = ["LC_ALL", "LC_CTYPE", "LANG"]
            .into_iter()
            .find_map(|key| std::env::var(key).ok().filter(|value| !value.is_empty()))
            .unwrap_or_else(|| "C".into());
        let locale = CString::new(locale).ok()?;
        // SAFETY: The locale is NUL-terminated. The table retains the context it needs and
        // the compose state retains its table, so temporary references can be released.
        unsafe {
            let context = NonNull::new((xkb.xkb_context_new)(xkb_api::xkb_context_flags::XKB_CONTEXT_NO_FLAGS))?;
            let table = (library.xkb_compose_table_new_from_locale)(
                context.as_ptr(),
                locale.as_ptr(),
                xkb_api::xkb_compose_compile_flags::XKB_COMPOSE_COMPILE_NO_FLAGS,
            );
            (xkb.xkb_context_unref)(context.as_ptr());
            let table = NonNull::new(table)?;
            let state = (library.xkb_compose_state_new)(table.as_ptr(), xkb_api::xkb_compose_state_flags::XKB_COMPOSE_STATE_NO_FLAGS);
            (library.xkb_compose_table_unref)(table.as_ptr());
            Some(Self { library, state: NonNull::new(state)? })
        }
    }

    pub(super) fn is_composing(&self) -> bool {
        // SAFETY: This compose state is live and only used on the UI thread.
        unsafe { (self.library.xkb_compose_state_get_status)(self.state.as_ptr()) == xkb_api::xkb_compose_status::XKB_COMPOSE_COMPOSING }
    }

    pub(super) fn dispatch(&self, key: KeySymbol, editor: &mut HostTextBridge) -> bool {
        if editor.session().is_none() {
            return false;
        }
        let Some(result) = self.handle(key) else { return false };
        if let Some(text) = result {
            editor.edit(TextEdit::Insert { text, replacement: None });
        }
        true
    }

    /// None leaves the key to the editor. Some consumes it and may commit text.
    fn handle(&self, key: KeySymbol) -> Option<Option<String>> {
        use xkb_api::xkb_compose_status::*;

        let active = self.is_composing();
        if key.modifiers.intersects(Modifiers::CTRL | Modifiers::META) || (active && key.value == xkb_api::keysyms::Escape) {
            self.reset();
            return active.then_some(None);
        }
        let state = self.state.as_ptr();
        let lib = self.library;
        // SAFETY: The state is owned and used only on the UI thread.
        unsafe {
            (lib.xkb_compose_state_feed)(state, key.value);
            match (lib.xkb_compose_state_get_status)(state) {
                XKB_COMPOSE_NOTHING => active.then_some(None),
                XKB_COMPOSE_COMPOSING => Some(None),
                XKB_COMPOSE_CANCELLED => {
                    self.reset();
                    Some(None)
                }
                XKB_COMPOSE_COMPOSED => {
                    let len = (lib.xkb_compose_state_get_utf8)(state, null_mut(), 0).max(0) as usize;
                    let mut bytes = vec![0u8; len + 1];
                    (lib.xkb_compose_state_get_utf8)(state, bytes.as_mut_ptr().cast(), bytes.len());
                    bytes.truncate(len);
                    self.reset();
                    Some(String::from_utf8(bytes).ok())
                }
            }
        }
    }

    pub(super) fn reset(&self) {
        // SAFETY: This instance owns the live compose state.
        unsafe { (self.library.xkb_compose_state_reset)(self.state.as_ptr()) };
    }
}

impl Drop for Compose {
    fn drop(&mut self) {
        // SAFETY: This instance owns one compose-state reference.
        unsafe { (self.library.xkb_compose_state_unref)(self.state.as_ptr()) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xkb_api::keysyms as sym;

    #[test]
    #[ignore = "requires the system XKB compose table"]
    fn composing_keys_are_consumed_before_app_shortcuts() {
        let compose = Compose::new().unwrap();
        let key = |value| KeySymbol { value, modifiers: Modifiers::empty() };
        for value in [sym::Escape, sym::Return, sym::space, sym::BackSpace, sym::_1, sym::Left, sym::Right] {
            assert_eq!(compose.handle(key(sym::dead_acute)), Some(None));
            assert!(compose.is_composing());
            assert!(compose.handle(key(value)).is_some());
            compose.reset();
        }
        assert_eq!(compose.handle(key(sym::dead_acute)), Some(None));
        assert_eq!(compose.handle(key(sym::e)), Some(Some("\u{e9}".into())));
        assert!(!compose.is_composing());
        assert_eq!(compose.handle(key(sym::Escape)), None);
    }
}
