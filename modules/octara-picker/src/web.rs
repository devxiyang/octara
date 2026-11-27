use std::cell::RefCell;
use std::rc::Rc;

use js_sys::{AsyncIterator, Function, Promise, Reflect, Uint8Array};
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::{JsFuture, spawn_local};
use web_sys::{DomException, Event, EventTarget, File, FileSystemDirectoryHandle, FileSystemFileHandle, HtmlInputElement, Window};

use super::{
    Dismiss, FolderFile, Job, ModuleCall, ModuleError, PickFiles, PickFolder, PickMedia, PickedFile, PickedFolder, Request, Result,
    too_large,
};

pub(super) fn start(request: Request, job: Job) -> Result<Dismiss> {
    if matches!(request, Request::Save(_)) {
        return Err(ModuleError::Unavailable);
    }
    let window = web_sys::window().ok_or(ModuleError::Unavailable)?;
    if activation(&window) == Some(false) {
        return Err(ModuleError::ActivationRequired);
    }
    match request {
        Request::Files(options) => files::<PickFiles>(&window, job, &options.accept, options.multiple, options.max_bytes),
        Request::Media(options) => files::<PickMedia>(&window, job, options.kind.accept(), options.multiple, options.max_bytes),
        Request::Folder(options) => directory(&window, job, options.max_bytes),
        Request::Save(_) => Err(ModuleError::Unavailable),
    }
}

fn activation(window: &Window) -> Option<bool> {
    let activation = property(window.navigator().as_ref(), "userActivation").ok()?;
    property(&activation, "isActive").ok()?.as_bool()
}

fn files<C: ModuleCall<Output = Vec<PickedFile>>>(
    window: &Window,
    job: Job,
    accept: &str,
    multiple: bool,
    max_bytes: Option<usize>,
) -> Result<Dismiss> {
    pick_input::<C, _>(window, job, accept, multiple, false, move |files, job| async move {
        let mut selected = Vec::with_capacity(files.len());
        for file in files {
            selected.push(read_file(file, max_bytes, &job).await?);
        }
        Ok(selected)
    })
}

fn directory(window: &Window, job: Job, max_bytes: Option<usize>) -> Result<Dismiss> {
    // File System Access prohibits cross-origin use. The file input is available in frames.
    let top_level = window.top().ok().flatten().as_ref() == Some(window);
    if top_level
        && window.is_secure_context()
        && let Ok(method) = property(window.as_ref(), "showDirectoryPicker")?.dyn_into::<Function>()
    {
        let promise = method.call0(window.as_ref()).map_err(|error| picker_error(window, error))?;
        let promise = promise.dyn_into::<Promise>().map_err(|_| ModuleError::Unavailable)?;
        spawn_local(async move {
            let result = walk_directory(promise, max_bytes, &job).await;
            job.finish::<PickFolder>(result);
        });
        return Ok(None);
    }
    pick_input::<PickFolder, _>(
        window,
        job,
        "",
        true,
        true,
        move |files, job| async move { read_relative_files(files, max_bytes, &job).await },
    )
}

async fn walk_directory(promise: Promise, max_bytes: Option<usize>, job: &Job) -> Result<PickedFolder> {
    let directory =
        JsFuture::from(promise).await.map_err(browser_error)?.dyn_into::<FileSystemDirectoryHandle>().map_err(|_| invalid_selection())?;
    let name = directory.name();
    let mut stack: Vec<(String, AsyncIterator)> = vec![(String::new(), directory.values())];
    let mut contents = DirectoryContents::new(max_bytes);
    while let Some((prefix, entries)) = stack.last_mut() {
        job.check_live()?;
        let entry = JsFuture::from(entries.next_iterator().map_err(browser_error)?).await.map_err(browser_error)?;
        if entry.done() {
            stack.pop();
            continue;
        }
        let value = entry.value();
        if let Some(directory) = value.dyn_ref::<FileSystemDirectoryHandle>() {
            let path = relative_path(prefix, &directory.name());
            stack.push((path, directory.values()));
        } else if let Some(handle) = value.dyn_ref::<FileSystemFileHandle>() {
            let file =
                JsFuture::from(handle.get_file()).await.map_err(browser_error)?.dyn_into::<File>().map_err(|_| invalid_selection())?;
            contents.push(relative_path(prefix, &file.name()), file, job).await?;
        } else {
            return Err(invalid_selection());
        }
    }
    Ok(contents.finish(name))
}

fn relative_path(prefix: &str, name: &str) -> String {
    if prefix.is_empty() { name.into() } else { format!("{prefix}/{name}") }
}

async fn read_relative_files(files: Vec<File>, max_bytes: Option<usize>, job: &Job) -> Result<PickedFolder> {
    let mut name = None;
    let mut contents = DirectoryContents::new(max_bytes);
    for file in files {
        let path = property(file.as_ref(), "webkitRelativePath")?.as_string().ok_or_else(invalid_selection)?;
        let (root, relative) = path.split_once('/').ok_or_else(invalid_selection)?;
        if root.is_empty()
            || relative.split('/').any(|part| matches!(part, "" | "." | ".."))
            || name.get_or_insert_with(|| root.to_owned()) != root
        {
            return Err(invalid_selection());
        }
        contents.push(relative.to_owned(), file, job).await?;
    }
    Ok(contents.finish(name.ok_or(ModuleError::Cancelled)?))
}

struct DirectoryContents {
    files: Vec<FolderFile>,
    max_bytes: Option<usize>,
    used: usize,
}

impl DirectoryContents {
    fn new(max_bytes: Option<usize>) -> Self {
        Self { files: Vec::new(), max_bytes, used: 0 }
    }

    async fn push(&mut self, relative_path: String, file: File, job: &Job) -> Result<()> {
        let remaining = self.max_bytes.map(|limit| limit - self.used);
        check_size(&file, remaining).map_err(|_| too_large(self.max_bytes.unwrap_or(usize::MAX)))?;
        let file = read_file(file, remaining, job).await?;
        if let Some(bytes) = &file.bytes {
            self.used += bytes.len();
        }
        self.files.push(FolderFile { relative_path, file });
        Ok(())
    }

    fn finish(mut self, name: String) -> PickedFolder {
        self.files.sort_unstable_by(|left, right| left.relative_path.cmp(&right.relative_path));
        PickedFolder::Files { name, files: self.files }
    }
}

enum InputSelection {
    Files(Vec<File>),
    Cancelled,
}

fn pick_input<C: ModuleCall<Output: Send>, F: Future<Output = Result<C::Output>> + 'static>(
    window: &Window,
    job: Job,
    accept: &str,
    multiple: bool,
    directory: bool,
    read: impl Fn(Vec<File>, Job) -> F + 'static,
) -> Result<Dismiss> {
    let document = window.document().ok_or(ModuleError::Unavailable)?;
    let body = document.body().ok_or(ModuleError::Unavailable)?;
    let input =
        document.create_element("input").map_err(browser_error)?.dyn_into::<HtmlInputElement>().map_err(|_| ModuleError::Unavailable)?;
    if directory {
        if !Reflect::has(input.as_ref(), &JsValue::from_str("webkitdirectory")).map_err(browser_error)? {
            return Err(ModuleError::Unavailable);
        }
        input.set_attribute("webkitdirectory", "").map_err(browser_error)?;
    }
    input.set_type("file");
    input.set_multiple(multiple);
    input.set_accept(accept);
    input.set_hidden(true);
    input.set_attribute("data-octara-file-picker", "").map_err(browser_error)?;

    let slot = Rc::new(RefCell::new(None::<InputPicker>));
    let active = Rc::downgrade(&slot);
    let read = Rc::new(read);
    let handle = Rc::new(move |selection| {
        let active = active.clone();
        let read = read.clone();
        let job = job.clone();
        // Detach the element and listeners after this JavaScript event returns.
        spawn_local(async move {
            if active.upgrade().and_then(|slot| slot.borrow_mut().take()).is_none() {
                return;
            }
            let result = match selection {
                InputSelection::Files(files) if !files.is_empty() => read(files, job.clone()).await,
                _ => Err(ModuleError::Cancelled),
            };
            job.finish::<C>(result);
        });
    });
    let changed = handle.clone();
    let listeners = [
        Listener::new(input.as_ref(), "change", move |event| {
            let files = event
                .target()
                .and_then(|target| target.dyn_into::<HtmlInputElement>().ok())
                .and_then(|input| input.files())
                .map_or_else(Vec::new, |files| (0..files.length()).filter_map(|index| files.item(index)).collect());
            changed(InputSelection::Files(files));
        })?,
        Listener::new(input.as_ref(), "cancel", move |_| handle(InputSelection::Cancelled))?,
    ];
    slot.replace(Some(InputPicker { input: input.clone(), _listeners: listeners }));
    body.append_child(&input).map_err(browser_error)?;
    if property(input.as_ref(), "showPicker")?.is_function() {
        input.show_picker().map_err(|error| picker_error(window, error))?;
    } else {
        input.click();
    }
    Ok(Some(Box::new(move || {
        slot.borrow_mut().take();
    })))
}

struct InputPicker {
    input: HtmlInputElement,
    _listeners: [Listener; 2],
}

impl Drop for InputPicker {
    fn drop(&mut self) {
        self.input.remove();
    }
}

struct Listener {
    target: EventTarget,
    name: &'static str,
    callback: Closure<dyn Fn(Event)>,
}

impl Listener {
    fn new(target: &EventTarget, name: &'static str, callback: impl Fn(Event) + 'static) -> Result<Self> {
        let callback = Closure::new(callback);
        target.add_event_listener_with_callback(name, callback.as_ref().unchecked_ref()).map_err(browser_error)?;
        Ok(Self { target: target.clone(), name, callback })
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        let _ = self.target.remove_event_listener_with_callback(self.name, self.callback.as_ref().unchecked_ref());
    }
}

fn check_size(file: &File, max_bytes: Option<usize>) -> Result<()> {
    let limit = max_bytes.unwrap_or(isize::MAX as usize).min(isize::MAX as usize);
    if file.size() > limit as f64 {
        return Err(too_large(limit));
    }
    Ok(())
}

async fn read_file(file: File, max_bytes: Option<usize>, job: &Job) -> Result<PickedFile> {
    job.check_live()?;
    check_size(&file, max_bytes)?;
    let buffer = JsFuture::from(file.array_buffer()).await.map_err(browser_error)?;
    job.check_live()?;
    let source = Uint8Array::new(&buffer);
    let len = source.length() as usize;
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(len).map_err(|_| ModuleError::Failed("could not allocate space for the selected file".into()))?;
    bytes.resize(len, 0);
    source.copy_to(&mut bytes);
    Ok(PickedFile { path: None, name: file.name(), media_type: file.type_(), bytes: Some(bytes) })
}

fn property(value: &JsValue, key: &str) -> Result<JsValue> {
    Reflect::get(value, &JsValue::from_str(key)).map_err(browser_error)
}

fn picker_error(window: &Window, value: JsValue) -> ModuleError {
    if error_name(&value).as_deref() == Some("NotAllowedError") {
        return ModuleError::ActivationRequired;
    }
    if error_name(&value).as_deref() == Some("SecurityError") && activation(window) == Some(false) {
        return ModuleError::ActivationRequired;
    }
    browser_error(value)
}

fn error_name(value: &JsValue) -> Option<String> {
    value.dyn_ref::<DomException>().map(DomException::name)
}

fn browser_error(value: JsValue) -> ModuleError {
    match error_name(&value).as_deref() {
        Some("NotAllowedError" | "SecurityError") => ModuleError::PermissionDenied,
        Some("AbortError") => ModuleError::Cancelled,
        Some("NotSupportedError") => ModuleError::Unavailable,
        _ => ModuleError::Failed("browser could not complete the picker request".into()),
    }
}

fn invalid_selection() -> ModuleError {
    ModuleError::Failed("browser returned an invalid file selection".into())
}
