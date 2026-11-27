//! Compile verified scene data into the same transactions as authored Rust views.

use crate::{El, Styled, View, WriteCx, el, text};
use octara_model::semantics::Role;
use octara_model::style::{FlexDirection, Length, StyleProp};
use octara_model::theme::{Color, Fill};
use octara_tree::tables::{AuthoredStyle, CommandSlot, HandlerId, HandlerKind, SemanticsSlot};
use octara_tree::{NodeChange, NodeContent, NodeFlags, NodeId, NodeRef, NodeSpec, Transaction, Tree, TxBuilder};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::Arc;

pub const SCENE_FORMAT: &str = "octara.scene/2";

#[derive(Debug)]
pub enum SceneError {
    Json(serde_json::Error),
    Format(String),
    Color(String),
    Command(String),
    Length,
}

impl std::fmt::Display for SceneError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Json(error) => error.fmt(f),
            Self::Format(format) => write!(f, "expected {SCENE_FORMAT}, received {format}"),
            Self::Color(name) => write!(f, "scene palette has no entry for {name}"),
            Self::Command(name) => write!(f, "application did not bind scene command {name}"),
            Self::Length => f.write_str("scene dimensions must be finite and nonnegative"),
        }
    }
}

impl std::error::Error for SceneError {}

/// Layout, styles and a color palette carried by a signed resource's payload.
///
/// Decode after verifying the resource's target and publisher, for example inside
/// `octara-release`'s prepare callback. Names resolve against app-owned command registrations.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScenePacket {
    format: String,
    #[serde(default)]
    palette: BTreeMap<String, [u8; 4]>,
    nodes: Vec<Spec>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct Spec {
    content: Content,
    #[serde(default)]
    children: Vec<Spec>,
    #[serde(default)]
    style: Style,
    test_id: Option<String>,
    label: Option<String>,
    role: Option<Role>,
    press: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", deny_unknown_fields)]
enum Content {
    Box,
    Text { value: String },
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct Style {
    width: Option<Dimension>,
    height: Option<Dimension>,
    padding: Option<f32>,
    background: Option<Paint>,
    text: Option<Paint>,
    direction: Option<Direction>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(untagged)]
enum Dimension {
    Pixels(f32),
    Fraction(Fraction),
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct Fraction {
    fraction: f32,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(untagged)]
enum Paint {
    Rgba([u8; 4]),
    Palette(String),
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Direction {
    Row,
    Column,
}

impl ScenePacket {
    pub fn parse(payload: &str) -> Result<Self, SceneError> {
        let scene: Self = serde_json::from_str(payload).map_err(SceneError::Json)?;
        if scene.format != SCENE_FORMAT {
            return Err(SceneError::Format(scene.format));
        }
        Ok(scene)
    }

    /// Compiles directly against a retained tree, for example to preflight a candidate.
    /// For app windows use [`Self::into_view`] to register runtime-scoped commands.
    pub fn compile(
        self,
        tree: &Tree,
        parent: NodeId,
        mut commands: impl FnMut(&str) -> Option<HandlerId>,
    ) -> Result<Transaction, SceneError> {
        let mut draft = tree.transaction();
        let parent = tree.node_ref(parent);
        for spec in self.nodes {
            spec.emit(parent, &self.palette, &mut commands, &mut draft)?;
        }
        Ok(draft.finish())
    }

    /// Prepares a scene for a window by resolving each command to an app-owned callback.
    /// The view mounts through the window's authored transaction and registers commands in its scope.
    pub fn into_view<C>(self, mut commands: impl FnMut(&str) -> Option<C>) -> Result<View, SceneError>
    where
        C: Fn(&mut WriteCx<'_>) + 'static,
    {
        let nodes: Vec<_> = self.nodes.into_iter().map(|spec| spec.into_el(&self.palette, &mut commands)).collect::<Result<_, _>>()?;
        Ok(View::fragment(nodes))
    }
}

impl Spec {
    fn emit(
        self,
        parent: NodeRef,
        palette: &BTreeMap<String, [u8; 4]>,
        commands: &mut impl FnMut(&str) -> Option<HandlerId>,
        draft: &mut TxBuilder,
    ) -> Result<(), SceneError> {
        let content = match self.content {
            Content::Box => NodeContent::Box,
            Content::Text { value } => NodeContent::Text(value.into()),
        };
        let node = draft.create(NodeSpec::new(content));
        let authored = self.style.resolve(palette)?;
        if !authored.base.is_empty() {
            draft.change(node, NodeChange::Style(Box::new(authored)));
        }
        if let Some(id) = self.test_id {
            draft.change(node, NodeChange::TestId(Some(id.into())));
        }
        if self.label.is_some() || self.role.is_some() {
            let slot = SemanticsSlot { label: self.label.map(Into::into), role: self.role, ..Default::default() };
            draft.change(node, NodeChange::Semantics(Box::new(slot)));
        }
        if let Some(name) = self.press {
            let id = commands(&name).ok_or_else(|| SceneError::Command(name.clone()))?;
            let name = Arc::<str>::from(name);
            let mut slot = CommandSlot::default();
            slot.commands.push((name, id));
            draft.change(node, NodeChange::Commands(Box::new(slot)));
            draft.change(node, NodeChange::Handler { kind: HandlerKind::Press, id: Some(id) });
            draft.change(node, NodeChange::Flags(NodeFlags::FOCUSABLE));
        }
        draft.place(parent, usize::MAX, node);
        for child in self.children {
            child.emit(node.into(), palette, commands, draft)?;
        }
        Ok(())
    }

    fn into_el<C>(self, palette: &BTreeMap<String, [u8; 4]>, commands: &mut impl FnMut(&str) -> Option<C>) -> Result<El, SceneError>
    where
        C: Fn(&mut WriteCx<'_>) + 'static,
    {
        let mut node = match self.content {
            Content::Box => el(),
            Content::Text { value } => text(value),
        }
        .push_styles(&self.style.resolve(palette)?.base);
        if let Some(id) = self.test_id {
            node = node.test_id(id);
        }
        if let Some(label) = self.label {
            node = node.label(label);
        }
        if let Some(role) = self.role {
            node = node.role(role);
        }
        if let Some(name) = self.press {
            let Some(command) = commands(&name) else { return Err(SceneError::Command(name)) };
            let command = Rc::new(command);
            let press = Rc::clone(&command);
            node = node.focusable().on_command(name, move |write| command(write)).on_press(move |write| press(write));
        }
        for child in self.children {
            node = node.child(child.into_el(palette, commands)?);
        }
        Ok(node)
    }
}

impl Style {
    fn resolve(self, palette: &BTreeMap<String, [u8; 4]>) -> Result<AuthoredStyle, SceneError> {
        let mut authored = AuthoredStyle::default();
        for (value, property) in [(self.width, StyleProp::Width as fn(Length) -> StyleProp), (self.height, StyleProp::Height)] {
            if let Some(value) = value {
                let length = match value {
                    Dimension::Pixels(px) => Length::Px(nonnegative(px)?),
                    Dimension::Fraction(value) => Length::Pct(nonnegative(value.fraction)?),
                };
                authored.base.push(property(length));
            }
        }
        if let Some(value) = self.padding {
            let value = Length::Px(nonnegative(value)?);
            authored.base.extend([
                StyleProp::PaddingTop(value),
                StyleProp::PaddingRight(value),
                StyleProp::PaddingBottom(value),
                StyleProp::PaddingLeft(value),
            ]);
        }
        if let Some(value) = self.background {
            authored.base.push(StyleProp::Background(Some(Fill::Solid(value.resolve(palette)?.into()))));
        }
        if let Some(value) = self.text {
            authored.base.push(StyleProp::TextColor(value.resolve(palette)?.into()));
        }
        if let Some(direction) = self.direction {
            let direction = match direction {
                Direction::Row => FlexDirection::Row,
                Direction::Column => FlexDirection::Column,
            };
            authored.base.push(StyleProp::Direction(direction));
        }
        Ok(authored)
    }
}

impl Paint {
    fn resolve(self, palette: &BTreeMap<String, [u8; 4]>) -> Result<Color, SceneError> {
        let rgba = match self {
            Self::Rgba(rgba) => rgba,
            Self::Palette(name) => palette.get(&name).copied().ok_or(SceneError::Color(name))?,
        };
        Ok(Color::new(rgba.map(|channel| f32::from(channel) / 255.0)))
    }
}

fn nonnegative(value: f32) -> Result<f32, SceneError> {
    if value.is_finite() && value >= 0.0 { Ok(value) } else { Err(SceneError::Length) }
}
