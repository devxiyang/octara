use crate::{VideoCommand, VideoSource, snapshot_unavailable};
use octara::module::{CallTicket, ModuleDispatcher, NativePlacement, NativeSnapshotRequest, NativeViewInstance};
use std::any::Any;

pub(super) struct Video {
    source: VideoSource,
    visible: bool,
    playing: bool,
}

impl Video {
    pub(super) fn new(source: VideoSource) -> Self {
        Self { source, visible: false, playing: false }
    }
}

impl NativeViewInstance for Video {
    fn update(&mut self, props: Box<dyn Any>) {
        if let Ok(source) = props.downcast::<VideoSource>() {
            if self.source.uri != source.uri {
                self.playing = false;
            }
            self.source = *source;
        }
    }

    fn place(&mut self, placement: &NativePlacement) {
        self.visible = placement.visible;
        self.playing &= self.visible;
    }

    fn call(&mut self, _: CallTicket, command: Box<dyn Any>, _: ModuleDispatcher) {
        if let Ok(command) = command.downcast::<VideoCommand>() {
            self.playing = self.visible && *command == VideoCommand::Play;
        }
    }

    fn snapshot(&mut self, request: NativeSnapshotRequest, reply: ModuleDispatcher) {
        snapshot_unavailable(request, reply);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use octara::Rect;
    use octara::module::{NativeId, NativeSlot};

    #[test]
    fn changing_source_or_hiding_pauses_mock_playback() {
        let mut view = Video::new(VideoSource::new("first.mp4"));
        let command = |view: &mut Video, command| view.call(CallTicket(1), Box::new(command), ModuleDispatcher::default());
        let mut placement = NativePlacement {
            id: NativeId(1),
            slot: NativeSlot::Overlay,
            rect: Rect::new(0.0, 0.0, 200.0, 120.0),
            radius: [0.0; 4],
            clip: Rect::new(0.0, 0.0, 200.0, 120.0),
            clip_radius: 0.0,
            opacity: 1.0,
            visible: true,
            covered: Vec::new(),
        };
        view.place(&placement);
        command(&mut view, VideoCommand::Play);
        assert!(view.playing);
        view.update(Box::new(VideoSource { controls: false, ..view.source.clone() }));
        assert!(view.playing);
        view.update(Box::new(VideoSource::new("second.mp4")));
        assert!(!view.playing);

        command(&mut view, VideoCommand::Play);
        assert!(view.playing);
        command(&mut view, VideoCommand::Pause);
        assert!(!view.playing);
        command(&mut view, VideoCommand::Play);
        placement.visible = false;
        view.place(&placement);
        assert!(!view.playing);
        command(&mut view, VideoCommand::Play);
        assert!(!view.playing);
    }
}
