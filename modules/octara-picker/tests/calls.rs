use std::cell::RefCell;
use std::rc::Rc;

use octara::module::{ModuleError, ModuleObservationKind};
use octara::{App, AppDefinition};
use octara_picker::{ID, PickFiles, PickFolder, PickMedia, PickSavePath, PickedFile, mock, module};

fn headless_app() -> App {
    App::new(|_, _| unreachable!("picker calls need no headless window"))
}

#[test]
fn mock_cancels_each_call_and_validates_save_filenames() {
    let mut app = headless_app();
    AppDefinition::new().with_module(mock()).launch(&mut app, None).unwrap();
    app.update(|w| {
        w.call_module(PickFiles::default(), |_, result| assert_eq!(result.unwrap_err(), ModuleError::Cancelled));
        w.call_module(PickFolder::default(), |_, result| assert_eq!(result.unwrap_err(), ModuleError::Cancelled));
        w.call_module(PickMedia::default(), |_, result| assert_eq!(result.unwrap_err(), ModuleError::Cancelled));
        w.call_module(PickSavePath::default(), |_, result| assert_eq!(result.unwrap_err(), ModuleError::Cancelled));
        for name in ["", ".", "..", "../outside", "/absolute", r"nested\file", "null\0byte"] {
            w.call_module(PickSavePath { name: Some(name.into()), ..Default::default() }, |_, result| {
                assert!(matches!(result, Err(ModuleError::InvalidArgs(_))));
            });
        }
    })
    .unwrap();
}

#[test]
fn platform_picker_reports_unavailable_on_a_headless_host() {
    let mut app = headless_app();
    AppDefinition::new().with_module(module()).launch(&mut app, None).unwrap();
    app.update(|w| {
        w.call_module(PickMedia::default(), |_, result| assert_eq!(result.unwrap_err(), ModuleError::Unavailable));
    })
    .unwrap();
}

#[test]
fn selected_bytes_are_delivered_to_the_caller_and_redacted_from_observations() {
    let mut app = headless_app();
    AppDefinition::new().with_module(mock()).launch(&mut app, None).unwrap();
    let observations = Rc::new(RefCell::new(Vec::new()));
    let captured = observations.clone();
    let runtime = app.runtime_mut();
    runtime.set_module_observer(Some(Box::new(move |entry| captured.borrow_mut().push(entry))));
    runtime.stub_module_call::<PickFiles>(|request| {
        assert_eq!(request.title.as_deref(), Some("private project"));
        Ok(vec![PickedFile {
            path: None,
            name: "private.txt".into(),
            media_type: "text/plain".into(),
            bytes: Some(b"private contents".to_vec()),
        }])
    });
    let called = Rc::new(RefCell::new(Vec::new()));
    let files = called.clone();
    app.update(|w| {
        w.call_module(PickFiles { title: Some("private project".into()), ..Default::default() }, move |_, result| {
            files.replace(result.unwrap());
        });
    })
    .unwrap();
    let files = called.borrow();
    assert_eq!(files[0].bytes.as_deref(), Some(b"private contents".as_slice()));
    assert_eq!(files[0].path, None);

    let observations = observations.borrow();
    assert_eq!(
        observations.iter().map(|entry| entry.kind).collect::<Vec<_>>(),
        [ModuleObservationKind::Call, ModuleObservationKind::Reply]
    );
    for entry in &*observations {
        assert_eq!(entry.module, ID.0);
        assert_eq!(entry.name, "pick_files");
        assert_eq!(entry.payload.as_ref().and_then(|value| value.as_str()), Some("<redacted>"));
    }
}
