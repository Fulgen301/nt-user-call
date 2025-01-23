//! Provides error enumerations.

use std::fmt::Display;

use windows::{
    core::HRESULT,
    Win32::Foundation::{
        ERROR_MOD_NOT_FOUND, ERROR_NOT_SUPPORTED, ERROR_OLD_WIN_VERSION, E_ILLEGAL_METHOD_CALL,
    },
};

#[repr(usize)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum UserCallError {
    OsNotSupported = 1,
    OsTooNew = 2,
    CallNotFound = 3,
    LibraryNotFound = 4,
}

impl Display for UserCallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OsNotSupported => write!(f, "The operating system is not supported."),
            Self::OsTooNew => write!(
                f,
                "The operating system does not use the NtUserCall* family of syscalls anymore."
            ),
            Self::CallNotFound => write!(f, "The function was not found."),
            Self::LibraryNotFound => write!(f, "A required library was not found."),
        }
    }
}

impl TryFrom<usize> for UserCallError {
    type Error = ();

    fn try_from(value: usize) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::OsNotSupported),
            2 => Ok(Self::OsTooNew),
            3 => Ok(Self::CallNotFound),
            4 => Ok(Self::LibraryNotFound),
            _ => Err(()),
        }
    }
}

impl From<UserCallError> for windows::core::Error {
    fn from(value: UserCallError) -> Self {
        match value {
            UserCallError::OsNotSupported => {
                Self::from_hresult(HRESULT::from_win32(ERROR_OLD_WIN_VERSION.0))
            }
            UserCallError::OsTooNew => Self::from_hresult(E_ILLEGAL_METHOD_CALL),
            UserCallError::CallNotFound => {
                Self::from_hresult(HRESULT::from_win32(ERROR_NOT_SUPPORTED.0))
            }
            UserCallError::LibraryNotFound => {
                Self::from_hresult(HRESULT::from_win32(ERROR_MOD_NOT_FOUND.0))
            }
        }
    }
}
