//! A release review workspace built from styled Kit widgets and an unstyled checkbox part.

use octara::prelude::*;
use octara_icons::{ARROW_LEFT, ARROW_RIGHT, PACKAGE_OPEN};
use octara_kit::headless::{
    Choice, Control, DataTableState, Selection, TreeViewState,
    data_table::{Column, ColumnWidth, SortMode},
    tree_view::TreeItem,
};
use octara_kit::{
    ButtonVariant, Size as KitSize, ToastOptions, ToastQueue, Tone, badge, button, command_palette, data_table, icon_button, text_area,
    toast_viewport, tree_view,
};
use std::fmt;

mod queue;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Route {
    Releases,
    Review(&'static str),
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Release {
    name: &'static str,
    platform: &'static str,
    owner: &'static str,
    status: Status,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Status {
    Ready,
    InReview,
    Blocked,
}

impl Status {
    fn label(self) -> &'static str {
        match self {
            Self::Ready => "Ready",
            Self::InReview => "In review",
            Self::Blocked => "Blocked",
        }
    }

    fn tone(self) -> Tone {
        match self {
            Self::Ready => Tone::Success,
            Self::InReview => Tone::Accent,
            Self::Blocked => Tone::Warning,
        }
    }
}

const RELEASES: [(&str, Release); 9] = {
    use Status::{Blocked, InReview, Ready};
    [
        ("studio", Release { name: "Studio 2.4", platform: "desktop", owner: "Mika", status: InReview }),
        ("reader", Release { name: "Reader 1.8", platform: "mobile", owner: "Sana", status: Ready }),
        ("portal", Release { name: "Portal 3.2", platform: "web", owner: "Jules", status: Blocked }),
        ("capture", Release { name: "Capture 1.2", platform: "desktop", owner: "Jules", status: Ready }),
        ("field-notes", Release { name: "Field Notes 2.0", platform: "mobile", owner: "Alex", status: InReview }),
        ("help-center", Release { name: "Help Center 1.6", platform: "web", owner: "Mika", status: Ready }),
        ("console", Release { name: "Console 2.3", platform: "desktop", owner: "Alex", status: InReview }),
        ("sync", Release { name: "Sync 1.1", platform: "mobile", owner: "Sana", status: Blocked }),
        ("billing", Release { name: "Billing 4.0", platform: "web", owner: "Jules", status: InReview }),
    ]
};

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum ReleaseColumn {
    Name,
    Owner,
    Status,
}

impl fmt::Display for ReleaseColumn {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Name => "release",
            Self::Owner => "owner",
            Self::Status => "status",
        })
    }
}

pub(super) fn view(b: &mut BuildCx<'_>) -> El {
    let nav = b.navigator(Route::Releases);
    let selected = b.signal(Some("studio"));
    let palette = b.signal(false);
    let toasts = ToastQueue::new(b);
    let commands = b.derive_list(move |r| {
        vec![
            ("open".into(), Choice::new("Open selected review").disabled(selected_review(r, nav, selected).is_none())),
            ("back".into(), Choice::new("Return to releases").disabled(!nav.can_go_back(r))),
            ("forward".into(), Choice::new("Resume review").disabled(!nav.can_go_forward(r))),
        ]
    });
    b.on_chord("mod+k".parse().expect("command palette shortcut"), move |w| w.set(palette, true));

    col()
        .test_id("release-workspace")
        .size(rel(1.0))
        .position(Position::Relative)
        .bg(ColorToken::Background)
        .rounded(RadiusToken::Xl)
        .p(sp(4.0))
        .gap(sp(3.0))
        .child(
            row()
                .wrap(FlexWrap::Wrap)
                .align(AlignItems::Center)
                .justify(JustifyContent::SpaceBetween)
                .gap(sp(3.0))
                .child(
                    row()
                        .align(AlignItems::Center)
                        .gap(sp(3.0))
                        .child(
                            el().size(44.0)
                                .rounded(RadiusToken::Lg)
                                .bg(ColorToken::Accent)
                                .align(AlignItems::Center)
                                .justify(JustifyContent::Center)
                                .child(icon(PACKAGE_OPEN.into()).size(24.0).text_color(ColorToken::AccentText).a11y_hidden()),
                        )
                        .child(
                            col()
                                .child(text("Release desk").role(Role::Heading).level(1).text_size(FontSizeToken::Xl).text_weight(650))
                                .child(text("Build reviews and sign-off").text_size(FontSizeToken::Sm).text_color(ColorToken::TextMuted)),
                        ),
                )
                .child(
                    row()
                        .align(AlignItems::Center)
                        .gap(sp(1.0))
                        .child(
                            icon_button(ARROW_LEFT, "Back")
                                .test_id("workspace-back")
                                .disabled(move |r: &mut ReadCx<'_>| !nav.can_go_back(r))
                                .on_press(move |w| {
                                    nav.pop(w);
                                }),
                        )
                        .child(
                            icon_button(ARROW_RIGHT, "Forward")
                                .test_id("workspace-forward")
                                .disabled(move |r: &mut ReadCx<'_>| !nav.can_go_forward(r))
                                .on_press(move |w| {
                                    nav.go(w, 1);
                                }),
                        )
                        .child(
                            button(
                                if cfg!(target_vendor = "apple") { "Commands  ⌘K" } else { "Commands  Ctrl+K" },
                                KitSize::Md,
                                ButtonVariant::Secondary,
                            )
                            .test_id("workspace-commands")
                            .label("Commands")
                            .on_press(move |w| w.set(palette, true)),
                        ),
                ),
        )
        .child(col().grow(1.0).min_h(0).overflow_y(Overflow::Scroll).child(route_stack(nav, RouteMount::KeepAlive, move |b, route| {
            match *route {
                Route::Releases => releases(b, nav, selected, toasts),
                Route::Review(key) => review(b, key, toasts),
            }
        })))
        .child(command_palette("workspace-palette", "Release commands", palette.into(), commands, move |w, command| {
            match command.as_str() {
                "open" => open_review(w, nav, selected),
                "back" => {
                    nav.pop(w);
                }
                "forward" => {
                    nav.go(w, 1);
                }
                _ => {}
            }
        }))
        .child(toast_viewport("workspace-toasts", toasts))
}

fn selected_review(r: &mut ReadCx<'_>, nav: Navigator<Route>, selected: Signal<Option<&'static str>>) -> Option<Route> {
    match nav.current(r) {
        Route::Releases => r.get(selected).map(Route::Review),
        Route::Review(_) => None,
    }
}

fn open_review(w: &mut WriteCx<'_>, nav: Navigator<Route>, selected: Signal<Option<&'static str>>) {
    if let Some(route) = w.read(|r| selected_review(r, nav, selected)) {
        nav.push(w, route);
    }
}

fn releases(b: &mut BuildCx<'_>, nav: Navigator<Route>, selected: Signal<Option<&'static str>>, toasts: ToastQueue<String>) -> El {
    let category = b.signal(Some("all"));
    let filtered = b.derive_list(move |r| filter_releases(r.get(category)).collect());
    let category_selection = b.binding(
        move |r| r.get(category),
        move |w, next| {
            w.set(category, next);
            let current = w.get(selected);
            if !filter_releases(next).any(|(key, _)| Some(key) == current) {
                w.set(selected, filter_releases(next).next().map(|(key, _)| key));
            }
        },
    );
    let platforms = b.list_from([
        ("all", TreeItem::new("All releases")),
        ("native", TreeItem::new("Native").under("all")),
        ("desktop", TreeItem::new("Desktop").under("native")),
        ("mobile", TreeItem::new("Mobile").under("native")),
        ("web", TreeItem::new("Web").under("all")),
    ]);
    let tree = TreeViewState::new(b, platforms, Control::Uncontrolled(vec!["all", "native"]), Selection::Single(category_selection));
    let table = DataTableState::new(
        b,
        [
            (ReleaseColumn::Name, Column::new("Release").sortable().resizable(ColumnWidth::new(124.0, 106.0, 240.0))),
            (ReleaseColumn::Owner, Column::new("Owner").sortable()),
            (ReleaseColumn::Status, Column::new("Status").sortable()),
        ],
        SortMode::Multiple,
        Control::Uncontrolled(Vec::new()),
        Some(Selection::Single(selected.into())),
    );
    let grid = data_table(
        b,
        table,
        filtered,
        Some(VirtualOptions { estimated_row_height: 36.0, overscan: 1, ..Default::default() }),
        |column, a: &Release, b| match column {
            ReleaseColumn::Name => a.name.cmp(b.name),
            ReleaseColumn::Owner => a.owner.cmp(b.owner),
            ReleaseColumn::Status => a.status.cmp(&b.status),
        },
        |b, column, key, item| match column {
            ReleaseColumn::Name => text(move |r: &mut ReadCx<'_>| r.try_with(item, |release| release.name).unwrap_or_default())
                .test_id(format!("release-{key}"))
                .text_overflow(TextOverflow::Ellipsis)
                .text_weight(600),
            ReleaseColumn::Owner => text(move |r: &mut ReadCx<'_>| r.try_with(item, |release| release.owner).unwrap_or_default())
                .text_color(ColorToken::TextMuted),
            ReleaseColumn::Status => {
                let status = b.read(|r| r.with(item, |release| release.status));
                row()
                    .align(AlignItems::Center)
                    .child(badge(status.label(), status.tone()).px(sp(1.0)).text_overflow(TextOverflow::Ellipsis))
            }
        },
    );
    let title = move |r: &mut ReadCx<'_>| match r.get(category) {
        Some("native") => "Native releases",
        Some("desktop") => "Desktop releases",
        Some("mobile") => "Mobile releases",
        Some("web") => "Web releases",
        _ => "All releases",
    };

    col()
        .gap(sp(3.0))
        .child(
            col()
                .gap(sp(3.0))
                .when(Breakpoint::Medium, |p| p.direction(FlexDirection::Row))
                .child(
                    panel()
                        .p(sp(3.0))
                        .gap(sp(2.0))
                        .when(Breakpoint::Medium, |p| p.w(168.0).shrink(0.0))
                        .child(subheading("Platforms"))
                        .child(tree_view(tree, None).label("Release platforms")),
                )
                .child(
                    panel()
                        .p(sp(3.0))
                        .gap(sp(2.0))
                        .grow(1.0)
                        .min_w(0.0)
                        .child(
                            row()
                                .align(AlignItems::Center)
                                .justify(JustifyContent::SpaceBetween)
                                .wrap(FlexWrap::Wrap)
                                .gap(sp(2.0))
                                .child(subheading(title).test_id("workspace-filter"))
                                .child(
                                    button("Open review", KitSize::Md, ButtonVariant::Primary)
                                        .test_id("workspace-review")
                                        .on_press(move |w| open_review(w, nav, selected)),
                                ),
                        )
                        .child(grid.test_id("workspace-releases").label("Releases").h(208.0)),
                ),
        )
        .child(queue::view(b, toasts))
}

fn filter_releases(category: Option<&'static str>) -> impl Iterator<Item = (&'static str, Release)> {
    RELEASES.into_iter().filter(move |(_, release)| match category {
        Some("native") => release.platform != "web",
        Some("all") | None => true,
        Some(platform) => release.platform == platform,
    })
}

fn review(b: &mut BuildCx<'_>, key: &'static str, toasts: ToastQueue<String>) -> El {
    let release = RELEASES.into_iter().find_map(|(id, release)| (id == key).then_some(release)).expect("reviewed release");
    let draft = b.signal(String::new());
    let saved = b.signal(String::new());
    panel()
        .test_id("release-review")
        .p(sp(4.0))
        .gap(sp(3.0))
        .child(
            row()
                .align(AlignItems::Center)
                .justify(JustifyContent::SpaceBetween)
                .gap(sp(2.0))
                .child(
                    text(release.name)
                        .test_id("review-release-name")
                        .role(Role::Heading)
                        .level(2)
                        .text_size(FontSizeToken::Xl)
                        .text_weight(650),
                )
                .child(badge(release.status.label(), release.status.tone())),
        )
        .child(text(format!("Owner · {}", release.owner)).text_size(FontSizeToken::Sm).text_color(ColorToken::TextMuted))
        .child(subheading("Review note").id("review-draft-label"))
        .child(
            text_area(draft.into())
                .test_id("review-draft")
                .labelled_by("review-draft-label")
                .placeholder("Findings, blockers, or sign-off…")
                .min_h(112.0),
        )
        .child(
            button("Save note", KitSize::Md, ButtonVariant::Primary)
                .test_id("review-save-note")
                .align_self(AlignItems::Start)
                .disabled(move |r: &mut ReadCx<'_>| r.get(draft).trim() == r.get(saved))
                .on_press(move |w| {
                    let value = w.get(draft).trim().to_owned();
                    w.set(saved, value);
                    toasts.push(w, format!("Saved note for {}", release.name), ToastOptions::default());
                }),
        )
}

fn panel() -> El {
    col().bg(ColorToken::Surface).rounded(RadiusToken::Lg).border(1).border_color(ColorToken::Border)
}

fn subheading(title: impl IntoText) -> El {
    text(title).role(Role::Heading).level(2).text_weight(600)
}
