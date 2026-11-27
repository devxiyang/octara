//! A row-major table that derives shared column widths from intrinsic cell geometry.

use super::{Kind, RenderBlock, inline_view, keyed_rows};
use crate::ast::Align;
use octara::prelude::*;
use std::collections::BTreeMap;

/// A scrollable, row-major table with shared column widths measured from unwrapped cells.
pub fn default_table(build: &mut BuildCx<'_>, block: &RenderBlock) -> El {
    let source = block.clone();
    let theme = block.context.theme;
    let rows = build.derive_list(move |r| {
        source.project(r, |block| match &block.kind {
            Kind::Table { head, rows, align } => {
                let width = head.len().max(rows.iter().map(Vec::len).max().unwrap_or(0));
                keyed_rows(
                    'r',
                    std::iter::once(head).chain(rows).map(|row| {
                        (0..width)
                            .map(|j| (row.get(j).cloned().unwrap_or_default(), align.get(j).copied().unwrap_or(Align::None)))
                            .collect::<Vec<_>>()
                    }),
                )
            }
            _ => Vec::new(),
        })
    });
    let measured = build.signal(BTreeMap::<(usize, usize), f32>::new());
    let source = block.clone();
    let dimensions = build.memo(move |r| {
        source.project(r, |block| match &block.kind {
            Kind::Table { head, rows, .. } => (rows.len() + 1, head.len().max(rows.iter().map(Vec::len).max().unwrap_or(0))),
            _ => (0, 0),
        })
    });
    build.watch(
        move |r| r.get(dimensions),
        move |w, &(rows, cols), _| {
            w.mutate(measured, |widths| {
                let before = widths.len();
                widths.retain(|&(i, j), _| i < rows && j < cols);
                widths.len() != before
            });
        },
    );
    let mut next_widths = Vec::new();
    let widths = build.memo_with(Vec::new(), move |r, widths| {
        r.with2(dimensions, measured, |&(rows, cols), measured| {
            next_widths.resize(cols, 0.0_f32);
            next_widths.fill(0.0);
            for (&(i, j), &w) in measured {
                if i < rows && j < cols {
                    next_widths[j] = next_widths[j].max(w);
                }
            }
        });
        if *widths == next_widths {
            return false;
        }
        std::mem::swap(widths, &mut next_widths);
        true
    });
    let block = block.clone();
    col()
        .role(Role::Table)
        .min_w(0)
        .overflow_x(Overflow::Scroll)
        .style_with(move |r, p| {
            let s = r.with(theme, |t| t.table);
            p.rounded(s.radius).border(1).border_color(s.border_color)
        })
        .child(each(rows, move |b, key, cells| {
            let i = key.index;
            let cells = b.derive_list(move |r| r.try_with(cells, |cells| keyed_rows('c', cells.iter().cloned())).unwrap_or_default());
            let block = block.clone();
            let line = row().role(Role::Row).style_with(move |r, p| {
                let s = r.with(theme, |t| t.table);
                let color = match i {
                    0 => s.header_background,
                    n if n.is_multiple_of(2) => s.striped_background.unwrap_or(s.row_background),
                    _ => s.row_background,
                };
                p.bg(color).border_b(1).border_color(s.border_color)
            });
            let line = if i == 0 { line.text_weight(600) } else { line };
            line.child(each(cells, move |b, key, cell| {
                let j = key.index;
                let cell = b.memo(move |r| r.try_get(cell));
                let geometry = b.signal(None::<LayoutBox>);
                b.watch(
                    move |r| r.get(geometry).map(|layout| layout.rect.w),
                    move |w, width, _| {
                        if let Some(width) = *width {
                            w.mutate(measured, |widths| widths.insert((i, j), width) != Some(width));
                        }
                    },
                );
                let block = block.clone();
                row()
                    .role(if i == 0 { Role::ColumnHeader } else { Role::Cell })
                    .shrink(0.0)
                    .style_with(move |r, p| {
                        let w = r.with(widths, |widths| widths.get(j).copied().unwrap_or_default());
                        let a = r.with(cell, |cell| cell.as_ref().map(|(_, align)| *align));
                        let p = if w > 0.0 { p.min_w(w) } else { p };
                        p.justify(match a {
                            Some(Align::Center) => JustifyContent::Center,
                            Some(Align::Right) => JustifyContent::End,
                            _ => JustifyContent::Start,
                        })
                    })
                    .child(
                        row()
                            .shrink(0.0)
                            .observe_geometry(geometry)
                            .style_with(move |r, p| {
                                let padding = r.with(theme, |t| t.table.cell_padding);
                                p.pt(padding.top).pr(padding.right).pb(padding.bottom).pl(padding.left)
                            })
                            .child(
                                inline_view(
                                    b,
                                    &block,
                                    move |r| i * r.get(dimensions).1 + j,
                                    move |r| r.with(cell, |cell| cell.as_ref().map(|(inline, _)| inline.clone()).unwrap_or_default()),
                                )
                                .text_wrap(TextWrap::None)
                                .shrink(0.0),
                            ),
                    )
            }))
        }))
}
