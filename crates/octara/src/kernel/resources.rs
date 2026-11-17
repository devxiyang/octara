use super::*;

/// What every window's kernel shares ([design](https://github.com/octoii-works/octara/blob/main/docs/design/core.md)):
/// the theme, the text system (fonts, fallback, shaping and layout caches), whether input is
/// touch, and the reduced-motion preference. The app changes them and tells each kernel.
pub(crate) struct Resources {
    pub(crate) theme: RefCell<Theme>,
    pub(crate) text: TextSystem,
    pub(crate) touch: Cell<bool>,
    pub(crate) reduced_motion: Cell<bool>,
}

impl Default for Resources {
    fn default() -> Self {
        Self::new(Theme::default(), Box::new(text::ParleyEngine::default()), false)
    }
}

impl Resources {
    pub(crate) fn new(theme: Theme, text: Box<dyn TextEngine>, touch: bool) -> Self {
        Self {
            theme: RefCell::new(theme),
            text: TextSystem(RefCell::new(Some(text))),
            touch: Cell::new(touch),
            reduced_motion: Cell::new(false),
        }
    }
}

/// A provider can run without holding the storage borrowed. Unavailable engines are never
/// replaced with dummy fonts; synchronous native queries can decline and retry instead.
pub(crate) struct TextSystem(RefCell<Option<Box<dyn TextEngine>>>);

impl TextSystem {
    pub(crate) fn borrow(&self) -> std::cell::Ref<'_, Box<dyn TextEngine>> {
        std::cell::Ref::map(self.0.borrow(), |engine| engine.as_ref().expect("text engine is in use by its provider"))
    }

    pub(crate) fn borrow_mut(&self) -> std::cell::RefMut<'_, Box<dyn TextEngine>> {
        self.try_borrow_mut().expect("text engine is in use")
    }

    pub(crate) fn try_borrow_mut(&self) -> Option<std::cell::RefMut<'_, Box<dyn TextEngine>>> {
        std::cell::RefMut::filter_map(self.0.try_borrow_mut().ok()?, Option::as_mut).ok()
    }

    pub(crate) fn with_detached(&self, f: impl FnOnce(&mut dyn TextEngine)) {
        let engine = self.0.borrow_mut().take().expect("text engine is in use");
        let mut restore = std::mem::DropGuard::new(engine, |engine| {
            *self.0.borrow_mut() = Some(engine);
        });
        f(&mut **restore);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_lease_releases_storage_and_restores_the_engine_after_panic() {
        let resources = Resources::default();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            resources.text.with_detached(|_| {
                assert!(resources.text.0.try_borrow_mut().is_ok(), "provider code holds no storage borrow");
                assert!(resources.text.try_borrow_mut().is_none(), "native queries can decline while the provider runs");
                panic!("provider failed");
            });
        }));
        assert!(result.is_err());
        let mut engine = resources.text.try_borrow_mut().expect("the guard restored the engine");
        let layout = engine.layout("still owned", &TextStyle::default(), &[], None, 0);
        assert!(layout.measure(None).0 > 0.0);
    }
}
