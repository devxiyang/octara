use octara::prelude::*;
use octara_chat::{Chat, Options};
use octara_inspect::HeadlessDriver;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

pub struct Session {
    pub driver: RefCell<HeadlessDriver>,
    pub window: WindowId,
    pub chat: Chat,
}

#[allow(dead_code)]
impl Session {
    pub fn new(options: Options, view: impl FnOnce(&mut BuildCx<'_>, Chat) -> View + 'static) -> Self {
        let mut driver = HeadlessDriver::default();
        let exposed = Rc::new(Cell::new(None));
        let slot = exposed.clone();
        let window = driver
            .app_mut()
            .open_window(WindowOptions::default(), move |b| {
                let chat = Chat::new(b, options);
                slot.set(Some(chat));
                view(b, chat)
            })
            .unwrap();
        Self { driver: RefCell::new(driver), window, chat: exposed.get().unwrap() }
    }

    pub fn update<R>(&self, run: impl FnOnce(&Chat, &mut WriteCx<'_>) -> R) -> R {
        self.driver.borrow_mut().app_mut().update_window(self.window, |w| run(&self.chat, w)).unwrap()
    }

    pub fn settle(&self) {
        self.driver.borrow_mut().app_mut().runtime_mut().settle().unwrap();
    }

    pub fn now(&self) -> Duration {
        self.driver.borrow().app().runtime().now()
    }

    pub fn clean(&self) {
        let diagnostics = self.driver.borrow_mut().app_mut().runtime_mut().take_diagnostics();
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
    }
}
