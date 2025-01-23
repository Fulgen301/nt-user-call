use std::sync::{LazyLock, OnceLock};

use windows::{
    Wdk::System::SystemServices::RtlGetVersion, Win32::System::SystemInformation::OSVERSIONINFOW,
};

use crate::error::UserCallError;

#[repr(u8)]
#[derive(Copy, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum OsVersion {
    #[cfg(any(target_vendor = "win7", feature = "all_os_versions"))]
    Win7,
    Win8,
    Win81,
    Win10,
}

static OS_VERSION: OnceLock<Result<OsVersion, UserCallError>> = OnceLock::new();

static HAS_DEDICATED_SYSCALLS: LazyLock<bool> =
    LazyLock::new(|| matches!(get_os_version(), Err(UserCallError::OsTooNew)));

pub(crate) fn get_os_version() -> Result<OsVersion, UserCallError> {
    *OS_VERSION.get_or_init(|| {
        let mut version_info = OSVERSIONINFOW {
            dwOSVersionInfoSize: std::mem::size_of::<OSVERSIONINFOW>() as _,
            ..Default::default()
        };

        // SAFETY: `version_info` is initialized with the correct size.
        unsafe {
            RtlGetVersion(&raw mut version_info).ok().unwrap();
        }

        map_os_version_info(version_info)
    })
}

fn map_os_version_info(version_info: OSVERSIONINFOW) -> Result<OsVersion, UserCallError> {
    match version_info {
        OSVERSIONINFOW {
            dwMajorVersion: ..6,
            ..
        } => Err(UserCallError::OsNotSupported),
        #[cfg(any(target_vendor = "win7", feature = "all_os_versions"))]
        OSVERSIONINFOW {
            dwMajorVersion: 6,
            dwMinorVersion: 1,
            ..
        } => Ok(OsVersion::Win7),
        OSVERSIONINFOW {
            dwMajorVersion: 6,
            dwMinorVersion: 2,
            ..
        } => Ok(OsVersion::Win8),
        OSVERSIONINFOW {
            dwMajorVersion: 6,
            dwMinorVersion: 3,
            ..
        } => Ok(OsVersion::Win81),
        OSVERSIONINFOW {
            dwMajorVersion: 10,
            dwMinorVersion: 0,
            dwBuildNumber: ..20292,
            ..
        } => Ok(OsVersion::Win10),
        OSVERSIONINFOW {
            dwMajorVersion: 10..,
            dwMinorVersion: 0,
            ..
        } => Err(UserCallError::OsTooNew),
        _ => Err(UserCallError::OsNotSupported),
    }
}

#[cfg(test)]
pub fn set_os_version(
    os_version: Result<OsVersion, UserCallError>,
) -> Result<(), Result<OsVersion, UserCallError>> {
    OS_VERSION.set(os_version)
}

#[cfg(test)]
pub fn set_os_version_info(
    version_info: OSVERSIONINFOW,
) -> Result<(), Result<OsVersion, UserCallError>> {
    OS_VERSION.set(map_os_version_info(version_info))
}

pub(crate) fn has_dedicated_syscalls() -> bool {
    *HAS_DEDICATED_SYSCALLS
}

#[cfg(test)]
mod test {
    use windows::{
        core::{s, w, Owned},
        Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW},
    };

    use crate::version::has_dedicated_syscalls;

    #[test]
    pub fn has_dedicated_syscalls_should_match_dll() {
        let win32u =
            // SAFETY: `LoadLibraryW` is called with a valid LPCWSTR.
            unsafe { Owned::new(LoadLibraryW(w!("win32u.dll")).expect("Could not load win32u")) };

        // SAFETY: `GetProcAddress` is called with a valid HMODULE and a valid LPCSTR.
        let function = unsafe { GetProcAddress(*win32u, s!("NtUserGetInputEvent")) };

        assert_eq!(function.is_some(), has_dedicated_syscalls());
    }
}
