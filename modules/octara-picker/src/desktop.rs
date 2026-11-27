use std::num::NonZeroIsize;
use std::path::{Path, PathBuf};

use octara::module::RawNativeSurface;
use raw_window_handle::{
    DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle, Win32WindowHandle, WindowHandle,
    WindowsDisplayHandle, XlibDisplayHandle, XlibWindowHandle,
};
use rfd::FileDialog;

use super::{
    Dismiss, HostSurface, Job, ModuleCall, ModuleError, PickFiles, PickFolder, PickMedia, PickSavePath, PickedFile, PickedFolder, Request,
    Result, native,
};

pub(super) fn start(request: Request, surface: Option<HostSurface<'_>>, job: Job) -> Result<Dismiss> {
    let mut dialog = FileDialog::new();
    if let Some(parent) = surface.and_then(|surface| surface.raw_native()).and_then(Parent::from_surface) {
        dialog = dialog.set_parent(&parent);
    }
    let worker = std::thread::Builder::new().name("octara-picker".into()).spawn(move || match request {
        Request::Files(options) => files::<PickFiles>(dialog, options, job),
        Request::Media(options) => {
            let options = PickFiles {
                accept: options.kind.accept().into(),
                multiple: options.multiple,
                max_bytes: options.max_bytes,
                ..Default::default()
            };
            files::<PickMedia>(dialog, options, job);
        }
        Request::Folder(options) => {
            let result = job.check_live().and_then(|()| {
                configure(dialog, options.title, options.directory.as_deref())
                    .pick_folder()
                    .map(|path| PickedFolder::Path(path.into()))
                    .ok_or(ModuleError::Cancelled)
            });
            job.finish::<PickFolder>(result);
        }
        Request::Save(options) => {
            let result = job.check_live().and_then(|()| {
                let mut dialog = configure(dialog, options.title, options.directory.as_deref());
                if let Some(name) = options.name {
                    dialog = dialog.set_file_name(name);
                }
                dialog.save_file().map(Into::into).ok_or(ModuleError::Cancelled)
            });
            job.finish::<PickSavePath>(result);
        }
    });
    worker.map(|_| None).map_err(|_| ModuleError::Failed("could not start the desktop picker".into()))
}

fn files<C: ModuleCall<Output = Vec<PickedFile>>>(dialog: FileDialog, options: PickFiles, job: Job) {
    let result = job.check_live().and_then(|()| {
        let mut dialog = configure(dialog, options.title, options.directory.as_deref());
        let extensions = native::accept_extensions(&options.accept);
        if !extensions.is_empty() {
            dialog = dialog.add_filter("Supported files", &extensions);
        }
        let paths: Vec<PathBuf> =
            if options.multiple { dialog.pick_files().unwrap_or_default() } else { dialog.pick_file().into_iter().collect() };
        native::selected(paths, options.max_bytes, &job)
    });
    job.finish::<C>(result);
}

fn configure(mut dialog: FileDialog, title: Option<String>, directory: Option<&Path>) -> FileDialog {
    if let Some(title) = title {
        dialog = dialog.set_title(title);
    }
    if let Some(directory) = directory {
        dialog = dialog.set_directory(directory);
    }
    dialog
}

struct Parent(RawWindowHandle, RawDisplayHandle);

impl Parent {
    fn from_surface(surface: RawNativeSurface) -> Option<Self> {
        match surface {
            RawNativeSurface::Win32 { hwnd, .. } => {
                let handle = Win32WindowHandle::new(NonZeroIsize::new(hwnd.as_ptr() as isize)?);
                Some(Self(handle.into(), WindowsDisplayHandle::new().into()))
            }
            RawNativeSurface::Xlib { window, .. } => {
                #[allow(clippy::useless_conversion, reason = "Xlib uses c_ulong on both 32- and 64-bit targets")]
                let id = std::ffi::c_ulong::try_from(window).ok()?;
                Some(Self(XlibWindowHandle::new(id).into(), XlibDisplayHandle::new(None, 0).into()))
            }
            // rfd retains the handles after this call. Wayland pointers require a window
            // lifetime that the asynchronous dialog cannot guarantee.
            _ => None,
        }
    }
}

impl HasWindowHandle for Parent {
    fn window_handle(&self) -> std::result::Result<WindowHandle<'_>, HandleError> {
        // SAFETY: This adapter is borrowed while the host's HWND or XID is valid.
        Ok(unsafe { WindowHandle::borrow_raw(self.0) })
    }
}

impl HasDisplayHandle for Parent {
    fn display_handle(&self) -> std::result::Result<DisplayHandle<'_>, HandleError> {
        // SAFETY: No display pointer is needed for an HWND or XID.
        Ok(unsafe { DisplayHandle::borrow_raw(self.1) })
    }
}
