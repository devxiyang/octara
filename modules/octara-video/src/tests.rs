use super::*;
use octara::module::{NativeChange, NativeViewMounts};
use octara::prelude::*;
use octara::{CommitOutput, ContactTarget, HostEngine, HostReply, Input, NodeId, ProjectError, ProjectFeedback, Tree, TreeDelta};

struct Headless;

impl HostEngine for Headless {
    fn project(&mut self, _: &Tree, _: &TreeDelta, _: WindowViewport, _: f64) -> Result<ProjectFeedback, ProjectError> {
        Ok(ProjectFeedback::default())
    }

    fn commit(&mut self) -> CommitOutput {
        CommitOutput::default()
    }

    fn rollback(&mut self) {}

    fn hit_test(&self, _: &Tree, _: Point) -> Option<ContactTarget> {
        None
    }

    fn geometry(&self, _: NodeId) -> Option<LayoutBox> {
        None
    }
}

fn definition(source: VideoSource) -> AppDefinition {
    AppDefinition::new().window(WindowDefinition::new(WindowOptions::default(), move |_| video(source).test_id("clip")))
}

#[test]
fn video_without_an_adapter_renders_its_poster() {
    let mut app = App::new(|_, _| Box::new(Headless));
    let poster = "https://example.test/poster.png";
    let source = VideoSource { poster: Some(poster.to_owned()), ..VideoSource::new("https://example.test/clip.mp4") };
    let window = definition(source).launch(&mut app, None).unwrap()[0];
    let tree = app.runtime().window(window).unwrap().tree();
    let node = tree.test_ids("clip")[0];
    let media = tree.media_source(node).unwrap();
    assert_eq!(media.source, ImageSource::Uri(poster.into()));
    assert_eq!(media.fit, ObjectFit::Contain);
    assert_eq!(tree.native_nodes().count(), 0);
    assert!(!app.update_window(window, |write| send(write, "clip", VideoCommand::Play)).unwrap());
}

#[test]
fn installing_the_mock_mounts_typed_props_and_routes_commands() {
    let mut app = App::new(|_, _| Box::new(Headless));
    let source = VideoSource { poster: Some("poster.png".into()), controls: false, ..VideoSource::new("clip.mp4") };
    let window = definition(source.clone()).with_module(mock()).launch(&mut app, None).unwrap()[0];
    let tree = app.runtime().window(window).unwrap().tree();
    let node = tree.test_ids("clip")[0];
    let native = tree.native(node).unwrap();
    let props = native.typed_props.clone().unwrap();
    assert_eq!(props.downcast_ref::<VideoSource>(), Some(&source));
    assert_eq!(native.kind, ID.0);
    assert_eq!(native.slot, NativeSlot::Overlay);
    assert!(tree.media_source(node).is_none());

    let id = NativeId::from(node);
    let target = tree.guard(node);
    let surface = HostSurface::new(HostSurfaceKind::Headless, Some(window));
    let dispatcher = app.runtime().module_dispatcher();
    let mut mounts = NativeViewMounts::default();
    let create = NativeChange::CreateTyped { id, kind: ID.0.to_owned(), slot: NativeSlot::Overlay, props };
    assert!(mounts.apply_change(&create, surface, app.runtime_mut().native_views_mut(), &dispatcher).unwrap());
    app.dispatch(window, Input::Host(HostReply::NativeLive { target, live: true })).unwrap();

    assert!(app.update_window(window, |write| send(write, "clip", VideoCommand::Play)).unwrap());
    let [(target_window, command)] = app.runtime_mut().take_typed_native_commands().try_into().ok().unwrap();
    assert_eq!(target_window, window);
    assert_eq!(command.id, id);
    assert_eq!(command.payload.downcast_ref::<VideoCommand>(), Some(&VideoCommand::Play));
    assert!(mounts.command(command, &dispatcher));
}
