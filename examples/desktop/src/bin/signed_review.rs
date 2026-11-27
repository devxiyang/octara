//! A native review form with a signed, application-defined presentation.

#[cfg(target_os = "macos")]
mod macos {
    use objc2_foundation::{NSFileManager, NSSearchPathDirectory, NSSearchPathDomainMask};
    use octara::prelude::*;
    use octara_examples::window_options;
    use octara_kit::{ButtonVariant, Size as KitSize};
    use octara_release::{Channel, Check, Policy, ReleaseTarget, Session, Staged, Startup, Store, public_key};
    use serde::{Deserialize, Serialize};
    use std::fs::{self, File};
    use std::io::{self, Read, Write};
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    const APP_ID: &str = "dev.octara.signed-review";
    const STATE_SCHEMA: &str = "review.draft/1";
    const DRAFT_LIMIT: usize = 1024 * 1024;

    #[derive(Clone, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Presentation {
        title: String,
        accent: Accent,
        sections: [Section; 2],
    }

    #[derive(Clone, Copy, Deserialize)]
    #[serde(rename_all = "snake_case")]
    enum Accent {
        Jade,
        Violet,
    }

    #[derive(Clone, Copy, Deserialize, PartialEq, Eq)]
    #[serde(rename_all = "snake_case")]
    enum Section {
        Editor,
        Preview,
    }

    impl Presentation {
        fn embedded() -> Self {
            Self { title: "Release review".into(), accent: Accent::Jade, sections: [Section::Editor, Section::Preview] }
        }

        fn prepare(raw: &str) -> io::Result<Self> {
            let value: Self = serde_json::from_str(raw)?;
            if value.title.trim().is_empty() || value.title.chars().count() > 80 || value.title.chars().any(char::is_control) {
                return Err(invalid("the review title must contain 1–80 visible characters"));
            }
            if value.sections[0] == value.sections[1] {
                return Err(invalid("each registered review section must appear once"));
            }
            Ok(value)
        }

        fn theme(&self) -> Theme {
            let accent = match self.accent {
                Accent::Jade => rgb(0x176345),
                Accent::Violet => rgb(0x5a43a4),
            };
            Theme::light().color(ColorToken::Accent, accent)
        }
    }

    #[derive(Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Draft {
        schema: String,
        text: String,
    }

    impl Draft {
        fn load(path: &Path) -> io::Result<String> {
            let file = match File::open(path) {
                Ok(file) => file,
                Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(String::new()),
                Err(error) => return Err(error),
            };
            let mut bytes = Vec::new();
            file.take(DRAFT_LIMIT as u64 + 1).read_to_end(&mut bytes)?;
            if bytes.len() > DRAFT_LIMIT {
                return Err(invalid("review draft exceeds 1 MiB"));
            }
            let draft: Self = serde_json::from_slice(&bytes)?;
            if draft.schema != STATE_SCHEMA {
                return Err(invalid("unsupported review draft"));
            }
            Ok(draft.text)
        }

        fn save(path: &Path, text: String) -> io::Result<()> {
            let bytes = serde_json::to_vec(&Self { schema: STATE_SCHEMA.into(), text })?;
            if bytes.len() > DRAFT_LIMIT {
                return Err(invalid("review draft exceeds 1 MiB"));
            }
            let mut file = tempfile::NamedTempFile::new_in(path.parent().expect("draft directory"))?;
            file.write_all(&bytes)?;
            file.as_file().sync_all()?;
            file.persist(path)?;
            Ok(())
        }
    }

    #[ui]
    fn view(
        cx: &mut BuildCx<'_>,
        presentation: &Presentation,
        version: &str,
        initial: &str,
        draft_path: PathBuf,
        session: Arc<Session>,
        store: Store,
    ) -> El {
        let draft = cx.signal(initial.to_owned());
        let saving = cx.signal(false);
        let status = cx.signal(String::new());
        let save = move |w: &mut WriteCx<'_>| {
            let text = w.get(draft);
            let path = draft_path.clone();
            let owner = session.clone();
            w.set(saving, true);
            w.spawn("save signed-review draft", move |task| async move {
                let saved = task
                    .background(move || {
                        // An in-flight save keeps the lease even if the window has closed.
                        let _owner = owner;
                        Draft::save(&path, text)
                    })
                    .await;
                task.write(|w| {
                    w.set(
                        status,
                        match saved {
                            Ok(()) => "Draft saved.".into(),
                            Err(error) => format!("Could not save draft: {error}"),
                        },
                    );
                    w.set(saving, false);
                });
            });
        };
        let editor = col()
            .test_id("review-editor")
            .p(sp(4.0))
            .gap(sp(3.0))
            .rounded(RadiusToken::Lg)
            .bg(ColorToken::Surface)
            .child(text("Review note").text_weight(600).test_id("review-label"))
            .child(
                textarea()
                    .test_id("review-draft")
                    .labelled_by("review-label")
                    .placeholder("Write your notes for this release")
                    .rows(3, 4)
                    .w(rel(1.0))
                    .p(sp(3.0))
                    .rounded(RadiusToken::Md)
                    .border(1)
                    .border_color(ColorToken::Border)
                    .value(draft)
                    .on_input(move |w, value| w.set(draft, value.to_owned())),
            )
            .child(
                row()
                    .gap(sp(3.0))
                    .align(AlignItems::Center)
                    .child(
                        octara_kit::button("Save draft", KitSize::Md, ButtonVariant::Secondary)
                            .test_id("review-save")
                            .disabled(saving)
                            .on_press(save),
                    )
                    .child(
                        text(status)
                            .test_id("review-save-status")
                            .role(Role::Status)
                            .live(Live::Polite)
                            .text_size(FontSizeToken::Sm)
                            .text_color(ColorToken::TextMuted),
                    ),
            );
        let preview = col()
            .test_id("review-preview")
            .min_h(90)
            .p(sp(4.0))
            .gap(sp(3.0))
            .rounded(RadiusToken::Lg)
            .border(1)
            .border_color(ColorToken::Border)
            .child(text("Draft preview").text_weight(600))
            .child(text(draft).test_id("review-preview-text").text_color(ColorToken::TextMuted));
        let header = row()
            .gap(sp(3.0))
            .align(AlignItems::Center)
            .child(
                text(presentation.title.clone())
                    .role(Role::Heading)
                    .test_id("review-title")
                    .text_size(FontSizeToken::Xl2)
                    .text_color(ColorToken::Accent)
                    .text_weight(600),
            )
            .child(text(version.to_owned()).test_id("review-version").text_size(FontSizeToken::Sm).text_color(ColorToken::TextMuted));
        let mut content =
            col().size(rel(1.0)).p(sp(5.0)).gap(sp(4.0)).bg(ColorToken::Background).overflow_y(Overflow::Scroll).child(header);
        if let Some(origin) = option_env!("OCTARA_REVIEW_UPDATE_ORIGIN") {
            content = content.child(update_controls(cx, store, origin));
        }
        match presentation.sections {
            [Section::Editor, Section::Preview] => content.child(editor).child(preview),
            _ => content.child(preview).child(editor),
        }
    }

    #[ui]
    fn update_controls(cx: &mut BuildCx<'_>, store: Store, origin: &'static str) -> El {
        let checking = cx.signal(false);
        let status = cx.signal(String::new());
        let check = move |w: &mut WriteCx<'_>| {
            w.set(checking, true);
            w.set(status, "Checking for updates…".into());
            let store = store.clone();
            w.spawn("check signed-review updates", move |task| async move {
                let checked = task.background(move || Channel::new(origin)?.check(&store, Presentation::prepare)).await;
                let message = match checked {
                    Ok(Check::Unpublished) => "No update has been published on this channel.".into(),
                    Ok(Check::AlreadyAdmitted(version)) => format!("Version {} has already been processed.", version.sequence),
                    Ok(Check::Staged(version)) => format!("Version {} downloaded. Reopen the app to apply it.", version.sequence),
                    Ok(Check::Superseded) => "The channel changed during the check. Check for updates again.".into(),
                    Err(error) => format!("Could not check for updates: {error}"),
                };
                task.write(|w| {
                    w.set(status, message);
                    w.set(checking, false);
                });
            });
        };
        row()
            .gap(sp(3.0))
            .align(AlignItems::Center)
            .child(
                octara_kit::button("Check for updates", KitSize::Md, ButtonVariant::Secondary)
                    .test_id("review-check-update")
                    .disabled(checking)
                    .on_press(check),
            )
            .child(text(status).test_id("review-update-status").role(Role::Status).live(Live::Polite).text_size(FontSizeToken::Sm))
    }

    fn policy() -> io::Result<Policy> {
        let encoded =
            option_env!("OCTARA_REVIEW_PUBLIC_KEY").ok_or_else(|| invalid("build signed_review with OCTARA_REVIEW_PUBLIC_KEY"))?;
        Ok(Policy {
            target: ReleaseTarget {
                app: APP_ID.into(),
                channel: "direct".into(),
                platform: "macos".into(),
                runtime: "signed-review/1".into(),
                resource_schema: "review.layout/1".into(),
                state_schema: STATE_SCHEMA.into(),
            },
            public_key: public_key(encoded)?,
            embedded_sequence: 0,
        })
    }

    pub(super) fn run() -> io::Result<()> {
        let mut args = std::env::args_os().skip(1);
        let directory = match args.next() {
            Some(path) => PathBuf::from(path),
            None => default_store()?,
        };
        let package = args.next();
        if args.next().is_some() {
            return Err(invalid("usage: signed_review [STORE [SIGNED_PACKAGE]]"));
        }
        let store = Store::new(directory.join("updates"), policy()?);
        if let Some(package) = package {
            match store.stage(File::open(package)?, Presentation::prepare)? {
                Staged::New(version) => println!("staged {version}"),
                Staged::AlreadyAdmitted(version) => println!("already admitted {version}"),
            }
            return Ok(());
        }

        let draft_path = directory.join("draft.json");
        fs::create_dir_all(&directory)?;
        let Startup { selected, recovery, session } = store.start(Presentation::prepare)?;
        let draft = Draft::load(&draft_path)?;
        if let Some(reason) = recovery {
            eprintln!("signed_review: {reason}");
        }
        let (version, presentation) = selected.map_or_else(
            || ("Built-in version".into(), Presentation::embedded()),
            |(version, presentation)| (format!("Version {}", version.sequence), presentation),
        );
        let session = Arc::new(session);
        let confirmer = session.clone();
        let mut options = window_options("Octara · Release review", 680.0, 550.0);
        options.presentation.theme = presentation.theme();
        let app = AppDefinition::new()
            .window(WindowDefinition::new(options, move |b| view(b, &presentation, &version, &draft, draft_path, session, store)));
        octara_host_native::run_app_with_first_frame(app, move |receipt| match confirmer.confirm() {
            Ok(Some(version)) => eprintln!("confirmed {version} at frame {}", receipt.stamp.frame),
            Ok(None) => {}
            Err(error) => eprintln!("signed_review: confirming the frame failed: {error}"),
        })
        .map_err(io::Error::other)
    }

    fn default_store() -> io::Result<PathBuf> {
        let directories = NSFileManager::defaultManager()
            .URLsForDirectory_inDomains(NSSearchPathDirectory::ApplicationSupportDirectory, NSSearchPathDomainMask::UserDomainMask);
        let directory =
            directories.firstObject().and_then(|url| url.to_file_path()).ok_or_else(|| invalid("could not find Application Support"))?;
        Ok(directory.join(APP_ID))
    }

    fn invalid(message: &'static str) -> io::Error {
        io::Error::new(io::ErrorKind::InvalidData, message)
    }
}

#[cfg(target_os = "macos")]
fn main() -> std::io::Result<()> {
    macos::run()
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("signed_review runs on macOS.");
}
