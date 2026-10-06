//! Read-only executable and process identity checks; never launch or modify games.
use std::{
    fs::{File, OpenOptions},
    io::{Read, Seek, SeekFrom},
    os::windows::{
        ffi::OsStrExt,
        fs::{MetadataExt, OpenOptionsExt},
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    path::{Component, Path, Prefix},
};
use windows_sys::Win32::{
    Foundation::{FILETIME, WAIT_OBJECT_0, WAIT_TIMEOUT},
    Storage::FileSystem::{
        FileIdInfo, GetDriveTypeW, GetFileInformationByHandleEx, GetFileType,
        GetFinalPathNameByHandleW, FILE_ID_INFO, FILE_TYPE_DISK,
    },
    System::Threading::{
        GetProcessTimes, OpenProcess, QueryFullProcessImageNameW, WaitForSingleObject,
        PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
    },
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Executable {
    pub path: String,
    pub identity: String,
}

pub fn inspect(path: &Path) -> Result<Executable, String> {
    if !path.is_absolute()
        || path.as_os_str().encode_wide().any(|unit| unit == 0)
        || !path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("exe"))
    {
        return Err("Pilih file executable Windows (.exe) yang valid".into());
    }
    let drive = match path.components().next() {
        Some(Component::Prefix(prefix)) => match prefix.kind() {
            Prefix::Disk(drive) | Prefix::VerbatimDisk(drive) => drive,
            _ => return Err("Daftarkan executable dari drive lokal".into()),
        },
        _ => return Err("Daftarkan executable dari drive lokal".into()),
    };
    let root = [drive as u16, b':' as u16, b'\\' as u16, 0];
    // DRIVE_REMOVABLE=2, DRIVE_FIXED=3; reject mapped network volumes before I/O.
    if ![2, 3].contains(&unsafe { GetDriveTypeW(root.as_ptr()) }) {
        return Err("Daftarkan executable dari drive lokal".into());
    }
    // Resolve from a held file handle, including junctions, without trusting the
    // frontend path. A full volume + 128-bit file ID distinguishes replacements.
    let mut file = OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(path)
        .map_err(|_| "Executable tidak dapat dibaca".to_string())?;
    if unsafe { GetFileType(file.as_raw_handle()) } != FILE_TYPE_DISK
        || !file
            .metadata()
            .map_err(|_| "Metadata executable tidak tersedia")?
            .is_file()
    {
        return Err("Pilih file executable Windows (.exe) yang valid".into());
    }
    validate_pe(&mut file)?;
    let mut id = FILE_ID_INFO::default();
    let mut name = vec![0u16; 32768];
    // SAFETY: file stays open and both output buffers have their declared sizes.
    if unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle(),
            FileIdInfo,
            (&mut id as *mut FILE_ID_INFO).cast(),
            std::mem::size_of::<FILE_ID_INFO>() as u32,
        )
    } == 0
    {
        return Err("Identitas file executable tidak dapat diverifikasi".into());
    }
    let length = unsafe {
        GetFinalPathNameByHandleW(
            file.as_raw_handle(),
            name.as_mut_ptr(),
            name.len() as u32,
            0,
        )
    } as usize;
    if length == 0 || length >= name.len() {
        return Err("Lokasi executable tidak dapat diverifikasi".into());
    }
    let path = String::from_utf16(&name[..length])
        .map_err(|_| "Lokasi executable tidak dapat diverifikasi")?;
    // Network files can block a polling worker indefinitely. Local DOS volumes
    // are the supported registration boundary; do not normalize UNC into one.
    let local = path
        .strip_prefix(r"\\?\")
        .ok_or("Daftarkan executable dari drive lokal")?;
    if local.as_bytes().get(1) != Some(&b':') {
        return Err("Daftarkan executable dari drive lokal".into());
    }
    let metadata = file
        .metadata()
        .map_err(|_| "Metadata executable tidak tersedia")?;
    let identity = format!(
        "{:016x}:{}:{:016x}:{:016x}",
        id.VolumeSerialNumber,
        id.FileId
            .Identifier
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>(),
        metadata.file_size(),
        metadata.last_write_time()
    );
    Ok(Executable { path, identity })
}

fn validate_pe(file: &mut File) -> Result<(), String> {
    let invalid = || "Pilih file executable Windows (.exe) yang valid".to_string();
    let mut dos = [0u8; 64];
    file.read_exact(&mut dos).map_err(|_| invalid())?;
    if &dos[..2] != b"MZ" {
        return Err(invalid());
    }
    let offset = u32::from_le_bytes(dos[60..64].try_into().unwrap()) as u64;
    if !(64..=16 * 1024 * 1024).contains(&offset) {
        return Err(invalid());
    }
    file.seek(SeekFrom::Start(offset)).map_err(|_| invalid())?;
    let mut pe = [0u8; 26];
    file.read_exact(&mut pe).map_err(|_| invalid())?;
    let flags = u16::from_le_bytes(pe[22..24].try_into().unwrap());
    let magic = u16::from_le_bytes(pe[24..26].try_into().unwrap());
    if &pe[..4] != b"PE\0\0"
        || flags & 0x0002 == 0
        || flags & 0x2000 != 0
        || ![0x10b, 0x20b].contains(&magic)
    {
        return Err(invalid());
    }
    Ok(())
}

pub struct Process {
    handle: OwnedHandle,
    pub pid: u32,
    pub created: u64,
    pub executable: Executable,
}
impl Process {
    pub fn open(pid: u32) -> Result<Self, String> {
        if pid == 0 || pid == std::process::id() {
            return Err("Proses tidak dapat diverifikasi".into());
        }
        let raw = unsafe {
            OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
                0,
                pid,
            )
        };
        if raw.is_null() {
            return Err("Proses tidak dapat diverifikasi".into());
        }
        // SAFETY: OpenProcess transferred one valid owned handle.
        let handle = unsafe { OwnedHandle::from_raw_handle(raw) };
        let mut creation = FILETIME::default();
        let mut exit = FILETIME::default();
        let mut kernel = FILETIME::default();
        let mut user = FILETIME::default();
        let mut buffer = vec![0u16; 32768];
        let mut length = buffer.len() as u32;
        if unsafe {
            GetProcessTimes(
                handle.as_raw_handle(),
                &mut creation,
                &mut exit,
                &mut kernel,
                &mut user,
            )
        } == 0
            || unsafe {
                QueryFullProcessImageNameW(
                    handle.as_raw_handle(),
                    0,
                    buffer.as_mut_ptr(),
                    &mut length,
                )
            } == 0
        {
            return Err("Proses tidak dapat diverifikasi".into());
        }
        let path = String::from_utf16(&buffer[..length as usize])
            .map_err(|_| "Proses tidak dapat diverifikasi")?;
        let executable = inspect(Path::new(&path))?;
        let created = ((creation.dwHighDateTime as u64) << 32) | creation.dwLowDateTime as u64;
        let process = Self {
            handle,
            pid,
            created,
            executable,
        };
        if created == 0
            || unsafe { WaitForSingleObject(process.handle.as_raw_handle(), 0) } != WAIT_TIMEOUT
        {
            return Err("Proses tidak dapat diverifikasi".into());
        }
        Ok(process)
    }
    pub fn alive(&self) -> bool {
        (unsafe { WaitForSingleObject(self.handle.as_raw_handle(), 0) }) != WAIT_OBJECT_0
    }
    pub fn verified_alive(&self) -> bool {
        (unsafe { WaitForSingleObject(self.handle.as_raw_handle(), 0) }) == WAIT_TIMEOUT
    }
    pub fn matches(&self, executable: &Executable) -> bool {
        self.executable.identity == executable.identity
            && self.executable.path.eq_ignore_ascii_case(&executable.path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        process::{Command, Stdio},
        time::Duration,
    };
    #[test]
    fn executable_identity_rejects_same_name_replacements_and_invalid_files() {
        let project = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let root = project
            .join("artifacts/games")
            .join(uuid::Uuid::new_v4().to_string());
        std::fs::create_dir_all(&root).unwrap();
        let root = std::fs::canonicalize(root).unwrap();
        assert!(root.starts_with(std::fs::canonicalize(project).unwrap()));
        let first = root.join("game.exe");
        let second = root.join("other.exe");
        std::fs::copy(std::env::current_exe().unwrap(), &first).unwrap();
        std::fs::copy(&first, &second).unwrap();
        let before = inspect(&first).unwrap();
        let renamed = root.join("original.exe");
        std::fs::rename(&first, &renamed).unwrap();
        std::fs::rename(&second, &first).unwrap();
        assert_ne!(inspect(&first).unwrap().identity, before.identity);
        assert_eq!(inspect(&renamed).unwrap().identity, before.identity);
        std::fs::write(root.join("text.exe"), b"not an executable").unwrap();
        assert!(inspect(&root.join("text.exe")).is_err());
        assert!(inspect(&root).is_err());
        assert!(inspect(Path::new("relative.exe")).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn native_process_handle_confirms_creation_identity_and_final_exit() {
        let system = std::env::var_os("SystemRoot").unwrap();
        let exe = Path::new(&system).join("System32/ping.exe");
        let expected = inspect(&exe).unwrap();
        struct ChildGuard(std::process::Child);
        impl Drop for ChildGuard {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let mut child = ChildGuard(
            Command::new(&exe)
                .args(["-n", "8", "127.0.0.1"])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );
        std::thread::sleep(Duration::from_millis(100));
        let process = Process::open(child.0.id()).unwrap();
        assert!(process.created > 0 && process.alive() && process.matches(&expected));
        let mut unrelated = expected.clone();
        unrelated.identity.push('0');
        assert!(!process.matches(&unrelated));
        unrelated = expected.clone();
        unrelated.path = Path::new(&unrelated.path)
            .with_file_name("different.exe")
            .to_string_lossy()
            .into_owned();
        assert!(!process.matches(&unrelated));
        child.0.kill().unwrap();
        child.0.wait().unwrap();
        assert!(!process.alive());
        assert!(Process::open(0).is_err());
        assert!(Process::open(std::process::id()).is_err());
    }
}
