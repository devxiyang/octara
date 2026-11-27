//! Value and reactive boundaries for replaceable document blocks.

use crate::CalloutKind;
use octara::prelude::*;
use std::fmt;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BlockPlacement {
    Root,
    Blockquote,
    Callout(CalloutKind),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SlotId(pub Arc<str>);

impl fmt::Display for SlotId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodeInfo {
    pub lang: Arc<str>,
    pub info: Arc<str>,
}

pub type DirectiveAttrs = Arc<[(Arc<str>, Arc<str>)]>;

/// Values presented to the slot renderer. Streaming bodies and completion flags
/// are signals owned by the slot's keyed scope.
#[derive(Clone, Debug, PartialEq)]
pub enum BlockSlot {
    Code {
        id: SlotId,
        placement: BlockPlacement,
        info: CodeInfo,
        code: ReadSignal<Arc<str>>,
        is_complete: ReadSignal<bool>,
    },
    StandaloneLink {
        id: SlotId,
        placement: BlockPlacement,
        url: Arc<str>,
        label: Arc<str>,
    },
    Image {
        id: SlotId,
        placement: BlockPlacement,
        url: Arc<str>,
        alt: Arc<str>,
        title: Option<Arc<str>>,
        occurrence_index: usize,
    },
    Video {
        id: SlotId,
        placement: BlockPlacement,
        src: Arc<str>,
        poster: Option<Arc<str>>,
    },
    Math {
        id: SlotId,
        placement: BlockPlacement,
        tex: ReadSignal<Arc<str>>,
        is_complete: ReadSignal<bool>,
    },
    Directive {
        id: SlotId,
        placement: BlockPlacement,
        name: Arc<str>,
        attrs: DirectiveAttrs,
        body: ReadSignal<Arc<str>>,
        is_complete: ReadSignal<bool>,
    },
}

impl BlockSlot {
    pub fn id(&self) -> &SlotId {
        match self {
            Self::Code { id, .. }
            | Self::StandaloneLink { id, .. }
            | Self::Image { id, .. }
            | Self::Video { id, .. }
            | Self::Math { id, .. }
            | Self::Directive { id, .. } => id,
        }
    }

    pub fn placement(&self) -> BlockPlacement {
        match *self {
            Self::Code { placement, .. }
            | Self::StandaloneLink { placement, .. }
            | Self::Image { placement, .. }
            | Self::Video { placement, .. }
            | Self::Math { placement, .. }
            | Self::Directive { placement, .. } => placement,
        }
    }
}

/// Provide an `Arc<dyn BlockSlotRenderer>` to a subtree. Return `None` to use
/// the Markdown document's default view for this slot.
pub trait BlockSlotRenderer: 'static {
    fn render_slot(&self, cx: &mut BuildCx<'_>, slot: &BlockSlot) -> Option<El>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SlotRecord {
    pub id: SlotId,
    pub placement: BlockPlacement,
    pub content: SlotContent,
}

/// Immutable values available for indexing and prefetch before mounting.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SlotContent {
    Code { info: CodeInfo, code: Arc<str>, is_complete: bool },
    StandaloneLink { url: Arc<str>, label: Arc<str> },
    Image { url: Arc<str>, alt: Arc<str>, title: Option<Arc<str>>, occurrence_index: usize },
    Video { src: Arc<str>, poster: Option<Arc<str>> },
    Math { tex: Arc<str>, is_complete: bool },
    Directive { name: Arc<str>, attrs: DirectiveAttrs, body: Arc<str>, is_complete: bool },
}

impl SlotRecord {
    pub(crate) fn mount(&self, build: &mut BuildCx<'_>, value: ReadSignal<Option<SlotRecord>>) -> BlockSlot {
        let id = self.id.clone();
        let placement = self.placement;
        let read_content = move |read: &mut ReadCx<'_>| {
            read.with(value, |value| match value.as_ref().map(|slot| &slot.content) {
                Some(SlotContent::Code { code, is_complete, .. }) => (code.clone(), *is_complete),
                Some(SlotContent::Math { tex, is_complete }) => (tex.clone(), *is_complete),
                Some(SlotContent::Directive { body, is_complete, .. }) => (body.clone(), *is_complete),
                _ => (Arc::from(""), false),
            })
        };
        let signals = |build: &mut BuildCx<'_>| {
            let state = build.memo(read_content);
            (build.memo(move |r| r.with(state, |(body, _)| body.clone())), build.memo(move |r| r.with(state, |(_, done)| *done)))
        };
        match &self.content {
            SlotContent::Code { info, .. } => {
                let (code, is_complete) = signals(build);
                BlockSlot::Code { id, placement, info: info.clone(), code, is_complete }
            }
            SlotContent::StandaloneLink { url, label } => {
                BlockSlot::StandaloneLink { id, placement, url: url.clone(), label: label.clone() }
            }
            SlotContent::Image { url, alt, title, occurrence_index } => BlockSlot::Image {
                id,
                placement,
                url: url.clone(),
                alt: alt.clone(),
                title: title.clone(),
                occurrence_index: *occurrence_index,
            },
            SlotContent::Video { src, poster } => BlockSlot::Video { id, placement, src: src.clone(), poster: poster.clone() },
            SlotContent::Math { .. } => {
                let (tex, is_complete) = signals(build);
                BlockSlot::Math { id, placement, tex, is_complete }
            }
            SlotContent::Directive { name, attrs, .. } => {
                let (body, is_complete) = signals(build);
                BlockSlot::Directive { id, placement, name: name.clone(), attrs: attrs.clone(), body, is_complete }
            }
        }
    }
}
