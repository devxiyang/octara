//! The shared window overlay for authored hover tooltips.

use super::control::{OverlayBinding, mount_portal, next_region};
use super::{El, NodeHandle, Styled, el, px, sp, text};
use octara_model::{
    geometry::Placement,
    overlay::AnchorTarget,
    semantics::Role,
    theme::{ColorToken, FontSizeToken, RadiusToken},
};
use octara_runtime::{BuildCx, HoveredTooltip, MountCx, ScopeId};
use octara_tree::{NodeFlags, NodeRef, Retirement, transaction::OverlaySpec};

type RenderTooltip = Box<dyn FnMut(&mut BuildCx<'_>, &HoveredTooltip) -> El>;

/// A window's shared tooltip renderer. The portal inherits from the hovered node.
pub struct WindowTooltipLayer(RenderTooltip);

impl Default for WindowTooltipLayer {
    fn default() -> Self {
        Self::new(|_, tip| {
            el().max_w(px(280.0))
                .px(sp(2.0))
                .py(sp(1.0))
                .rounded(RadiusToken::Md)
                .border(px(1.0))
                .border_color(ColorToken::Border)
                .bg(ColorToken::SurfaceElevated)
                .child(text(tip.text.clone()).text_color(ColorToken::Text).text_size(FontSizeToken::Sm))
        })
    }
}

impl WindowTooltipLayer {
    pub fn new(render: impl FnMut(&mut BuildCx<'_>, &HoveredTooltip) -> El + 'static) -> Self {
        Self(Box::new(render))
    }

    pub(crate) fn mount(self, cx: &mut MountCx<'_>) {
        let Self(mut render) = self;
        let tooltip = cx.build(|build| build.hovered_tooltip());
        let order = next_region();
        let mut active: Option<(ScopeId, NodeHandle)> = None;
        cx.structure(
            move |read| read.get(tooltip),
            move |cx, tip| {
                if let Some((scope, handle)) = active.take() {
                    if let NodeRef::Existing { id, .. } = handle.get()
                        && let Some(node) = cx.live_node(id)
                    {
                        cx.retire(node, Retirement::Destroy);
                    }
                    cx.retire_scope(scope);
                }
                let Some(tip) = tip else { return };
                let Some(owner) = cx.live_node(tip.node) else { return };
                active = Some(cx.with_scope(|cx| {
                    let view = cx.build(|build| render(build, tip)).role(Role::Tooltip).flags(NodeFlags::INERT).select_none();
                    let overlay = OverlayBinding {
                        spec: OverlaySpec {
                            target: Some((AnchorTarget::Node(owner), Placement::ABOVE.fit(true))),
                            mount: Some((owner, order)),
                            ..OverlaySpec::default()
                        },
                        target: None,
                    };
                    mount_portal(cx, view, None, overlay)
                }));
            },
        );
    }
}
