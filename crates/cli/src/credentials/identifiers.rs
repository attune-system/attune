use anyhow::{bail, Context, Result};
use std::collections::BTreeMap;

pub(super) fn policy() -> &'static str {
    #[cfg(target_os = "linux")]
    {
        "linux-v1"
    }
    #[cfg(target_os = "macos")]
    {
        "macos-v1"
    }
    #[cfg(target_os = "windows")]
    {
        "windows-v1"
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        "unsupported"
    }
}

pub(super) fn validate_minimum(ids: &BTreeMap<String, String>) -> Result<()> {
    for source in ["account", "hostname", "machine"] {
        let value = ids.get(source).or_else(|| {
            (source == "machine")
                .then(|| ids.get("machine-dmi"))
                .flatten()
        });
        if value.is_none_or(|s| s.trim().is_empty()) {
            bail!("Required system identifier unavailable; log in again or explicitly set credential_encryption to false for this profile");
        }
    }
    Ok(())
}

fn insert(ids: &mut BTreeMap<String, String>, name: &str, value: Option<String>) {
    if let Some(value) = value.filter(|s| !s.trim().is_empty()) {
        ids.insert(name.into(), value.trim().into());
    }
}

pub(super) fn collect() -> Result<BTreeMap<String, String>> {
    let mut ids = BTreeMap::new();
    let username = whoami::fallible::username().context("Cannot read OS account identity")?;
    #[cfg(unix)]
    let account = format!("{}/{}", unsafe { libc::geteuid() }, username);
    #[cfg(windows)]
    let account = windows::account(&username)?;
    #[cfg(not(any(unix, windows)))]
    let account = username;
    ids.insert("account".into(), account);
    ids.insert(
        "hostname".into(),
        hostname::get()?
            .into_string()
            .map_err(|_| anyhow::anyhow!("Invalid system hostname"))?,
    );
    #[cfg(target_os = "linux")]
    {
        use std::{fs, os::unix::fs::MetadataExt};
        // A container without machine-id can use its DMI identity, when exposed.
        // Never generate a substitute key file alongside the configuration.
        insert(
            &mut ids,
            "machine",
            fs::read_to_string("/etc/machine-id")
                .ok()
                .filter(|s| !s.trim().is_empty()),
        );
        insert(
            &mut ids,
            "machine-dmi",
            fs::read_to_string("/sys/class/dmi/id/product_uuid").ok(),
        );
        if let Ok(cpu) = fs::read_to_string("/proc/cpuinfo") {
            let mut fields = BTreeMap::new();
            for line in cpu.lines() {
                if let Some((key, value)) = line.split_once(':') {
                    if matches!(
                        key.trim(),
                        "vendor_id" | "model name" | "CPU implementer" | "CPU part" | "Hardware"
                    ) {
                        fields.entry(key.trim()).or_insert(value.trim());
                    }
                }
            }
            if !fields.is_empty() {
                insert(
                    &mut ids,
                    "cpu",
                    Some(format!(
                        "{}:{}",
                        std::env::consts::ARCH,
                        serde_json::to_string(&fields)?
                    )),
                );
            }
        }
        // Match root's device number rather than names such as /dev/sda.
        if let (Ok(root), Ok(entries)) = (fs::metadata("/"), fs::read_dir("/dev/disk/by-uuid")) {
            let mut matches = Vec::new();
            for entry in entries.flatten() {
                if fs::metadata(entry.path()).is_ok_and(|m| m.rdev() == root.dev()) {
                    matches.push(entry.file_name().to_string_lossy().into_owned());
                }
            }
            matches.sort();
            insert(&mut ids, "system-volume", matches.into_iter().next());
        }
    }
    #[cfg(target_os = "macos")]
    macos::collect(&mut ids);
    #[cfg(windows)]
    windows::collect(&mut ids)?;
    validate_minimum(&ids)?;
    Ok(ids)
}

#[cfg(target_os = "macos")]
mod macos {
    use super::*;
    use std::{
        ffi::{c_char, c_void, CStr, CString},
        ptr,
    };
    #[link(name = "IOKit", kind = "framework")]
    unsafe extern "C" {
        fn IOServiceMatching(name: *const c_char) -> *mut c_void;
        fn IOServiceGetMatchingService(port: u32, matching: *mut c_void) -> u32;
        fn IORegistryEntryCreateCFProperty(
            entry: u32,
            key: *const c_void,
            allocator: *const c_void,
            options: u32,
        ) -> *const c_void;
        fn IOObjectRelease(object: u32) -> i32;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFStringCreateWithCString(
            allocator: *const c_void,
            string: *const c_char,
            encoding: u32,
        ) -> *const c_void;
        fn CFStringGetCString(
            string: *const c_void,
            buffer: *mut c_char,
            size: isize,
            encoding: u32,
        ) -> u8;
        fn CFGetTypeID(value: *const c_void) -> usize;
        fn CFStringGetTypeID() -> usize;
        fn CFRelease(value: *const c_void);
    }
    fn platform_uuid() -> Option<String> {
        unsafe {
            let service = IOServiceGetMatchingService(
                0,
                IOServiceMatching(c"IOPlatformExpertDevice".as_ptr()),
            );
            if service == 0 {
                return None;
            }
            let key =
                CFStringCreateWithCString(ptr::null(), c"IOPlatformUUID".as_ptr(), 0x08000100);
            if key.is_null() {
                IOObjectRelease(service);
                return None;
            }
            let value = IORegistryEntryCreateCFProperty(service, key, ptr::null(), 0);
            CFRelease(key);
            IOObjectRelease(service);
            if value.is_null() {
                return None;
            }
            let mut buffer = [0 as c_char; 256];
            let ok = CFGetTypeID(value) == CFStringGetTypeID()
                && CFStringGetCString(
                    value,
                    buffer.as_mut_ptr(),
                    buffer.len() as isize,
                    0x08000100,
                ) != 0;
            CFRelease(value);
            ok.then(|| {
                CStr::from_ptr(buffer.as_ptr())
                    .to_string_lossy()
                    .into_owned()
            })
        }
    }
    fn sysctl(name: &str) -> Option<String> {
        let name = CString::new(name).ok()?;
        let mut bytes = [0u8; 1024];
        let mut len = bytes.len();
        let status = unsafe {
            libc::sysctlbyname(
                name.as_ptr(),
                bytes.as_mut_ptr().cast(),
                &mut len,
                ptr::null_mut(),
                0,
            )
        };
        (status == 0 && len <= bytes.len()).then(|| {
            String::from_utf8_lossy(&bytes[..len])
                .trim_end_matches('\0')
                .to_owned()
        })
    }
    fn volume_uuid() -> Option<String> {
        #[repr(C)]
        struct VolumeUuid {
            length: u32,
            uuid: [u8; 16],
        }
        let mut attrs: libc::attrlist = unsafe { std::mem::zeroed() };
        attrs.bitmapcount = 5;
        attrs.volattr = libc::ATTR_VOL_INFO | libc::ATTR_VOL_UUID;
        let mut value = VolumeUuid {
            length: 0,
            uuid: [0; 16],
        };
        let result = unsafe {
            libc::getattrlist(
                c"/".as_ptr(),
                (&mut attrs as *mut libc::attrlist).cast(),
                (&mut value as *mut VolumeUuid).cast(),
                std::mem::size_of::<VolumeUuid>(),
                0,
            )
        };
        (result == 0 && value.uuid != [0; 16])
            .then(|| value.uuid.iter().map(|b| format!("{b:02x}")).collect())
    }
    pub(super) fn collect(ids: &mut BTreeMap<String, String>) {
        insert(ids, "machine", platform_uuid());
        insert(ids, "cpu-brand", sysctl("machdep.cpu.brand_string"));
        insert(ids, "hardware-model", sysctl("hw.model"));
        insert(ids, "system-volume", volume_uuid());
    }
}

#[cfg(windows)]
mod windows {
    use super::*;
    use std::ptr;
    use windows_sys::Win32::{
        Foundation::{CloseHandle, LocalFree},
        Security::{
            Authorization::ConvertSidToStringSidW, GetTokenInformation, TokenUser, TOKEN_QUERY,
            TOKEN_USER,
        },
        Storage::FileSystem::GetVolumeNameForVolumeMountPointW,
        System::{
            Registry::*,
            SystemInformation::GetWindowsDirectoryW,
            Threading::{GetCurrentProcess, OpenProcessToken},
        },
    };
    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(Some(0)).collect()
    }
    fn registry(path: &str, name: &str) -> Option<String> {
        let mut bytes = [0u16; 1024];
        let mut len = (bytes.len() * 2) as u32;
        let status = unsafe {
            RegGetValueW(
                HKEY_LOCAL_MACHINE,
                wide(path).as_ptr(),
                wide(name).as_ptr(),
                RRF_RT_REG_SZ | RRF_SUBKEY_WOW6464KEY,
                ptr::null_mut(),
                bytes.as_mut_ptr().cast(),
                &mut len,
            )
        };
        (status == 0 && len >= 2 && len as usize <= bytes.len() * 2)
            .then(|| String::from_utf16_lossy(&bytes[..(len as usize / 2) - 1]))
    }
    pub(super) fn account(username: &str) -> Result<String> {
        unsafe {
            let mut token = ptr::null_mut();
            if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
                bail!("Cannot read OS account identity");
            }
            let mut len = 0;
            GetTokenInformation(token, TokenUser, ptr::null_mut(), 0, &mut len);
            // Aligned storage for TOKEN_USER and SID bytes.
            let mut data = vec![0usize; (len as usize).div_ceil(std::mem::size_of::<usize>())];
            let ok = GetTokenInformation(token, TokenUser, data.as_mut_ptr().cast(), len, &mut len);
            CloseHandle(token);
            if ok == 0 {
                bail!("Cannot read OS account identity");
            }
            let user = &*data.as_ptr().cast::<TOKEN_USER>();
            let mut sid = ptr::null_mut();
            if ConvertSidToStringSidW(user.User.Sid, &mut sid) == 0 {
                bail!("Cannot read OS account identity");
            }
            let mut n = 0;
            while *sid.add(n) != 0 {
                n += 1;
            }
            let value = String::from_utf16_lossy(std::slice::from_raw_parts(sid, n));
            LocalFree(sid.cast());
            Ok(format!("{value}/{username}"))
        }
    }
    pub(super) fn collect(ids: &mut BTreeMap<String, String>) -> Result<()> {
        insert(
            ids,
            "machine",
            registry(r"SOFTWARE\Microsoft\Cryptography", "MachineGuid"),
        );
        insert(
            ids,
            "cpu",
            registry(
                r"HARDWARE\DESCRIPTION\System\CentralProcessor\0",
                "ProcessorNameString",
            ),
        );
        let mut path = [0u16; 1024];
        let n = unsafe { GetWindowsDirectoryW(path.as_mut_ptr(), path.len() as u32) } as usize;
        if n >= 3 && n < path.len() {
            let root = [path[0], path[1], b'\\' as u16, 0];
            let mut volume = [0u16; 128];
            if unsafe {
                GetVolumeNameForVolumeMountPointW(
                    root.as_ptr(),
                    volume.as_mut_ptr(),
                    volume.len() as u32,
                )
            } != 0
            {
                let n = volume.iter().position(|c| *c == 0).unwrap_or(volume.len());
                insert(
                    ids,
                    "system-volume",
                    Some(String::from_utf16_lossy(&volume[..n])),
                );
            }
        }
        Ok(())
    }
}
