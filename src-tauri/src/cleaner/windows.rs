//! Handle-based operations for the Windows-only cleaner. Never delete by path.
use std::{ffi::{c_void, OsString}, fs::{File, OpenOptions}, io, os::windows::{ffi::OsStringExt, fs::OpenOptionsExt, io::AsRawHandle}, path::{Component, Path, PathBuf}};

const READ_ATTRIBUTES: u32 = 0x80;
const LIST_DIRECTORY: u32 = 1;
const DELETE: u32 = 0x10000;
const SHARE_READ: u32 = 1;
const SHARE_WRITE: u32 = 2;
const SHARE_DELETE: u32 = 4;
const BACKUP_SEMANTICS: u32 = 0x02000000;
const OPEN_REPARSE_POINT: u32 = 0x00200000;
const REPARSE_POINT: u32 = 0x400;

#[repr(C)]
#[derive(Default)]
struct HandleInformation {
    attributes: u32,
    creation_time: [u32; 2],
    access_time: [u32; 2],
    write_time: [u32; 2],
    volume_serial: u32,
    size_high: u32,
    size_low: u32,
    links: u32,
    index_high: u32,
    index_low: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Identity {
    volume_serial: u64,
    file_id: [u8; 16],
}

#[repr(C)]
struct Guid { first: u32, second: u16, third: u16, rest: [u8; 8] }

#[link(name = "kernel32")]
extern "system" {
    fn GetFinalPathNameByHandleW(handle: *mut c_void, path: *mut u16, length: u32, flags: u32) -> u32;
    fn GetFileInformationByHandle(handle: *mut c_void, information: *mut HandleInformation) -> i32;
    fn GetFileInformationByHandleEx(handle: *mut c_void, class: i32, information: *mut c_void, size: u32) -> i32;
    fn SetFileInformationByHandle(handle: *mut c_void, class: i32, information: *const c_void, size: u32) -> i32;
}
#[link(name = "shell32")]
extern "system" {
    fn SHGetKnownFolderPath(folder: *const Guid, flags: u32, token: *mut c_void, path: *mut *mut u16) -> i32;
}
#[link(name = "ole32")]
extern "system" { fn CoTaskMemFree(memory: *mut c_void); }

fn unsafe_path() -> io::Error { io::Error::new(io::ErrorKind::InvalidInput, "Unsafe or changed cleanup path") }

/// Use the current user's Windows known folder, not an environment-controlled TEMP.
/// Custom/redirected TEMP locations are intentionally outside the initial allowlist.
pub fn user_temp() -> io::Result<PathBuf> {
    let local_app_data = Guid { first: 0xf1b32785, second: 0x6fba, third: 0x4fcf, rest: [0x9d, 0x55, 0x7b, 0x8e, 0x7f, 0x15, 0x70, 0x91] };
    let mut path = std::ptr::null_mut();
    // SAFETY: valid GUID and output pointer; null token means the current user.
    let result = unsafe { SHGetKnownFolderPath(&local_app_data, 0, std::ptr::null_mut(), &mut path) };
    if result < 0 || path.is_null() {
        if !path.is_null() { unsafe { CoTaskMemFree(path.cast()) }; }
        return Err(io::Error::other("Windows local app-data folder is unavailable"));
    }
    // SAFETY: a successful call returns a task-allocated, null-terminated UTF-16 string.
    let value = unsafe {
        let mut length = 0;
        while *path.add(length) != 0 { length += 1; }
        let value = OsString::from_wide(std::slice::from_raw_parts(path, length));
        CoTaskMemFree(path.cast());
        value
    };
    let root = std::fs::canonicalize(PathBuf::from(value))?.join("Temp");
    // Reject a redirected Temp directory itself, including junctions and symlinks.
    let guard = DirectoryGuard::open(&root)?;
    Ok(guard.path)
}

pub fn final_path(file: &File) -> io::Result<PathBuf> {
    let mut buffer = vec![0_u16; 512];
    loop {
        // SAFETY: the File owns a live handle, and buffer has the advertised capacity.
        let length = unsafe { GetFinalPathNameByHandleW(file.as_raw_handle(), buffer.as_mut_ptr(), buffer.len() as u32, 0) };
        if length == 0 { return Err(io::Error::last_os_error()); }
        if length as usize >= buffer.len() {
            if length > 32768 { return Err(unsafe_path()); }
            buffer.resize(length as usize + 1, 0);
            continue;
        }
        return Ok(PathBuf::from(OsString::from_wide(&buffer[..length as usize])));
    }
}

pub fn identity(file: &File) -> io::Result<Identity> {
    let mut information = Identity::default();
    // FileIdInfo (18) provides the volume and full 128-bit ID on Windows 10/11.
    // SAFETY: the structure matches FILE_ID_INFO, and its full size is passed.
    let result = unsafe { GetFileInformationByHandleEx(file.as_raw_handle(), 18, (&mut information as *mut Identity).cast(), std::mem::size_of::<Identity>() as u32) };
    if result == 0 { Err(io::Error::last_os_error()) } else { Ok(information) }
}

pub fn single_link(file: &File) -> io::Result<bool> {
    let mut information = HandleInformation::default();
    // SAFETY: valid live handle and BY_HANDLE_FILE_INFORMATION output structure.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut information) } == 0 { return Err(io::Error::last_os_error()); }
    Ok(information.links == 1 && information.attributes & REPARSE_POINT == 0)
}

pub fn open_candidate(path: &Path, for_deletion: bool) -> io::Result<File> {
    // OPEN_REPARSE_POINT opens the entry itself rather than following the final link.
    // No share mode during deletion: active/locked files and new writes are excluded.
    OpenOptions::new().access_mode(READ_ATTRIBUTES | if for_deletion { DELETE } else { 0 })
        .share_mode(if for_deletion { 0 } else { SHARE_READ | SHARE_WRITE | SHARE_DELETE })
        .custom_flags(OPEN_REPARSE_POINT).open(path)
}

pub struct DirectoryGuard { file: File, pub path: PathBuf }
impl DirectoryGuard {
    pub fn open(path: &Path) -> io::Result<Self> {
        // Keep every parent pinned until deletion finishes. Omitting SHARE_DELETE
        // prevents a directory from being renamed or replaced underneath the handle.
        // Request LIST_DIRECTORY as well as metadata: Windows may ignore sharing
        // restrictions for an attribute-only open, which does not pin renames.
        let file = OpenOptions::new().access_mode(READ_ATTRIBUTES | LIST_DIRECTORY).share_mode(SHARE_READ)
            .custom_flags(BACKUP_SEMANTICS | OPEN_REPARSE_POINT).open(path)?;
        use std::os::windows::fs::MetadataExt;
        let metadata = file.metadata()?;
        let actual = final_path(&file)?;
        if !metadata.is_dir() || metadata.file_attributes() & REPARSE_POINT != 0 || actual != path { return Err(unsafe_path()); }
        Ok(Self { file, path: actual })
    }
    pub fn identity(&self) -> io::Result<Identity> { identity(&self.file) }
}

pub fn pin_parents(root: &Path, candidate: &Path) -> io::Result<Vec<DirectoryGuard>> {
    let relative = candidate.strip_prefix(root).map_err(|_| unsafe_path())?;
    let mut parent = root.to_path_buf();
    let mut guards = Vec::new();
    let components = relative.components().collect::<Vec<_>>();
    if components.is_empty() { return Err(unsafe_path()); }
    for component in &components[..components.len() - 1] {
        if !matches!(component, Component::Normal(_)) { return Err(unsafe_path()); }
        parent.push(component.as_os_str());
        guards.push(DirectoryGuard::open(&parent)?);
    }
    if !matches!(components.last(), Some(Component::Normal(_))) { return Err(unsafe_path()); }
    Ok(guards)
}

pub fn pin_root(root: &Path) -> io::Result<Vec<DirectoryGuard>> {
    // Pin ancestors too: moving a parent of Temp would otherwise move the whole
    // tree outside the approved path despite Temp's own rename being blocked.
    root.ancestors().collect::<Vec<_>>().into_iter().rev().map(DirectoryGuard::open).collect()
}

pub fn mark_for_deletion(file: &File) -> io::Result<()> {
    // FILE_DISPOSITION_INFO is a single BOOLEAN (one byte), not a Win32 BOOL.
    let delete_file = 1_u8;
    // SAFETY: valid exclusive DELETE handle and correctly sized disposition buffer.
    if unsafe { SetFileInformationByHandle(file.as_raw_handle(), 4, (&delete_file as *const u8).cast(), 1) } == 0 {
        Err(io::Error::last_os_error())
    } else { Ok(()) }
}
