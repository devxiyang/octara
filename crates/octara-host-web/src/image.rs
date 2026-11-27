//! Direct RGBA uploads, with the old sample retained only while an upload is speculative.

use octara_model::geometry::{Align, Edges, Rect, Size};
use octara_model::paint::{CanvasOp, CanvasText, FillRule, LineCap, LineJoin, PathStyle, Sample};
use octara_model::style::Visual;
use octara_model::theme::{Fill, Theme};
use wasm_bindgen::{Clamped, JsCast, JsValue};
use web_sys::{CanvasRenderingContext2d, CanvasWindingRule, Document, HtmlCanvasElement, ImageData};

pub(crate) fn upload(canvas: &HtmlCanvasElement, sample: Option<&Sample>) -> Result<(), JsValue> {
    let frame = sample.map(Sample::frame);
    let data = match frame.filter(|frame| frame.width != 0 && frame.height != 0) {
        Some(frame) => Some(ImageData::new_with_u8_clamped_array_and_sh(Clamped(&frame.rgba), frame.width, frame.height)?),
        None => None,
    };
    let context = context(canvas)?;
    canvas.set_width(frame.filter(|frame| frame.height != 0).map_or(0, |frame| frame.width));
    canvas.set_height(frame.filter(|frame| frame.width != 0).map_or(0, |frame| frame.height));
    if let Some(data) = data {
        context.put_image_data(&data, 0.0, 0.0)?;
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct CanvasSize {
    pub area: Size,
    pub padding: Edges,
    pub scale: f32,
}

pub(crate) fn draw(
    document: &Document,
    operations: &[CanvasOp],
    samples: impl IntoIterator<Item = Sample>,
    size: CanvasSize,
    visual: Visual,
    theme: &Theme,
) -> Result<HtmlCanvasElement, JsValue> {
    let CanvasSize { area, padding, scale } = size;
    let canvas = crate::dom::html(document, "canvas")?.dyn_into::<HtmlCanvasElement>()?;
    canvas.set_width((area.width * scale).ceil() as u32);
    canvas.set_height((area.height * scale).ceil() as u32);
    canvas.set_attribute("aria-hidden", "true")?;
    canvas.set_attribute(
        "style",
        &format!(
            "display:block;position:absolute;pointer-events:none;left:{}px;top:{}px;width:{}px;height:{}px;",
            padding.left, padding.top, area.width, area.height
        ),
    )?;
    let context = context(&canvas)?;
    context.scale(scale.into(), scale.into())?;
    let mut samples = samples.into_iter();
    let mut clip_depth = 0;
    let mut source = None;
    for op in operations {
        match op {
            CanvasOp::Path { path, style, fill, transform } => {
                use octara_model::paint::PathCmd;

                context.save();
                context.transform(
                    transform.xx.into(),
                    transform.xy.into(),
                    transform.yx.into(),
                    transform.yy.into(),
                    transform.x.into(),
                    transform.y.into(),
                )?;
                context.begin_path();
                for command in path.commands() {
                    match command {
                        PathCmd::MoveTo(p) => context.move_to(p.x.into(), p.y.into()),
                        PathCmd::LineTo(p) => context.line_to(p.x.into(), p.y.into()),
                        PathCmd::QuadTo(a, b) => context.quadratic_curve_to(a.x.into(), a.y.into(), b.x.into(), b.y.into()),
                        PathCmd::CurveTo(a, b, c) => {
                            context.bezier_curve_to(a.x.into(), a.y.into(), b.x.into(), b.y.into(), c.x.into(), c.y.into());
                        }
                        PathCmd::Close => context.close_path(),
                    }
                }
                let color = match fill {
                    Fill::Solid(color) => Some(crate::css::color(color.resolve(theme).0)),
                    Fill::Linear(gradient) => {
                        let bounds = path.bounds().unwrap_or_default();
                        let (sin, cos) = gradient.angle.to_radians().sin_cos();
                        let span = (sin.abs() * bounds.w + cos.abs() * bounds.h) / 2.0;
                        let (x, y) = (bounds.x + bounds.w / 2.0, bounds.y + bounds.h / 2.0);
                        let (dx, dy) = (sin * span, -cos * span);
                        let fill = context.create_linear_gradient((x - dx).into(), (y - dy).into(), (x + dx).into(), (y + dy).into());
                        for stop in [gradient.from, gradient.to] {
                            if stop.percentage.is_finite() {
                                fill.add_color_stop(stop.percentage.clamp(0.0, 1.0), &crate::css::color(stop.color.resolve(theme).0))?;
                            }
                        }
                        context.set_fill_style_canvas_gradient(&fill);
                        context.set_stroke_style_canvas_gradient(&fill);
                        None
                    }
                };
                match style {
                    PathStyle::Fill(rule) => {
                        if let Some(color) = color {
                            context.set_fill_style_str(&color);
                        }
                        let winding = match rule {
                            FillRule::NonZero => CanvasWindingRule::Nonzero,
                            FillRule::EvenOdd => CanvasWindingRule::Evenodd,
                        };
                        context.fill_with_canvas_winding_rule(winding);
                    }
                    PathStyle::Stroke(stroke) => {
                        if let Some(color) = color {
                            context.set_stroke_style_str(&color);
                        }
                        context.set_line_width(stroke.width.into());
                        context.set_miter_limit(stroke.miter_limit.into());
                        context.set_line_dash_offset(stroke.dash_offset.into());
                        let dash: js_sys::Array = stroke.dash.iter().map(|&dash| JsValue::from_f64(dash.into())).collect();
                        context.set_line_dash(&dash)?;
                        context.set_line_cap(match stroke.cap {
                            LineCap::Butt => "butt",
                            LineCap::Round => "round",
                            LineCap::Square => "square",
                        });
                        context.set_line_join(match stroke.join {
                            LineJoin::Miter => "miter",
                            LineJoin::Round => "round",
                            LineJoin::Bevel => "bevel",
                        });
                        context.stroke();
                    }
                }
                context.restore();
            }
            CanvasOp::Image { rect, .. } => {
                let sample = samples.next().expect("one sample per recorded canvas image");
                let frame = sample.frame();
                if frame.width != 0 && frame.height != 0 && !rect.is_empty() {
                    if source.is_none() {
                        source = Some(crate::dom::html(document, "canvas")?.dyn_into::<HtmlCanvasElement>()?);
                    }
                    let source = source.as_ref().expect("scratch image canvas");
                    upload(source, Some(&sample))?;
                    context.draw_image_with_html_canvas_element_and_dw_and_dh(
                        source,
                        rect.x.into(),
                        rect.y.into(),
                        rect.w.into(),
                        rect.h.into(),
                    )?;
                }
            }
            CanvasOp::Text(text) => draw_text(&context, text, visual, theme)?,
            CanvasOp::PushClip { rect, radius } => {
                let Rect { x, y, w, h } = *rect;
                context.save();
                context.begin_path();
                if *radius > 0.0 {
                    context.round_rect_with_f64(x.into(), y.into(), w.into(), h.into(), (*radius).into())?;
                } else {
                    context.rect(x.into(), y.into(), w.into(), h.into());
                }
                context.clip();
                clip_depth += 1;
            }
            CanvasOp::PopClip if clip_depth != 0 => {
                context.restore();
                clip_depth -= 1;
            }
            CanvasOp::PopClip => {}
        }
    }
    Ok(canvas)
}

fn context(canvas: &HtmlCanvasElement) -> Result<CanvasRenderingContext2d, JsValue> {
    let context = canvas.get_context("2d")?.ok_or_else(|| JsValue::from_str("Could not create a 2D canvas"))?;
    Ok(context.dyn_into()?)
}

fn draw_text(context: &CanvasRenderingContext2d, text: &CanvasText, visual: Visual, theme: &Theme) -> Result<(), JsValue> {
    let size = text.size.map_or(visual.font_size, |size| size * visual.text_scale) * text.scale;
    let line_height = visual.line_height * (size / visual.font_size.max(1.0));
    let weight = text.weight.unwrap_or(visual.font_weight);
    let family = crate::css::font_family(visual.font_family);
    context.set_font(&format!("{weight} {size}px {family}"));
    let color = text.color.map_or(visual.text, |color| color.resolve(theme).0);
    context.set_fill_style_str(&crate::css::color(color));
    context.set_text_align(match text.align {
        Align::Start => "start",
        Align::Center => "center",
        Align::End => "end",
    });
    context.set_text_baseline("top");
    let mut lines: Vec<String> = Vec::new();
    for paragraph in text.text.split('\n') {
        if let Some(max_width) = text.max_width.filter(|width| width.is_finite() && *width > 0.0) {
            let mut line = String::new();
            for word in paragraph.split_inclusive(char::is_whitespace) {
                let previous = line.len();
                line.push_str(word);
                if previous != 0 && context.measure_text(line.trim_end())?.width() > f64::from(max_width) {
                    line.truncate(previous);
                    lines.push(std::mem::replace(&mut line, word.to_owned()));
                }
            }
            lines.push(line);
        } else {
            lines.push(paragraph.to_owned());
        }
    }
    let block = lines.len() as f32 * line_height;
    let top = text.at.y
        - match text.valign {
            Align::Start => 0.0,
            Align::Center => block / 2.0,
            Align::End => block,
        };
    for (index, line) in lines.iter().enumerate() {
        context.fill_text(line.trim_end(), text.at.x.into(), (top + index as f32 * line_height).into())?;
    }
    Ok(())
}
