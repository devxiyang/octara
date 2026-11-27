//! Rich document views and fixtures for the offline transit answer.

use octara::prelude::*;
use octara_icons as icons;
use octara_kit::{DialogOptions, collapsible, dialog, icon_button};
use octara_markdown::{
    BlockSlot, BlockSlotRenderer, LinkPillContent, LinkVariantRule, LinkVariantRules, MarkdownConfig, MarkdownRenderer, MarkdownTheme,
    StreamingEffects, markdown,
};
use std::sync::{Arc, OnceLock};

pub(crate) const TRANSIT_PROMPT: &str = "How does an exoplanet transit reveal the planet’s size?";
pub(crate) const TRANSIT_ANSWER: &str = r#":::think
Use a simple transit model, then show how the same measurement appears in a diagram, a light curve and a short calculation.
:::

## Measuring an exoplanet

When a planet passes in front of its star, a telescope measures a small dip in the star’s light. The [archive](https://exoplanetarchive.ipac.caltech.edu/) publishes measurements from real systems. The values below are illustrative.

![A planet crossing the face of a star](transit:crossing "Transit geometry")

The planet’s silhouette ![silhouette](transit:silhouette) blocks a fraction $\delta$ of the light. For a uniform stellar disk, the depth is the ratio of the two projected areas.

$$
\delta \approx \left(\frac{R_p}{R_\star}\right)^2
$$

Here a dip of **1%** suggests the planet’s radius is **one tenth** of the star’s radius.

```transit-chart
Before,100
Ingress,99.6
Midpoint,99
Egress,99.6
After,100
```

```rust transit-depth
fn planet_to_star_radius(depth: f64) -> f64 {
    depth.sqrt()
}
```

Repeated dips reveal the orbital period. A single dip gives a size estimate if you already know the star’s radius.
"#;

const ARCHIVE: &str = "https://exoplanetarchive.ipac.caltech.edu/";

pub(crate) const REVEAL: StreamingEffects =
    StreamingEffects::Gradient { trail_bytes: 22, tint: Some(ColorValue::Token(ColorToken::Accent)), glide_bytes_per_sec: 120.0 };

/// Shares presentation across virtual message rows while they come into and out of view.
pub(crate) fn transcript(b: &mut BuildCx<'_>, compact: ReadSignal<bool>) -> View {
    let selected = b.signal(None::<(Arc<str>, Arc<str>)>);
    let open = b.binding(
        move |r| r.with(selected, Option::is_some),
        move |w, open| {
            if !open {
                w.set(selected, None);
            }
        },
    );
    let config = MarkdownConfig::chat()
        .theme_with(move |r| if r.get(compact) { MarkdownTheme::compact() } else { MarkdownTheme::chat() })
        .link_variant_rules(
            LinkVariantRules::new([
                LinkVariantRule::new(ARCHIVE, pill_style(ColorToken::Accent)),
                LinkVariantRule::new("user://", pill_style(ColorToken::Info)),
            ])
            .expect("fixed link patterns fit a DFA"),
        )
        .link_pill_content(move |r, url| match url {
            ARCHIVE => Some(LinkPillContent {
                label: Some(if r.get(compact) { "Exoplanet Archive" } else { "NASA Exoplanet Archive" }.into()),
                icon: Some(icons::TELESCOPE.into()),
                tint: Some(ColorToken::Accent.into()),
            }),
            _ if url.starts_with("user://") => Some(LinkPillContent { icon: Some(icons::USERS.into()), ..LinkPillContent::default() }),
            _ => None,
        })
        .on_image_press(move |w, slot| {
            if let BlockSlot::Image { url, alt, .. } = slot {
                w.set(selected, Some((url.clone(), alt.clone())));
            }
        });
    #[allow(clippy::arc_with_non_send_sync)]
    let renderer: Arc<dyn MarkdownRenderer> = Arc::new(config);
    b.provide(renderer);
    b.provide::<Arc<dyn BlockSlotRenderer>>(Arc::new(TranscriptSlots));
    b.provide::<Arc<dyn MediaResolver>>(Arc::new(TransitImages));
    dialog("image-preview", "Image preview", open, DialogOptions::default(), move |b| {
        let Some((url, alt)) = b.sample(selected) else { return col() };
        col()
            .gap(sp(2.0))
            .child(image_source(ImageSource::Uri(url)).w(rel(1.0)).object_fit(ObjectFit::Contain).label(alt.as_ref()))
            .child(text(alt.to_string()).text_size(FontSizeToken::Sm).text_color(ColorToken::TextMuted))
    })
}

fn pill_style(color: ColorToken) -> Span {
    Span {
        color: Some(color.into()),
        background: Some(ColorToken::Surface.into()),
        padding_x: Some(5.0),
        padding_y: Some(2.0),
        radius: Some(5.0),
        ..Span::default()
    }
}

struct TranscriptSlots;

impl BlockSlotRenderer for TranscriptSlots {
    fn render_slot(&self, b: &mut BuildCx<'_>, slot: &BlockSlot) -> Option<El> {
        match slot {
            BlockSlot::Directive { id, name, body, is_complete, .. } if name.as_ref() == "think" => {
                let open = b.signal(false);
                let (body, complete) = (*body, *is_complete);
                Some(collapsible(
                    b,
                    &format!("think-{id}"),
                    open.into(),
                    text(move |r: &mut ReadCx<'_>| if r.get(complete) { "Analysis notes" } else { "Analyzing…" }),
                    move |b| markdown(b, move |r: &mut ReadCx<'_>| r.with(body, |body| body.to_string())).p(sp(2.0)),
                ))
            }
            BlockSlot::Code { info, code, is_complete, .. } if info.lang.as_ref() == "transit-chart" => {
                Some(transit_chart(b, *code, *is_complete))
            }
            BlockSlot::Code { info, code, is_complete, .. } if info.info.as_ref() == "rust transit-depth" => {
                Some(transit_code(*code, *is_complete))
            }
            BlockSlot::Image { url, alt, .. } if url.as_ref() == "transit:crossing" => Some(
                image_source(ImageSource::Uri(url.clone()))
                    .w(360.0)
                    .max_w(rel(1.0))
                    .aspect_ratio(600.0 / 222.0)
                    .object_fit(ObjectFit::Contain)
                    .rounded(RadiusToken::Md)
                    .label(alt.as_ref()),
            ),
            _ => None,
        }
    }
}

fn transit_code(code: ReadSignal<Arc<str>>, complete: ReadSignal<bool>) -> El {
    let copy = icon_button(icons::COPY, "Copy Rust calculation")
        .test_id("copy-transit-code")
        .disabled(move |r: &mut ReadCx<'_>| !r.get(complete))
        .on_press(move |w| {
            let text = w.with(code, |code| code.to_string());
            w.request_service(ServiceRequest::WriteClipboard { items: vec![ClipboardItem::Text(text)] }, |_, _| {});
        });
    panel("Radius calculation · Rust", icons::CODE, copy).child(
        text(move |r: &mut ReadCx<'_>| r.with(code, |code| code.to_string()))
            .text_family(FontFamilyToken::Mono)
            .text_size(FontSizeToken::Sm)
            .text_reveal(move |r: &mut ReadCx<'_>| REVEAL.reveal(r.with(code, |code| code.len()), r.get(complete)))
            .test_id("transit-rust"),
    )
}

fn transit_chart(b: &mut BuildCx<'_>, code: ReadSignal<Arc<str>>, complete: ReadSignal<bool>) -> El {
    let points = b.memo(move |r| {
        let complete = r.get(complete);
        r.with(code, |code| {
            let rows = if complete { code.as_ref() } else { code.rsplit_once('\n').map_or("", |(rows, _)| rows) };
            rows.lines()
                .filter_map(|row| {
                    let (stage, flux) = row.split_once(',')?;
                    let flux = flux.trim().parse::<f32>().ok().filter(|flux| (98.5..=100.0).contains(flux))?;
                    Some((stage.trim().to_owned(), flux))
                })
                .take(8)
                .collect::<Vec<_>>()
        })
    });
    let chart = canvas(move |r, c| {
        let points = r.get(points);
        let width = c.width();
        let height = c.height();
        let slot = width / points.len().max(5) as f32;
        let y = |flux: f32| 16.0 + (100.0 - flux) / 1.5 * (height - 40.0);
        for flux in [99.0, 99.5, 100.0] {
            c.rect(Rect::new(0.0, y(flux), width, 1.0), ColorToken::Border);
        }
        let line: Vec<_> = points.iter().enumerate().map(|(index, (_, flux))| point(slot * (index as f32 + 0.5), y(*flux))).collect();
        if line.len() > 1 {
            c.stroke(&Path::new().add_polygon(&line, false), &Stroke::new(2.5), ColorToken::Accent);
        }
        for ((stage, flux), at) in points.iter().zip(line) {
            c.fill(&Path::rounded_rect(Rect::new(at.x - 3.0, at.y - 3.0, 6.0, 6.0), 3.0), ColorToken::Accent);
            c.text(format!("{flux}%"), (at.x, at.y - 5.0)).align(Align::Center).valign(Align::End).size(11.0);
            c.text(stage, (at.x, height)).align(Align::Center).valign(Align::End).size(11.0);
        }
    })
    .test_id("transit-chart")
    .w(rel(1.0))
    .h(104.0)
    .text_color(ColorToken::TextMuted)
    .role(Role::Image)
    .label(move |r: &mut ReadCx<'_>| {
        let mut label = String::from("Relative stellar brightness");
        for (stage, flux) in r.get(points) {
            label.push_str(&format!(". {stage} {flux} percent"));
        }
        label
    });
    panel("Stellar brightness during transit", icons::CHART_COLUMN, el()).child(chart)
}

fn panel(title: &str, symbol: icons::LucideIcon, action: El) -> El {
    col().gap(sp(2.0)).p(sp(3.0)).rounded(RadiusToken::Lg).border(1.0).border_color(ColorToken::Border).bg(ColorToken::Surface).child(
        row()
            .align(AlignItems::Center)
            .gap(sp(2.0))
            .child(icon(symbol.into()).size(16.0).text_color(ColorToken::Accent).a11y_hidden())
            .child(text(title).grow(1.0).text_weight(600).text_size(FontSizeToken::Sm))
            .child(action),
    )
}

struct TransitImages;

impl MediaResolver for TransitImages {
    fn resolve(&self, uri: &str) -> Option<ImageSource> {
        static CROSSING: OnceLock<Image> = OnceLock::new();
        static SILHOUETTE: OnceLock<Image> = OnceLock::new();
        let image = match uri {
            "transit:crossing" => CROSSING.get_or_init(|| transit_image(600, 222)),
            "transit:silhouette" => SILHOUETTE.get_or_init(|| transit_image(64, 64)),
            _ => return None,
        };
        Some(ImageSource::Pixels(image.clone()))
    }
}

fn transit_image(width: u32, height: u32) -> Image {
    let pixels: Vec<_> = (0..height)
        .flat_map(|y| {
            (0..width).flat_map(move |x| {
                // Use the same field of view for the thumbnail and the wide illustration.
                let u = (x as f32 - width as f32 / 2.0) / height as f32;
                let v = y as f32 / height as f32 - 0.5;
                let star = (u * u + v * v).sqrt();
                let planet = ((u - 0.11).powi(2) + (v + 0.06).powi(2)).sqrt();
                let speck = (x.wrapping_mul(73) ^ y.wrapping_mul(151)) % 928 < 3;
                let glow = (0.55 - star).max(0.0);
                if planet < 0.09 {
                    [16, 19, 32, 255]
                } else if star < 0.33 {
                    let light = (0.33 - star) / 0.33;
                    [255, (152.0 + 93.0 * light) as u8, (55.0 + 134.0 * light) as u8, 255]
                } else if speck && glow < 0.04 {
                    [154, 172, 205, 255]
                } else {
                    [(12.0 + 170.0 * glow) as u8, (19.0 + 60.0 * glow) as u8, (38.0 + 26.0 * glow) as u8, 255]
                }
            })
        })
        .collect();
    Image::rgba(width, height, pixels)
}
