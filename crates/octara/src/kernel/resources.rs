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
            text: TextSystem { engine: RefCell::new(Some(text)), available: None, retired: Cell::new(false) },
            touch: Cell::new(touch),
            reduced_motion: Cell::new(false),
        }
    }
}

/// A provider can run without holding the storage borrowed. Unavailable engines are never
/// replaced with dummy fonts; synchronous native queries can decline and retry instead.
pub(crate) struct TextSystem {
    engine: RefCell<Option<Box<dyn TextEngine>>>,
    available: Option<Box<dyn Fn() -> bool>>,
    retired: Cell<bool>,
}

impl TextSystem {
    /// App resources can outlive their owner in a native snapshot. Standalone kernels do not
    /// install this check; the resource layer knows nothing about execution runtimes.
    pub(crate) fn set_available(&mut self, available: impl Fn() -> bool + 'static) {
        self.available = Some(Box::new(available));
    }

    pub(crate) fn is_available(&self) -> bool {
        !self.retired.get() && self.available.as_ref().is_none_or(|available| available())
    }

    /// Stop new native queries while keeping the engine owned for ordinary resource release.
    pub(crate) fn retire(&self) {
        self.retired.set(true);
    }

    pub(crate) fn borrow(&self) -> std::cell::Ref<'_, Box<dyn TextEngine>> {
        std::cell::Ref::map(self.engine.borrow(), |engine| engine.as_ref().expect("text engine is in use by its provider"))
    }

    pub(crate) fn borrow_mut(&self) -> std::cell::RefMut<'_, Box<dyn TextEngine>> {
        self.try_borrow_mut().expect("text engine is in use")
    }

    pub(crate) fn try_borrow_mut(&self) -> Option<std::cell::RefMut<'_, Box<dyn TextEngine>>> {
        if !self.is_available() {
            return None;
        }
        std::cell::RefMut::filter_map(self.engine.try_borrow_mut().ok()?, Option::as_mut).ok()
    }

    pub(crate) fn with_detached(&self, f: impl FnOnce(&mut dyn TextEngine)) {
        let engine = self.engine.borrow_mut().take().expect("text engine is in use");
        let mut restore = std::mem::DropGuard::new(engine, |engine| {
            *self.engine.borrow_mut() = Some(engine);
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
                assert!(resources.text.engine.try_borrow_mut().is_ok(), "provider code holds no storage borrow");
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
