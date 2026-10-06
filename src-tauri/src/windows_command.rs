//! Bounded, hidden Windows discovery commands. Not exposed as an IPC endpoint.
use std::{ffi::OsString, fmt, io::{self, Read}, os::windows::{ffi::OsStringExt, io::{AsRawHandle, FromRawHandle, OwnedHandle}, process::CommandExt}, path::PathBuf, process::{Child, Command, Stdio}, thread, time::{Duration, Instant}};
use windows_sys::Win32::System::{JobObjects::{AssignProcessToJobObject, CreateJobObjectW, SetInformationJobObject, JobObjectExtendedLimitInformation, JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE}, SystemInformation::GetSystemDirectoryW};

const OUTPUT_LIMIT: u64 = 64 * 1024;
pub(crate) const CREATE_NO_WINDOW: u32 = 0x08000000;

#[derive(Debug)]
pub enum CommandError { SystemDirectory, Start, Job, Timeout, OutputLimit, Read, Exit(Option<i32>) }
impl fmt::Display for CommandError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SystemDirectory => formatter.write_str("Windows system directory is unavailable"),
            Self::Start => formatter.write_str("Windows discovery command could not start"),
            Self::Job => formatter.write_str("Windows discovery command could not be isolated"),
            Self::Timeout => formatter.write_str("Windows discovery command timed out"),
            Self::OutputLimit => formatter.write_str("Windows discovery output exceeded its limit"),
            Self::Read => formatter.write_str("Windows discovery output could not be read"),
            Self::Exit(code) => write!(formatter, "Windows discovery command failed ({code:?})"),
        }
    }
}

fn system_directory() -> Result<PathBuf, CommandError> {
    let mut buffer = vec![0_u16; 512];
    loop {
        // SAFETY: buffer is valid for its advertised UTF-16 capacity.
        let length = unsafe { GetSystemDirectoryW(buffer.as_mut_ptr(), buffer.len() as u32) };
        if length == 0 { return Err(CommandError::SystemDirectory); }
        if length as usize >= buffer.len() {
            if length > 32768 { return Err(CommandError::SystemDirectory); }
            buffer.resize(length as usize + 1, 0);
            continue;
        }
        return Ok(PathBuf::from(OsString::from_wide(&buffer[..length as usize])));
    }
}

pub(crate) struct Job(OwnedHandle);
impl Job {
    pub(crate) fn new() -> Result<Self, CommandError> {
        // SAFETY: default security and an unnamed, non-inheritable job object.
        let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if handle.is_null() { return Err(CommandError::Job); }
        // SAFETY: CreateJobObjectW transferred ownership of this valid handle.
        let job = Self(unsafe { OwnedHandle::from_raw_handle(handle) });
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // SAFETY: valid owned handle and correctly sized Windows limit structure.
        if unsafe { SetInformationJobObject(job.0.as_raw_handle(), JobObjectExtendedLimitInformation, (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(), std::mem::size_of_val(&limits) as u32) } == 0 { return Err(CommandError::Job); }
        Ok(job)
    }
    pub(crate) fn assign(&self, child: &Child) -> Result<(), CommandError> {
        // SAFETY: both handles remain alive until assignment and command teardown.
        if unsafe { AssignProcessToJobObject(self.0.as_raw_handle(), child.as_raw_handle()) } == 0 { Err(CommandError::Job) } else { Ok(()) }
    }
}

fn bounded_read(stream: impl Read) -> io::Result<Vec<u8>> {
    let mut output = Vec::new();
    stream.take(OUTPUT_LIMIT + 1).read_to_end(&mut output)?;
    Ok(output)
}

pub fn powershell(script: &str) -> Result<String, CommandError> { powershell_with_timeout(script, Duration::from_secs(10)) }

fn powershell_with_timeout(script: &str, timeout: Duration) -> Result<String, CommandError> {
    let system = system_directory()?;
    let home = system.join("WindowsPowerShell").join("v1.0");
    let program = home.join("powershell.exe");
    let job = Job::new()?;
    // Resolve from the Windows API, never PATH/current-directory executable lookup.
    let script = format!("[Console]::OutputEncoding = [Text.UTF8Encoding]::new($false); $OutputEncoding = [Console]::OutputEncoding; {script}");
    let mut child = Command::new(program).args(["-NoProfile", "-NoLogo", "-NonInteractive", "-Command", &script])
        .current_dir(system).env("PSModulePath", home.join("Modules"))
        .creation_flags(CREATE_NO_WINDOW).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|_| CommandError::Start)?;
    if let Err(error) = job.assign(&child) { let _ = child.kill(); let _ = child.wait(); return Err(error); }
    let stdout = child.stdout.take().ok_or(CommandError::Read)?;
    let stderr = child.stderr.take().ok_or(CommandError::Read)?;
    let stdout = thread::spawn(move || bounded_read(stdout));
    let stderr = thread::spawn(move || bounded_read(stderr));
    let started = Instant::now();
    let exit = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Err(_) => break Err(CommandError::Read),
            Ok(None) if started.elapsed() >= timeout => break Err(CommandError::Timeout),
            Ok(None) => thread::sleep(Duration::from_millis(20)),
        }
    };
    // Close the job BEFORE joining pipe readers. This kills a timed-out process
    // and descendants that might otherwise keep inherited pipe handles open.
    drop(job);
    let _ = child.wait();
    let stdout = stdout.join().map_err(|_| CommandError::Read)?.map_err(|_| CommandError::Read)?;
    let stderr = stderr.join().map_err(|_| CommandError::Read)?.map_err(|_| CommandError::Read)?;
    let status = exit?;
    if stdout.len() as u64 > OUTPUT_LIMIT || stderr.len() as u64 > OUTPUT_LIMIT { return Err(CommandError::OutputLimit); }
    if !status.success() { return Err(CommandError::Exit(status.code())); }
    String::from_utf8(stdout).map_err(|_| CommandError::Read)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn real_discovery_command_preserves_unicode_without_console_windows() {
        assert_eq!(powershell("[Console]::Write('Core Pulse — Español')").unwrap(), "Core Pulse — Español");
    }
    #[test]
    fn hung_command_is_terminated_and_pipe_readers_finish() {
        let start = Instant::now();
        let result = powershell_with_timeout("[Threading.Thread]::Sleep(10000)", Duration::from_millis(150));
        assert!(matches!(result, Err(CommandError::Timeout)));
        assert!(start.elapsed() < Duration::from_secs(3));
    }
    #[test]
    fn failed_command_is_not_returned_as_successful_discovery() {
        assert!(matches!(powershell("exit 7"), Err(CommandError::Exit(Some(7)))));
    }
    #[test]
    fn excessive_output_is_rejected_without_unbounded_allocation() {
        assert!(matches!(powershell("[Console]::Write(('x' * 131072))"), Err(CommandError::OutputLimit)));
    }
}
