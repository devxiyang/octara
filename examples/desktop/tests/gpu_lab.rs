use octara::prelude::*;
use octara::{CaptureSource, PaintFallback, PaintOutcome};
use octara_examples::window_options;
use octara_examples_desktop::gpu_lab;
use octara_inspect::{AgentCommand, AgentRequest, HeadlessDriver};

#[test]
fn reference_capture_samples_shader_lab_fallbacks() {
    let mut driver = HeadlessDriver::default();
    let app = AppDefinition::new().window(WindowDefinition::new(window_options("Example", 800.0, 600.0), gpu_lab::view));
    let window = driver.launch(app).unwrap()[0];
    let initial = driver.execute(AgentRequest::new(Some(window), AgentCommand::Observe));
    assert!(initial.ok && initial.progress.settled, "{initial:?}");
    let snapshot = initial.snapshot.unwrap();
    let preview = snapshot.nodes.iter().find(|node| node.test_id.as_deref() == Some("gpu-preview")).expect("missing #gpu-preview");
    let flow = preview.target.node().unwrap();

    let request = AgentRequest::new(Some(window), AgentCommand::Capture { source: CaptureSource::Painted, settle_ms: 0 });
    let capture = driver.execute(request).capture.expect("reference capture");
    assert_eq!((capture.picture.width(), capture.picture.height()), (800, 600));
    assert!(capture.effects.iter().any(|receipt| {
        receipt.source == flow && receipt.outcome == PaintOutcome::Fallback { reason: PaintFallback::ReferenceRenderer }
    }));
}
