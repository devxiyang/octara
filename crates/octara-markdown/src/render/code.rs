//! Streaming code lines and the default code block header and scroll surface.

use super::{BlockRef, Kind, PositionKey, RenderBlock, focus_outline, keyed_rows};
use crate::{
    MarkdownLocale,
    slots::{BlockSlot, SlotContent, SlotRecord},
    stream::StreamingEffects,
};
use octara::prelude::*;
use octara_headless::interactive;
use std::{sync::Arc, time::Duration};

#[derive(Clone)]
struct CodeSnapshot(BlockRef);

impl CodeSnapshot {
    fn parts(&self) -> (&str, &str, &str) {
        let Kind::Code { lang, info, text, .. } = &self.0.kind else { unreachable!("code snapshot requires a code block") };
        (lang, info, text)
    }
}

impl PartialEq for CodeSnapshot {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0 || self.parts() == other.parts()
    }
}

type CodeSource = ReadSignal<Option<CodeSnapshot>>;

fn code_source(build: &mut BuildCx<'_>, block: &RenderBlock) -> CodeSource {
    let signal = block.source;
    build.memo(move |r| {
        r.with(signal, |block| block.as_ref().filter(|block| matches!(block.kind, Kind::Code { .. })).cloned().map(CodeSnapshot))
    })
}

/// One keyed row per source line. Copy feedback is scoped to the mounted header.
pub fn default_code_block(build: &mut BuildCx<'_>, block: &RenderBlock) -> El {
    let code = code_source(build, block);
    #[cfg(feature = "highlight")]
    let highlighter = build.context::<Arc<dyn crate::highlight::CodeHighlighter>>();
    let mut cache = Lines::default();
    let lines = build.derive_list(move |r| {
        r.with(code, |code| {
            let Some(code) = code else { return Vec::new() };
            let (lang, _, text) = code.parts();
            cache.update(lang, text, || {
                #[cfg(feature = "highlight")]
                {
                    crate::highlight::highlight_code(highlighter.as_deref(), lang, text)
                }
                #[cfg(not(feature = "highlight"))]
                {
                    None
                }
            })
        })
    });
    let digits = build.memo(move |r| r.len(lines).max(1).ilog10() as usize + 1);
    let theme = block.context.theme;
    let locale = MarkdownLocale::get(build);
    let code_block = block.clone();
    let header = show(
        move |r| r.with(theme, |theme| theme.code_block.show_header || theme.code_block.show_copy),
        move |_| {
            let code_block = code_block.clone();
            row()
                .tw(tw!("items-center border-b py-1"))
                .select_none()
                .style_with(move |r, p| {
                    let s = r.with(theme, |t| t.code_block);
                    p.px(s.padding).bg(s.header_background).border_color(s.border_color).rounded_t(s.radius)
                })
                .child(show(
                    move |r| r.with(theme, |theme| theme.code_block.show_header),
                    move |_| {
                        text(move |r: &mut ReadCx<'_>| r.with(code, |code| code.as_ref().map(|c| c.parts().1).unwrap_or("").to_owned()))
                            .min_w(0)
                            .text_overflow(TextOverflow::Ellipsis)
                            .text_size(FontSizeToken::Xs)
                            .text_color(ColorToken::TextMuted)
                    },
                ))
                .child(spacer())
                .child(show(move |r| r.with(theme, |theme| theme.code_block.show_copy), move |b| code_copy(b, &code_block, code, locale)))
        },
    );
    let reveal = block.clone();
    let body = col()
        .min_w(0)
        .text_family(FontFamilyToken::Mono)
        .style_with(move |r, p| {
            let s = r.with(theme, |t| t.code_block);
            let p = p.p(s.padding).text_size(s.font_size);
            let p = if s.wrap_lines { p.text_wrap(TextWrap::Words) } else { p.overflow_x(Overflow::Scroll).text_wrap(TextWrap::None) };
            if let Some(max) = s.max_height { p.max_h(max).overflow_y(Overflow::Scroll) } else { p }
        })
        .child(each(lines, move |_, key, line| {
            let number = key.index + 1;
            let content = text(move |r: &mut ReadCx<'_>| r.try_with(line, |line| (*line.0).clone()).unwrap_or_default())
                .min_w(0)
                .style_with(move |r, p| p.shrink(if r.with(theme, |t| t.code_block.wrap_lines) { 1.0 } else { 0.0 }));
            let content = if reveal.context.effects == StreamingEffects::None {
                content
            } else {
                let reveal = reveal.clone();
                content.text_reveal(move |r: &mut ReadCx<'_>| {
                    let bytes = r.try_with(line, |value| value.0.as_str().len()).unwrap_or_default();
                    let settled = number < r.len(lines);
                    reveal.reveal(r, bytes, settled)
                })
            };
            row()
                .align(AlignItems::Start)
                .child(show(
                    move |r| r.with(theme, |t| t.code_block.line_numbers),
                    move |_| {
                        text(move |r: &mut ReadCx<'_>| format!("{number:>width$}", width = r.get(digits)))
                            .text_wrap(TextWrap::None)
                            .select_none()
                            .a11y_hidden()
                            .tabular_nums(true)
                            .text_align(TextAlign::Right)
                            .text_color(ColorToken::TextMuted)
                            .min_w(sp(6.0))
                            .pr(sp(2.0))
                            .shrink(0.0)
                    },
                ))
                .child(content)
        }));
    col()
        .min_w(0)
        .style_with(move |r, p| {
            let s = r.with(theme, |t| t.code_block);
            p.rounded(s.radius).border(1).border_color(s.border_color).bg(s.background)
        })
        .child(header)
        .child(body)
}

pub(super) struct CodeAction {
    record: ReadSignal<Option<SlotRecord>>,
    code: ReadSignal<Arc<str>>,
    is_complete: ReadSignal<bool>,
}

impl CodeAction {
    pub(super) fn new(build: &mut BuildCx<'_>, block: &RenderBlock) -> Self {
        let record = block.slot;
        let Some(BlockSlot::Code { code, is_complete, .. }) = build.sample(record).map(|slot| slot.mount(build, record)) else {
            return Self { record, code: build.signal("".into()).read_only(), is_complete: build.signal(false).read_only() };
        };
        Self { record, code, is_complete }
    }

    pub(super) fn slot(&self, w: &mut WriteCx<'_>) -> Option<BlockSlot> {
        let SlotRecord { id, placement, content: SlotContent::Code { info, .. } } = w.read(|r| r.get(self.record))? else {
            return None;
        };
        Some(BlockSlot::Code { id, placement, info, code: self.code, is_complete: self.is_complete })
    }
}

fn code_copy(build: &mut BuildCx<'_>, block: &RenderBlock, code: CodeSource, locale: MarkdownLocale) -> El {
    let reset = build.signal(None::<TaskId>);
    let owner = build.scope();
    let caption = move |r: &mut ReadCx<'_>| if r.get(reset).is_some() { locale.copied } else { locale.copy };
    let renderer = block.context.renderer.clone();
    let action = renderer.on_code_copy().is_some().then(|| CodeAction::new(build, block));
    focus_outline(interactive::button(text(caption)))
        .label(caption)
        .select_none()
        .text_size(FontSizeToken::Xs)
        .px(sp(2.0))
        .py(sp(1.0))
        .min_h(26.0)
        .rounded(RadiusToken::Sm)
        .when(State::HOVER, |p| p.bg(ColorToken::SurfaceHover))
        .when(State::PRESSED, |p| p.bg(ColorToken::SurfacePressed))
        .on_press(move |w| {
            let text = w.with(code, |code| code.as_ref().map(|code| code.parts().2.to_owned()).unwrap_or_default());
            let slot = action.as_ref().and_then(|action| action.slot(w));
            let renderer = renderer.clone();
            w.request_service(ServiceRequest::WriteClipboardText { text }, move |w, outcome| {
                if outcome != ServiceOutcome::ClipboardWritten {
                    return;
                }
                if let Some(slot) = &slot
                    && let Some(copied) = renderer.on_code_copy()
                {
                    copied(w, slot);
                }
                if let Some(task) = w.read(|r| r.get(reset)) {
                    w.cancel_task(task);
                }
                let task = w.spawn_in(owner, "Markdown copy confirmation", move |task| async move {
                    task.sleep(Duration::from_millis(1500)).await;
                    task.write(|w| w.set(reset, None));
                });
                w.set(reset, Some(task));
            });
        })
}

#[derive(Clone)]
struct CodeLine(Arc<Rich>);

impl PartialEq for CodeLine {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

/// Plain appends only repair the last line. Highlighted updates reuse any unchanged lines.
#[derive(Default)]
struct Lines {
    source: String,
    language: String,
    lines: Vec<CodeLine>,
}

impl Lines {
    fn update(&mut self, lang: &str, source: &str, highlight: impl FnOnce() -> Option<Vec<Rich>>) -> Vec<(PositionKey, CodeLine)> {
        if !self.lines.is_empty() && self.language == lang && self.source == source {
            return keyed_rows('l', self.lines.iter().cloned());
        }
        let language_changed = self.language != lang;
        if language_changed {
            self.language.clear();
            self.language.push_str(lang);
        }
        if let Some(lines) = highlight() {
            self.retain_highlighted(source, lines);
        } else {
            self.repair_plain(source, language_changed);
        }
        if !source.starts_with(&self.source) {
            self.source.clear();
        }
        self.source.push_str(&source[self.source.len()..]);
        keyed_rows('l', self.lines.iter().cloned())
    }

    fn repair_plain(&mut self, source: &str, language_changed: bool) {
        let appended = !language_changed && source.starts_with(&self.source);
        let (from, cut) =
            if appended { (self.lines.len().saturating_sub(1), self.source.rfind('\n').map_or(0, |at| at + 1)) } else { (0, 0) };
        let mut end = from;
        for line in source[cut..].split('\n') {
            let line = selectable_line(line);
            if language_changed || self.lines.get(end).is_none_or(|old| old.0.as_str() != line) {
                self.set(end, line.into());
            }
            end += 1;
        }
        self.lines.truncate(end);
    }

    fn retain_highlighted(&mut self, source: &str, lines: Vec<Rich>) {
        let mut lines = lines.into_iter();
        let mut len = 0;
        for line in source.split('\n').map(selectable_line) {
            let styled = lines.next().filter(|styled| styled.as_str() == line).unwrap_or_else(|| line.into());
            self.set(len, styled);
            len += 1;
        }
        self.lines.truncate(len);
    }

    fn set(&mut self, index: usize, line: Rich) {
        match self.lines.get_mut(index) {
            Some(old) if *old.0 == line => {}
            Some(old) => *old = CodeLine(Arc::new(line)),
            None => self.lines.push(CodeLine(Arc::new(line))),
        }
    }
}

fn selectable_line(line: &str) -> &str {
    // A space gives empty lines the same selectable height as the other lines.
    if line.is_empty() { " " } else { line }
}
