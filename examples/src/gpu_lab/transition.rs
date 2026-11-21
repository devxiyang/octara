use super::{ACCENT, BORDER, PANEL, TEXT, action};
use octara::prelude::*;
use octara_icons as icons;
use octara_render::{
    gpu::{CanvasFrame, Transition, transition},
    wgpu,
};
use wgpu::util::DeviceExt;

#[ui]
pub(super) fn panel() -> El {
    let details = state("show-details", || false);
    let refreshed = state("refreshed", || false);

    col()
        .gap(8)
        .child(
            row()
                .gap(8)
                .child(action("gpu-transition-switch", "Switch content", icons::LAYERS).on_press(move || details.update(bool::toggle)))
                .child(
                    action("gpu-transition-refresh", "Refresh color", icons::ROTATE_CCW).on_press(move || refreshed.update(bool::toggle)),
                ),
        )
        .child(
            col().h(84).rounded(12.0).overflow_clip().border().border_color(BORDER).bg(PANEL).child(
                col()
                    .id("gpu-transition-region")
                    .size_full()
                    .p(8)
                    .paint_transition(program(), move || u64::from(details.get()), Curve::Fade(4.0))
                    .child(keyed(
                        move || details.get(),
                        move |details| {
                            let details = *details;
                            let color = move || match (details, refreshed.get()) {
                                (false, _) => rgb(0xcc4e59),
                                (true, false) => rgb(0x437cea),
                                (true, true) => rgb(0x29c690),
                            };
                            row().h_full().items_center().child(el().w(if details { 76.0 } else { 0.0 }).flex_shrink_0()).child(
                                el().id("gpu-transition-content")
                                    .role(Role::Button)
                                    .label(if details { "Pass details" } else { "Flow preview" })
                                    .focusable()
                                    .h(56)
                                    .w(166)
                                    .rounded(6.0)
                                    .items_center()
                                    .justify_center()
                                    .bg(color)
                                    .text_color(TEXT)
                                    .border()
                                    .focus_visible(|style| style.border_color(ACCENT))
                                    .on_press(move || refreshed.update(bool::toggle))
                                    .child(text(if details { "Pass details" } else { "Flow preview" }).text_size(13)),
                            )
                        },
                    )),
            ),
        )
}

fn program() -> PaintProgram {
    transition(|device| {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("dialog transition"),
            source: wgpu::ShaderSource::Wgsl(include_str!("transition.wgsl").into()),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("dialog transition"),
            layout: None,
            module: &shader,
            entry_point: Some("crossfade"),
            compilation_options: Default::default(),
            cache: None,
        });
        let layout = pipeline.get_bind_group_layout(0);
        move |frame: &mut CanvasFrame<'_>, inputs: Transition<'_>| {
            let [old_x, old_y] = inputs.old.origin;
            let [new_x, new_y] = inputs.new.origin;
            let values = [
                frame.rect.x,
                frame.rect.y,
                frame.scale,
                inputs.progress,
                old_x as f32,
                old_y as f32,
                inputs.old.scale,
                0.0,
                new_x as f32,
                new_y as f32,
                inputs.new.scale,
                0.0,
            ];
            let bytes = values.map(f32::to_ne_bytes);
            let uniforms = frame.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("dialog transition coordinates"),
                contents: bytes.as_flattened(),
                usage: wgpu::BufferUsages::UNIFORM,
            });
            let textures = frame.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("dialog transition inputs"),
                layout: &layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: uniforms.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(inputs.old.view) },
                    wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(inputs.new.view) },
                    wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(frame.target) },
                ],
            });
            let [width, height] = frame.size;
            let mut pass = frame.begin_compute_pass("dialog_transition", &Default::default());
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &textures, &[]);
            pass.dispatch_workgroups(width.div_ceil(8), height.div_ceil(8), 1);
        }
    })
}
