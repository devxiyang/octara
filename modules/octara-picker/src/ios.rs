use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use objc2::rc::{Allocated, Retained};
use objc2::runtime::ProtocolObject;
use objc2::{
    DefinedClass, MainThreadMarker, MainThreadOnly, Message, define_class, extern_class, extern_methods, extern_protocol, msg_send,
};
use objc2_foundation::{NSArray, NSError, NSItemProvider, NSObject, NSObjectProtocol, NSProgress, NSString, NSURL};
use objc2_photos_ui::{PHPickerConfiguration, PHPickerFilter, PHPickerResult};
use objc2_ui_kit::{
    UIAdaptivePresentationControllerDelegate, UIDocumentPickerDelegate, UIDocumentPickerViewController, UIPresentationController,
    UIViewController,
};
use objc2_uniform_type_identifiers::{UTType, UTTypeFolder, UTTypeItem};
use octara::module::RawNativeSurface;

use super::apple::{accept_types, later, media_types};
use super::{
    Dismiss, HostSurface, Job, MediaKind, ModuleCall, ModuleError, PickFiles, PickFolder, PickMedia, PickSavePath, PickedFile,
    PickedFolder, PickedPath, Request, Result, is_filename, native,
};

// objc2 0.3.2 has the AppKit picker controller binding. The iOS SDK declares
// this class as a UIViewController with an otherwise matching delegate.
extern_class!(
    #[unsafe(super(UIViewController, objc2_ui_kit::UIResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    struct PHPickerViewController;
);

impl PHPickerViewController {
    extern_methods!(
        #[unsafe(method(initWithConfiguration:))]
        #[unsafe(method_family = init)]
        fn init_with_configuration(this: Allocated<Self>, configuration: &PHPickerConfiguration) -> Retained<Self>;

        #[unsafe(method(setDelegate:))]
        #[unsafe(method_family = none)]
        fn set_delegate(&self, delegate: Option<&ProtocolObject<dyn PHPickerViewControllerDelegate>>);
    );
}

extern_protocol!(
    /// Callbacks from the system photo picker.
    ///
    /// # Safety
    ///
    /// Implementors must honor the PhotosUI delegate contract and handle events on the main thread.
    #[allow(clippy::missing_safety_doc, reason = "Clippy does not recognize the safety section expanded by extern_protocol")]
    unsafe trait PHPickerViewControllerDelegate: NSObjectProtocol + MainThreadOnly {
        #[unsafe(method(picker:didFinishPicking:))]
        #[unsafe(method_family = none)]
        fn picker_did_finish_picking(&self, picker: &PHPickerViewController, results: &NSArray<PHPickerResult>);
    }
);

enum Selection<'a> {
    Documents(&'a NSArray<NSURL>),
    Media(&'a NSArray<PHPickerResult>),
    Cancelled,
}

type Completion = Box<dyn FnOnce(Selection<'_>)>;

#[derive(Default)]
struct DelegateState(RefCell<Option<Completion>>);

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "OctaraModulePickerDelegate"]
    #[ivars = DelegateState]
    struct Delegate;

    unsafe impl NSObjectProtocol for Delegate {}

    unsafe impl UIDocumentPickerDelegate for Delegate {
        #[unsafe(method(documentPicker:didPickDocumentsAtURLs:))]
        fn documents_selected(&self, _: &UIDocumentPickerViewController, urls: &NSArray<NSURL>) {
            self.deliver(Selection::Documents(urls));
        }

        #[unsafe(method(documentPickerWasCancelled:))]
        fn documents_cancelled(&self, _: &UIDocumentPickerViewController) {
            self.deliver(Selection::Cancelled);
        }
    }

    unsafe impl PHPickerViewControllerDelegate for Delegate {
        #[unsafe(method(picker:didFinishPicking:))]
        fn photos_selected(&self, _: &PHPickerViewController, results: &NSArray<PHPickerResult>) {
            self.deliver(Selection::Media(results));
        }
    }

    unsafe impl UIAdaptivePresentationControllerDelegate for Delegate {
        #[unsafe(method(presentationControllerDidDismiss:))]
        fn dismissed(&self, _: &UIPresentationController) {
            self.deliver(Selection::Cancelled);
        }
    }
);

impl Delegate {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let delegate = mtm.alloc::<Self>().set_ivars(DelegateState::default());
        // SAFETY: NSObject's initializer is called on the allocated delegate.
        unsafe { msg_send![super(delegate), init] }
    }

    fn deliver(&self, selection: Selection<'_>) {
        let retained = self.retain();
        if let Some(complete) = retained.ivars().0.take() {
            complete(selection);
        }
    }
}

pub(super) fn start(request: Request, surface: Option<HostSurface<'_>>, job: Job) -> Result<Dismiss> {
    let mtm = MainThreadMarker::new().ok_or(ModuleError::Unavailable)?;
    let Some(RawNativeSurface::UiKit { ui_view_controller, .. }) = surface.and_then(|surface| surface.raw_native()) else {
        return Err(ModuleError::Unavailable);
    };
    // SAFETY: The native host lends this live UIViewController on the main thread.
    let presenter: Retained<UIViewController> =
        unsafe { Retained::retain(ui_view_controller.as_ptr().cast()) }.ok_or(ModuleError::Unavailable)?;
    let delegate = Delegate::new(mtm);
    match request {
        Request::Files(request) => {
            // SAFETY: UTTypeItem is the immutable system type used for unfiltered documents.
            let types = accept_types(&request.accept).unwrap_or_else(|| NSArray::from_slice(&[unsafe { UTTypeItem }]));
            let picker =
                document_picker(mtm, &delegate, &types, true, request.multiple, request.title.as_deref(), request.directory.as_deref());
            present::<PickFiles>(presenter, picker.into_super(), delegate, job, move |selection, job| {
                let paths = match selection {
                    Selection::Documents(urls) => urls.iter().map(|url| url_path(&url)).collect(),
                    _ => Err(ModuleError::Cancelled),
                };
                match paths {
                    Ok(paths) => native::read_selected::<PickFiles>(paths, request.max_bytes, job),
                    Err(error) => job.finish::<PickFiles>(Err(error)),
                }
                Vec::new()
            })
        }
        Request::Folder(request) => {
            let picker = folder_picker(mtm, &delegate, request.title.as_deref(), request.directory.as_deref());
            present::<PickFolder>(presenter, picker.into_super(), delegate, job, |selection, job| {
                job.finish::<PickFolder>(selected_folder(selection).map(PickedFolder::Path));
                Vec::new()
            })
        }
        Request::Save(request) => {
            let name =
                request.name.ok_or_else(|| ModuleError::InvalidArgs("iOS requires a filename to select a save destination".into()))?;
            let picker = folder_picker(mtm, &delegate, request.title.as_deref(), request.directory.as_deref());
            present::<PickSavePath>(presenter, picker.into_super(), delegate, job, move |selection, job| {
                let result = selected_folder(selection).map(|mut picked| {
                    picked.path.push(name);
                    picked
                });
                job.finish::<PickSavePath>(result);
                Vec::new()
            })
        }
        Request::Media(request) => {
            let picker = media_picker(mtm, &delegate, request.kind, request.multiple);
            present::<PickMedia>(presenter, picker.into_super(), delegate, job, move |selection, job| load_media(selection, request, job))
        }
    }
}

fn document_picker(
    mtm: MainThreadMarker,
    delegate: &Delegate,
    types: &NSArray<UTType>,
    copy: bool,
    multiple: bool,
    title: Option<&str>,
    directory: Option<&Path>,
) -> Retained<UIDocumentPickerViewController> {
    let picker = UIDocumentPickerViewController::initForOpeningContentTypes_asCopy(mtm.alloc(), types, copy);
    picker.setDelegate(Some(ProtocolObject::from_ref(delegate)));
    picker.setAllowsMultipleSelection(multiple);
    if let Some(title) = title {
        picker.setTitle(Some(&NSString::from_str(title)));
    }
    if let Some(url) = directory.and_then(NSURL::from_directory_path) {
        picker.setDirectoryURL(Some(&url));
    }
    picker
}

fn folder_picker(
    mtm: MainThreadMarker,
    delegate: &Delegate,
    title: Option<&str>,
    directory: Option<&Path>,
) -> Retained<UIDocumentPickerViewController> {
    // SAFETY: UTTypeFolder is an immutable system type.
    let types = NSArray::from_slice(&[unsafe { UTTypeFolder }]);
    document_picker(mtm, delegate, &types, false, false, title, directory)
}

fn media_picker(mtm: MainThreadMarker, delegate: &Delegate, kind: MediaKind, multiple: bool) -> Retained<PHPickerViewController> {
    // SAFETY: Use PhotosUI's configuration and immutable filters on the main thread.
    let configuration = unsafe {
        let filter = match kind {
            MediaKind::Photos => PHPickerFilter::imagesFilter(),
            MediaKind::Videos => PHPickerFilter::videosFilter(),
            MediaKind::PhotosAndVideos => {
                let filters = [PHPickerFilter::imagesFilter(), PHPickerFilter::videosFilter()];
                PHPickerFilter::anyFilterMatchingSubfilters(&NSArray::from_retained_slice(&filters))
            }
        };
        let configuration = PHPickerConfiguration::new();
        configuration.setFilter(Some(&filter));
        configuration.setSelectionLimit(if multiple { 0 } else { 1 });
        configuration
    };
    let picker = PHPickerViewController::init_with_configuration(mtm.alloc(), &configuration);
    picker.set_delegate(Some(ProtocolObject::from_ref(delegate)));
    picker
}

fn present<C: ModuleCall<Output: Send>>(
    presenter: Retained<UIViewController>,
    controller: Retained<UIViewController>,
    delegate: Retained<Delegate>,
    job: Job,
    collect: impl FnOnce(Selection<'_>, Job) -> Vec<Retained<NSProgress>> + 'static,
) -> Result<Dismiss> {
    let presented = Rc::new(RefCell::new(Some((controller, delegate.clone()))));
    let progress = Rc::new(RefCell::new(Vec::<Retained<NSProgress>>::new()));
    let completed = Rc::downgrade(&presented);
    let transfers = progress.clone();
    let completion = job.clone();
    delegate.ivars().0.replace(Some(Box::new(move |selection| {
        if let Some((controller, _)) = completed.upgrade().and_then(|slot| slot.borrow_mut().take()) {
            dismiss(&controller);
            transfers.replace(collect(selection, completion));
        }
    })));
    let opening = Rc::downgrade(&presented);
    later(move || {
        let Some((controller, delegate)) = opening.upgrade().and_then(|slot| slot.borrow().clone()) else { return };
        let Some(window) = presenter.viewIfLoaded().and_then(|view| view.window()) else {
            job.finish::<C>(Err(ModuleError::Unavailable));
            return;
        };
        if presenter.presentedViewController().is_some()
            || window.rootViewController().and_then(|root| root.presentedViewController()).is_some()
        {
            job.finish::<C>(Err(ModuleError::Busy));
            return;
        }
        observe_dismissal(&controller, &delegate);
        presenter.presentViewController_animated_completion(&controller, true, None);
        observe_dismissal(&controller, &delegate);
    })?;
    Ok(Some(Box::new(move || {
        if let Some((controller, delegate)) = presented.borrow_mut().take() {
            delegate.ivars().0.take();
            dismiss(&controller);
        }
        for transfer in progress.take() {
            transfer.cancel();
        }
    })))
}

fn observe_dismissal(controller: &UIViewController, delegate: &Delegate) {
    if let Some(presentation) = controller.presentationController() {
        // SAFETY: UIKit weakly references the delegate retained by the active picker.
        unsafe { presentation.setDelegate(Some(ProtocolObject::from_ref(delegate))) };
    }
}

fn dismiss(controller: &UIViewController) {
    if controller.presentingViewController().is_some() && !controller.isBeingDismissed() {
        controller.dismissViewControllerAnimated_completion(false, None);
    }
}

fn selected_folder(selection: Selection<'_>) -> Result<PickedPath> {
    let Selection::Documents(urls) = selection else { return Err(ModuleError::Cancelled) };
    let url = urls.firstObject().ok_or(ModuleError::Cancelled)?;
    let path = url_path(&url)?;
    // SAFETY: The document picker vends security-scoped directory URLs. The
    // access grant is released exactly once when the last handle is dropped.
    if !unsafe { url.startAccessingSecurityScopedResource() } {
        return Err(ModuleError::PermissionDenied);
    }
    Ok(PickedPath { path, _scope: Some(Arc::new(SecurityScope(url))) })
}

fn url_path(url: &NSURL) -> Result<PathBuf> {
    url.to_file_path().ok_or_else(|| ModuleError::Failed("picker returned no filesystem path".into()))
}

#[derive(Debug)]
pub(super) struct SecurityScope(Retained<NSURL>);

impl Drop for SecurityScope {
    fn drop(&mut self) {
        // SAFETY: This scope owns one successful startAccessing call.
        unsafe { self.0.stopAccessingSecurityScopedResource() };
    }
}

fn load_media(selection: Selection<'_>, request: PickMedia, job: Job) -> Vec<Retained<NSProgress>> {
    let Selection::Media(results) = selection else {
        job.finish::<PickMedia>(Err(ModuleError::Cancelled));
        return Vec::new();
    };
    let count = results.len();
    if count == 0 {
        job.finish::<PickMedia>(Err(ModuleError::Cancelled));
        return Vec::new();
    }
    let types = media_types(request.kind);
    let batch = Arc::new(MediaBatch { job, results: Mutex::new(MediaResults { pending: count, files: vec![None; count] }) });
    results
        .iter()
        .enumerate()
        .filter_map(|(index, item)| {
            // SAFETY: PhotosUI supplied a valid selection and owns the item provider.
            let provider = unsafe { item.itemProvider() };
            let Some((identifier, content_type)) = representation(&provider, &types) else {
                batch.complete(index, Err(ModuleError::Failed("selected asset has no matching file representation".into())));
                return None;
            };
            let name = provider.suggestedName().map(|name| name.to_string()).filter(|name| is_filename(name));
            let media_type = content_type.preferredMIMEType().map(|mime| mime.to_string());
            let batch = batch.clone();
            let done = block2::RcBlock::new(move |url: *mut NSURL, _: *mut NSError| {
                // SAFETY: NSItemProvider keeps the temporary URL and file alive until
                // this callback returns. Only the owned bytes escape the callback.
                let result = unsafe { url.as_ref() }
                    .ok_or_else(|| ModuleError::Failed("could not load selected media".into()))
                    .and_then(|url| media_bytes(url, name.as_deref(), media_type.as_deref(), request.max_bytes, &batch.job));
                batch.complete(index, result);
            });
            // SAFETY: The block captures owned, thread-safe values, and can be invoked off the main thread.
            Some(unsafe { provider.loadFileRepresentationForTypeIdentifier_completionHandler(&identifier, &done) })
        })
        .collect()
}

fn representation(provider: &NSItemProvider, types: &[Retained<UTType>]) -> Option<(Retained<NSString>, Retained<UTType>)> {
    provider.registeredTypeIdentifiers().iter().find_map(|identifier| {
        let ty = UTType::typeWithIdentifier(&identifier)?;
        types.iter().any(|candidate| ty.conformsToType(candidate)).then_some((identifier, ty))
    })
}

fn media_bytes(url: &NSURL, name: Option<&str>, media_type: Option<&str>, max_bytes: Option<usize>, job: &Job) -> Result<PickedFile> {
    let path = url_path(url)?;
    let bytes = native::read_file(&path, max_bytes, job)?;
    let name = if let Some(name) = name {
        let mut name = PathBuf::from(name);
        if let Some(extension) = path.extension() {
            name.set_extension(extension);
        }
        name.to_string_lossy().into_owned()
    } else {
        path.file_name().ok_or_else(|| ModuleError::Failed("selected asset has no filename".into()))?.to_string_lossy().into_owned()
    };
    Ok(PickedFile { path: None, name, media_type: media_type.unwrap_or_else(|| native::mime_type(&path)).into(), bytes: Some(bytes) })
}

struct MediaBatch {
    job: Job,
    results: Mutex<MediaResults>,
}

struct MediaResults {
    pending: usize,
    files: Vec<Option<Result<PickedFile>>>,
}

impl MediaBatch {
    fn complete(&self, index: usize, result: Result<PickedFile>) {
        let mut results = self.results.lock().expect("media selection");
        let Some(slot) = results.files.get_mut(index).filter(|slot| slot.is_none()) else { return };
        *slot = Some(result);
        results.pending -= 1;
        if results.pending == 0 {
            let selected = std::mem::take(&mut results.files).into_iter().flatten().collect();
            drop(results);
            self.job.finish::<PickMedia>(selected);
        }
    }
}
