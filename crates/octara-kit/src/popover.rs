//! Styled nonmodal popup surfaces.

use crate::parts::popup_surface;
use octara::prelude::*;
use octara_headless::PopoverState;

/// Placement and sizing shared by Kit's anchored popups.
#[derive(Clone, Debug, PartialEq)]
pub struct PopupOptions {
    pub placement: Placement,
    pub anchor_target: Option<AnchorTarget>,
    pub width: Option<Length>,
    pub min_width: Option<Length>,
    pub max_height: Option<Length>,
    pub match_trigger_width: bool,
    pub surface_style: Option<StylePatch>,
}

impl Default for PopupOptions {
    fn default() -> Self {
        Self {
            placement: Placement::BELOW,
            anchor_target: None,
            width: None,
            min_width: None,
            max_height: None,
            match_trigger_width: false,
            surface_style: None,
        }
    }
}

impl PopupOptions {
    pub fn placement(mut self, placement: Placement) -> Self {
        self.placement = placement;
        self
    }

    pub fn anchor_target(mut self, target: AnchorTarget) -> Self {
        self.anchor_target = Some(target);
        self
    }

    pub fn width(mut self, width: impl Into<Length>) -> Self {
        self.width = Some(width.into());
        self
    }

    pub fn min_width(mut self, width: impl Into<Length>) -> Self {
        self.min_width = Some(width.into());
        self
    }

    pub fn max_height(mut self, height: impl Into<Length>) -> Self {
        self.max_height = Some(height.into());
        self
    }

    /// Follows the trigger's measured width, subject to explicit size limits.
    pub fn match_trigger_width(mut self, enabled: bool) -> Self {
        self.match_trigger_width = enabled;
        self
    }

    /// Applies a style patch after the surface defaults and sizing.
    pub fn surface_style(mut self, style: StylePatch) -> Self {
        self.surface_style = Some(style);
        self
    }

    pub(crate) fn anchor<C>(&self, b: &mut BuildCx<'_>, trigger: El<C>) -> (El<C>, Option<ReadSignal<Option<f32>>>) {
        if !self.match_trigger_width {
            return (trigger, None);
        }
        let geometry = b.signal(None::<LayoutBox>);
        let width = b.memo(move |r| r.get(geometry).map(|bounds| bounds.rect.w));
        (trigger.observe_geometry(geometry), Some(width))
    }

    pub(crate) fn surface<C>(&self, mut element: El<C>, trigger_width: Option<ReadSignal<Option<f32>>>) -> El<C> {
        if self.width.is_some() || trigger_width.is_some() {
            element = element.min_w(Length::Auto);
        }
        if let Some(width) = self.width {
            element = element.w(width);
        }
        if let Some(trigger_width) = trigger_width {
            element = element.style_with(move |r, p| match r.get(trigger_width) {
                Some(width) => p.w(width),
                None => p,
            });
        }
        if let Some(width) = self.min_width {
            element = element.min_w(width);
        }
        if let Some(height) = self.max_height {
            element = element.max_h(height).scroll_y();
        }
        apply_style(element, self.surface_style.as_ref())
    }
}

pub(crate) fn apply_style<C>(element: El<C>, style: Option<&StylePatch>) -> El<C> {
    match style {
        Some(style) => element.style_patch(style),
        None => element,
    }
}

/// A themed nonmodal popup anchored to a caller-authored button.
/// The trigger uses `id` and the panel uses `{id}-popover`.
#[ui]
pub fn popover<V: IntoView + 'static>(
    b: &mut BuildCx<'_>,
    id: &str,
    open: Binding<bool>,
    options: PopupOptions,
    trigger: El,
    mut content: impl FnMut(&mut BuildCx<'_>) -> V + 'static,
) -> El {
    let state = PopoverState::controlled(open).focus_trap(FocusTrapFeatures::INITIAL_FOCUS | FocusTrapFeatures::RESTORE_FOCUS);
    let (trigger, geometry) = options.anchor(b, state.button(trigger.test_id(id.to_owned())));
    let panel_id = format!("{id}-popover");
    state.overlay(trigger, options.placement, options.anchor_target, move |b| {
        state.panel(options.surface(popup_surface(col()).test_id(panel_id.clone()), geometry).child(content(b)))
    })
}
