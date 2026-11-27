use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{NSModalResponseCancel, NSModalResponseOK, NSOpenPanel, NSSavePanel, NSView};
use objc2_foundation::{NSArray, NSString, NSURL};
use objc2_uniform_type_identifiers::UTType;
use octara::module::RawNativeSurface;

use super::apple::{accept_types, later, media_types};
use super::{
    Dismiss, HostSurface, Job, ModuleCall, ModuleError, PickFiles, PickFolder, PickMedia, PickSavePath, PickedFile, PickedFolder, Request,
    Result,
};

pub(super) fn start(request: Request, surface: Option<HostSurface<'_>>, job: Job) -> Result<Dismiss> {
    let mtm = MainThreadMarker::new().ok_or(ModuleError::Unavailable)?;
    let view = match surface.and_then(|surface| surface.raw_native()) {
        Some(RawNativeSurface::AppKit { ns_view, .. }) => {
            // SAFETY: The native host lends a live AppKit view on the main thread.
            Some(unsafe { Retained::<NSView>::retain(ns_view.as_ptr().cast()) }.ok_or(ModuleError::Unavailable)?)
        }
        Some(_) => return Err(ModuleError::Unavailable),
        None => None,
    };
    match request {
        Request::Files(request) => {
            let types = accept_types(&request.accept);
            open_files::<PickFiles>(mtm, view, job, request, types.as_deref())
        }
        Request::Media(request) => {
            let types = NSArray::from_retained_slice(&media_types(request.kind));
            let options = PickFiles { multiple: request.multiple, max_bytes: request.max_bytes, ..Default::default() };
            open_files::<PickMedia>(mtm, view, job, options, Some(&types))
        }
        Request::Folder(request) => {
            let panel = NSOpenPanel::openPanel(mtm);
            panel.setCanChooseFiles(false);
            panel.setCanChooseDirectories(true);
            panel.setAllowsMultipleSelection(false);
            configure(&panel, request.title.as_deref(), request.directory.as_deref());
            present::<PickFolder>(view, panel.into_super(), job, |panel, job| {
                job.finish::<PickFolder>(selected_path(panel).map(|path| PickedFolder::Path(path.into())));
            })
        }
        Request::Save(request) => {
            let panel = NSSavePanel::savePanel(mtm);
            configure(&panel, request.title.as_deref(), request.directory.as_deref());
            if let Some(name) = &request.name {
                panel.setNameFieldStringValue(&NSString::from_str(name));
            }
            present::<PickSavePath>(view, panel, job, |panel, job| {
                job.finish::<PickSavePath>(selected_path(panel).map(Into::into));
            })
        }
    }
}

fn open_files<C: ModuleCall<Output = Vec<PickedFile>>>(
    mtm: MainThreadMarker,
    view: Option<Retained<NSView>>,
    job: Job,
    request: PickFiles,
    types: Option<&NSArray<UTType>>,
) -> Result<Dismiss> {
    let panel = NSOpenPanel::openPanel(mtm);
    panel.setCanChooseFiles(true);
    panel.setCanChooseDirectories(false);
    panel.setAllowsMultipleSelection(request.multiple);
    configure(&panel, request.title.as_deref(), request.directory.as_deref());
    if let Some(types) = types {
        panel.setAllowedContentTypes(types);
    }
    present::<C>(view, panel.into_super(), job, move |panel, job| {
        let paths = panel.downcast_ref::<NSOpenPanel>().map(|panel| panel.URLs().iter().filter_map(|url| url.to_file_path()).collect());
        if let Some(paths) = paths {
            super::native::read_selected::<C>(paths, request.max_bytes, job);
        } else {
            job.finish::<C>(Err(ModuleError::Failed("file panel returned an invalid selection".into())));
        }
    })
}

fn configure(panel: &NSSavePanel, title: Option<&str>, directory: Option<&Path>) {
    if let Some(title) = title {
        let title = NSString::from_str(title);
        panel.setTitle(Some(&title));
        panel.setPrompt(Some(&title));
    }
    if let Some(url) = directory.and_then(NSURL::from_directory_path) {
        panel.setDirectoryURL(Some(&url));
    }
}

fn selected_path(panel: &NSSavePanel) -> Result<PathBuf> {
    panel.URL().and_then(|url| url.to_file_path()).ok_or_else(|| ModuleError::Failed("picker returned no filesystem path".into()))
}

fn present<C: ModuleCall<Output: Send>>(
    view: Option<Retained<NSView>>,
    panel: Retained<NSSavePanel>,
    job: Job,
    collect: impl Fn(&NSSavePanel, Job) + 'static,
) -> Result<Dismiss> {
    let panel = Rc::new(RefCell::new(Some(panel)));
    let opening = Rc::downgrade(&panel);
    let closing = Rc::downgrade(&panel);
    let completion = job.clone();
    let done = block2::RcBlock::new(move |response| {
        let Some(panel) = closing.upgrade().and_then(|slot| slot.borrow_mut().take()) else { return };
        panel.orderOut(None);
        if response == NSModalResponseOK {
            collect(&panel, completion.clone());
        } else if response == NSModalResponseCancel {
            completion.finish::<C>(Err(ModuleError::Cancelled));
        } else {
            completion.finish::<C>(Err(ModuleError::Failed("panel returned an unexpected response".into())));
        }
    });
    later(move || {
        let Some(panel) = opening.upgrade().and_then(|slot| slot.borrow().clone()) else { return };
        if let Some(view) = &view {
            let Some(window) = view.window() else {
                job.finish::<C>(Err(ModuleError::Unavailable));
                return;
            };
            if window.attachedSheet().is_some() {
                job.finish::<C>(Err(ModuleError::Busy));
                return;
            }
            panel.beginSheetModalForWindow_completionHandler(&window, &done);
        } else {
            panel.beginWithCompletionHandler(&done);
        }
    })?;
    Ok(Some(Box::new(move || {
        let retained = panel.borrow_mut().take();
        if let Some(panel) = retained.filter(|panel| panel.isVisible() || panel.sheetParent().is_some()) {
            // SAFETY: AppKit's cancel action accepts a nil sender.
            unsafe { panel.cancel(None) };
        }
    })))
}
