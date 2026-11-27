use super::long_session::{LongSession, Payload, seed};
use octara::prelude::{Size, WindowId, WindowOptions};
use octara_inspect::{AgentAction, AgentCommand, AgentRequest, HeadlessDriver, TargetRef};
use serde::Serialize;
use std::error::Error;
use std::time::Instant;

pub struct Fixture {
    pub driver: HeadlessDriver,
    pub model: LongSession,
    pub window: WindowId,
    _directory: tempfile::TempDir,
}

#[derive(Serialize)]
pub struct Sample {
    pub elapsed_us: f64,
    pub chunks: usize,
    pub bytes: usize,
    pub transactions: u64,
    pub frame: Option<u64>,
    pub mounted_messages: usize,
}

impl Fixture {
    pub fn new(history: usize, payload: Payload, chunk_bytes: usize) -> Result<Self, Box<dyn Error>> {
        let directory = tempfile::tempdir()?;
        let store = seed(directory.path(), history)?;
        let model = LongSession::new(false);
        model.set_payload(payload, chunk_bytes)?;
        let engine = model.clone();
        let mut options = WindowOptions::default();
        options.viewport.size = Size::new(1000.0, 720.0);
        let mut driver = HeadlessDriver::default();
        let window = driver.app_mut().open_window(options, move |b| engine.view(b, Some(store)))?;
        let mut fixture = Self { driver, window, model, _directory: directory };
        fixture.command("long.start")?;
        Ok(fixture)
    }

    pub fn command(&mut self, name: &str) -> Result<(), Box<dyn Error>> {
        let window = self.driver.app().window(self.window).unwrap();
        let target = TargetRef::for_window(window.runtime().tree().id());
        let action = AgentAction::MenuCommand { name: name.into() };
        let reply = self.driver.execute(AgentRequest::new(Some(self.window), AgentCommand::Act { target, action }));
        if !reply.ok || reply.handled != Some(true) || !reply.diagnostics.is_empty() {
            return Err(format!("fixture command {name} failed: {reply:?}").into());
        }
        Ok(())
    }

    pub fn sample(&mut self, batch: usize) -> Result<Sample, Box<dyn Error>> {
        let before = self.driver.app().window(self.window).unwrap().runtime().transaction_count();
        let bytes = self.model.sent_bytes();
        let start = Instant::now();
        let chunks = self.model.emit_burst(batch);
        self.driver.app_mut().runtime_mut().settle()?;
        let elapsed_us = start.elapsed().as_secs_f64() * 1e6;
        if chunks != batch {
            return Err("the fixture reached its input limit".into());
        }
        let sample = {
            let window = self.driver.app().window(self.window).unwrap();
            let runtime = window.runtime();
            let tree = runtime.tree();
            let mounted_messages = tree
                .subtree(tree.root())
                .filter(|id| tree.provenance(*id).and_then(|p| p.test_id.as_deref()).is_some_and(|id| id.starts_with("msg-")))
                .count();
            Sample {
                elapsed_us,
                chunks,
                bytes: self.model.sent_bytes() - bytes,
                transactions: runtime.transaction_count() - before,
                frame: runtime.last_commit().and_then(|commit| commit.frame),
                mounted_messages,
            }
        };
        let diagnostics = self.driver.app_mut().runtime_mut().take_diagnostics();
        if !diagnostics.is_empty() {
            return Err(format!("streaming diagnostics: {diagnostics:?}").into());
        }
        Ok(sample)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.driver.app_mut().close_window(self.window);
    }
}
