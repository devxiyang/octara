//! Manage the app and Cargo without letting terminal signals bypass teardown.

use std::{
    fs::{File, TryLockError},
    io::{self, Read, Seek},
    os::unix::process::CommandExt,
    process::{Child, Command, ExitStatus, Output, Stdio},
    sync::atomic::{AtomicI32, Ordering},
    thread,
    time::Duration,
};

static SIGNAL: AtomicI32 = AtomicI32::new(0);

extern "C" fn record_signal(signal: libc::c_int) {
    SIGNAL.store(signal, Ordering::Relaxed);
}

pub(super) fn install_signal_handlers() -> io::Result<()> {
    // SAFETY: sigaction copies the initialized structure and calls a function that only stores
    // to a lock-free atomic. The handler has static lifetime.
    unsafe {
        let mut action: libc::sigaction = std::mem::zeroed();
        action.sa_sigaction = record_signal as *const () as libc::sighandler_t;
        libc::sigemptyset(&mut action.sa_mask);
        for signal in [libc::SIGINT, libc::SIGTERM] {
            if libc::sigaction(signal, &action, std::ptr::null_mut()) != 0 {
                return Err(io::Error::last_os_error());
            }
        }
    }
    Ok(())
}

pub(super) fn received_signal() -> Option<i32> {
    let signal = SIGNAL.load(Ordering::Relaxed);
    (signal != 0).then_some(signal)
}

fn check_interrupted() -> io::Result<()> {
    if received_signal().is_some() {
        return Err(io::Error::new(io::ErrorKind::Interrupted, "dev build interrupted"));
    }
    Ok(())
}

pub(super) fn lock(file: &File) -> io::Result<()> {
    loop {
        check_interrupted()?;
        match file.try_lock() {
            Ok(()) => return Ok(()),
            Err(TryLockError::WouldBlock) => thread::sleep(Duration::from_millis(30)),
            Err(TryLockError::Error(error)) => return Err(error),
        }
    }
}

pub(super) struct Process {
    pub(super) child: Child,
}

impl Process {
    pub(super) fn spawn(command: &mut Command) -> io::Result<Self> {
        command.process_group(0).spawn().map(|child| Self { child })
    }

    pub(super) fn try_wait(&mut self) -> Result<Option<ExitStatus>, String> {
        self.child.try_wait().map_err(|error| format!("checking app process status: {error}"))
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        if !matches!(self.child.try_wait(), Ok(Some(_))) {
            // SAFETY: the live child leads the private process group created by `spawn`.
            unsafe { libc::kill(-(self.child.id() as libc::pid_t), libc::SIGKILL) };
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

pub(super) fn output(command: &mut Command) -> io::Result<Output> {
    check_interrupted()?;
    let mut stdout = tempfile::tempfile()?;
    let mut stderr = tempfile::tempfile()?;
    command.stdin(Stdio::null()).stdout(stdout.try_clone()?).stderr(stderr.try_clone()?);
    let mut process = Process::spawn(command)?;
    let status = loop {
        if let Some(status) = process.child.try_wait()? {
            break status;
        }
        check_interrupted()?;
        thread::sleep(Duration::from_millis(30));
    };
    let read = |file: &mut std::fs::File| -> io::Result<Vec<u8>> {
        file.rewind()?;
        let mut contents = Vec::new();
        file.read_to_end(&mut contents)?;
        Ok(contents)
    };
    Ok(Output { status, stdout: read(&mut stdout)?, stderr: read(&mut stderr)? })
}
