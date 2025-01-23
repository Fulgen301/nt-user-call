//! This library provides bindings to all functions accessible via the `NtUserCall*` family of system calls.
//!
//! Up until Windows 11, a bunch of system calls were grouped together into a dispatch table, `apfnSimpleCall`,
//! and invoked by calling a dedicated family of syscalls with the respective index of that function. For example,
//! `CreateMenu` would be called via `NtUserCallNoParam(0);`, with `0` being its index in the dispatch table in
//! all supported Windows versions. However, the number of functions and their indices in that table varied between
//! Windows versions, Windows versions prior to Windows 10 did not export the `NtUserCall*` family of syscalls,
//! and the dispatch table was removed in Windows 11 in its entirety, with all functions being converted to syscalls
//! exported from win32u.dll.
//!
//! This library provides a unified interface to all of these functions by abstracting away of the differences between
//! indices, syscall availability and exported syscalls in Windows 11.

#![deny(clippy::undocumented_unsafe_blocks)]

pub mod error;
pub mod functions;
pub mod indices;
pub mod macros;
pub mod version;
