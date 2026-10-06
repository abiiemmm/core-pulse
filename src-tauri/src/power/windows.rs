//! Read and select existing schemes through the supported Windows power API.
use crate::{hardware, models::PowerPlan};
use super::PowerBackend;
use std::ffi::c_void;
use uuid::Uuid;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Guid { first: u32, second: u16, third: u16, rest: [u8; 8] }
impl Guid {
    fn parse(value: &str) -> Result<Self, String> {
        let id = Uuid::parse_str(value).map_err(|_| "GUID power plan tidak valid")?;
        let (first, second, third, rest) = id.as_fields();
        Ok(Self { first, second, third, rest: *rest })
    }
    fn text(&self) -> String { Uuid::from_fields(self.first, self.second, self.third, &self.rest).to_string() }
}

#[link(name = "powrprof")]
extern "system" {
    fn PowerGetActiveScheme(root: *mut c_void, scheme: *mut *mut Guid) -> u32;
    fn PowerSetActiveScheme(root: *mut c_void, scheme: *const Guid) -> u32;
    fn PowerEnumerate(root: *mut c_void, scheme: *const Guid, subgroup: *const Guid, access: u32, index: u32, buffer: *mut u8, size: *mut u32) -> u32;
    fn PowerReadFriendlyName(root: *mut c_void, scheme: *const Guid, subgroup: *const Guid, setting: *const Guid, buffer: *mut u8, size: *mut u32) -> u32;
}
#[link(name = "kernel32")]
extern "system" { fn LocalFree(memory: *mut c_void) -> *mut c_void; }

fn windows_error(code: u32) -> String {
    if code == 5 { "Windows menolak izin perubahan atau pembacaan skema daya".into() }
    else { format!("Operasi skema daya Windows gagal (kode {code})") }
}

pub struct WindowsPower;
impl PowerBackend for WindowsPower {
    fn active(&mut self) -> Result<String, String> {
        let mut scheme = std::ptr::null_mut();
        // SAFETY: null root is required; Windows allocates the GUID on success.
        let status = unsafe { PowerGetActiveScheme(std::ptr::null_mut(), &mut scheme) };
        if status != 0 { return Err(windows_error(status)); }
        if scheme.is_null() { return Err("GUID power plan aktif tidak ditemukan".into()); }
        // SAFETY: the returned allocation contains a GUID and must use LocalFree.
        let guid = unsafe { let guid = (*scheme).text(); LocalFree(scheme.cast()); guid };
        Ok(guid)
    }

    fn plans(&mut self) -> Result<Vec<PowerPlan>, String> {
        let active = self.active()?;
        let mut plans = Vec::new();
        // Bound enumeration even if an OS/provider behaves incorrectly.
        for index in 0..256 {
            let mut guid = Guid::default();
            let mut size = std::mem::size_of::<Guid>() as u32;
            // ACCESS_SCHEME (16) enumerates GUIDs; both subgroup pointers are null.
            // SAFETY: buffer points to a GUID and has the advertised 16-byte size.
            let status = unsafe { PowerEnumerate(std::ptr::null_mut(), std::ptr::null(), std::ptr::null(), 16, index, (&mut guid as *mut Guid).cast(), &mut size) };
            if status == 259 { return Ok(plans); } // ERROR_NO_MORE_ITEMS
            if status != 0 { return Err(windows_error(status)); }
            if size != std::mem::size_of::<Guid>() as u32 { return Err("Data skema daya Windows tidak valid".into()); }
            let key = guid.text();
            plans.push(PowerPlan { active: key == active, guid: key, name: friendly_name(&guid).unwrap_or_else(|| "Windows power scheme".into()) });
        }
        Err("Jumlah skema daya Windows melebihi batas pembacaan".into())
    }

    fn set_active(&mut self, guid: &str) -> Result<(), String> {
        let scheme = Guid::parse(guid)?;
        // This changes only the existing scheme selected by the stored profile.
        // SAFETY: valid GUID pointer; the reserved root parameter is null.
        let status = unsafe { PowerSetActiveScheme(std::ptr::null_mut(), &scheme) };
        if status != 0 { Err(windows_error(status)) } else { Ok(()) }
    }

    fn power_source(&mut self) -> String { hardware::power_source() }
}

fn friendly_name(guid: &Guid) -> Option<String> {
    let mut bytes = 0;
    // SAFETY: the first call requests the UTF-16 name's required byte count.
    let status = unsafe { PowerReadFriendlyName(std::ptr::null_mut(), guid, std::ptr::null(), std::ptr::null(), std::ptr::null_mut(), &mut bytes) };
    if (status != 0 && status != 234) || bytes == 0 || bytes > 8192 || bytes % 2 != 0 { return None; }
    let mut buffer = vec![0_u16; bytes as usize / 2];
    // SAFETY: the allocated buffer has the requested byte capacity.
    let status = unsafe { PowerReadFriendlyName(std::ptr::null_mut(), guid, std::ptr::null(), std::ptr::null(), buffer.as_mut_ptr().cast(), &mut bytes) };
    if status != 0 || bytes as usize > buffer.len() * 2 || bytes % 2 != 0 { return None; }
    let valid = &buffer[..bytes as usize / 2];
    let end = valid.iter().position(|value| *value == 0).unwrap_or(valid.len());
    String::from_utf16(&valid[..end]).ok().filter(|name| !name.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_reads_existing_schemes_without_mutating_them() {
        let mut backend = WindowsPower;
        let original = backend.active().unwrap();
        let plans = backend.plans().unwrap();
        assert!(plans.iter().any(|plan| plan.guid == original && plan.active));
        assert!(plans.iter().all(|plan| Uuid::parse_str(&plan.guid).is_ok() && !plan.name.is_empty()));
        assert_eq!(backend.active().unwrap(), original);
    }
}
