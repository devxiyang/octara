//! Time the v2 application and projection paths with seeded example state.

use octara::prelude::*;
use octara::{A11yAction, EditorEvent, TextEdit};
use octara_examples::{apps, window_options};
use octara_inspect::{AgentAction, AgentCommand, AgentRequest, HeadlessDriver};
use std::time::Instant;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    match std::env::args().nth(1).as_deref() {
        None => {
            profile("counter", apps::counter_app(), "inc", 100)?;
            profile("todo (1,000)", app("Todo", |b| apps::todo(b, 1_000)), "toggle-500", 20)?;
            profile("chat (1,000) composer editing", app("Chat", |b| apps::chat(b, 1_000)), "composer", 20)?;
            profile("settings", app("Settings", apps::settings), "notifications", 20)
        }
        Some(mode) => Err(format!("unknown benchmark mode {mode:?}").into()),
    }
}

fn app(title: &str, view: fn(&mut BuildCx<'_>) -> El) -> AppDefinition {
    AppDefinition::new().window(WindowDefinition::new(window_options(title, 800.0, 600.0), view))
}

fn profile(name: &str, app: AppDefinition, id: &str, iterations: u32) -> Result<(), Box<dyn std::error::Error>> {
    let mut driver = HeadlessDriver::default();
    let start = Instant::now();
    let window = driver.launch(app)?[0];
    let init = start.elapsed();
    let snapshot = driver.execute(AgentRequest::new(Some(window), AgentCommand::Observe)).snapshot.ok_or("missing settled observation")?;
    let target = snapshot.nodes.iter().find(|node| node.test_id.as_deref() == Some(id)).ok_or("missing benchmark target")?.target;
    if id == "composer" {
        let focus = AgentAction::Accessibility { action: A11yAction::Focus };
        let receipt = driver.execute(AgentRequest::new(Some(window), AgentCommand::Act { target, action: focus }));
        if !receipt.ok || receipt.handled != Some(true) {
            return Err("the benchmark could not focus the composer".into());
        }
    }
    let start = Instant::now();
    for index in 0..iterations {
        let action = if id == "composer" {
            let snapshot = driver.execute(AgentRequest::new(Some(window), AgentCommand::Observe)).snapshot.ok_or("no observation")?;
            let editor = snapshot.editor.ok_or("the composer lost focus")?;
            let event = EditorEvent::Edit {
                session: editor.session,
                expected_rev: editor.document_rev,
                edit: TextEdit::Insert { text: if index % 2 == 0 { "a" } else { "b" }.into(), replacement: None },
            };
            AgentAction::Editor { event: event.into() }
        } else {
            AgentAction::Accessibility { action: A11yAction::Press }
        };
        let receipt = driver.execute(AgentRequest::new(Some(window), AgentCommand::Act { target, action }));
        if !receipt.ok || receipt.handled != Some(true) {
            return Err(receipt.message.unwrap_or_else(|| "action refused".into()).into());
        }
    }
    let state = driver.app().runtime().window(window).expect("benchmark window");
    println!(
        "{name}\tlaunch {:.2?}\tupdate {:.2?}\t{} nodes\t{} transactions",
        init,
        start.elapsed() / iterations,
        snapshot.nodes.len(),
        state.transaction_count()
    );
    Ok(())
}
