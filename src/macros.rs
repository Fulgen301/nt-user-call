#[macro_export]
macro_rules! load_runtime_fn_body {
    (
        [ $library:literal ] $name:ident ($($paramname:ident: $paramtype:ty),*) -> $rettype:ty
    ) => {{
        use ::std::sync::atomic::{AtomicPtr, Ordering};
        use ::windows::{core::{w, PCSTR}, Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress}};
        use $crate::error::UserCallError;

        type Function = unsafe extern "system" fn($($paramtype),*) -> $rettype;
        static FUNCTION: AtomicPtr<::std::ffi::c_void> = AtomicPtr::new(std::ptr::null_mut());

        let mut ptr = FUNCTION.load(Ordering::Relaxed);

        if ptr.is_null() {
            // SAFETY: On success, GetModuleHandleW returns a valid module handle
            let win32u = match unsafe { GetModuleHandleW(w!($library)) } {
                Ok(library) => library,
                Err(_) => {
                    _ = FUNCTION.compare_exchange(std::ptr::null_mut(), UserCallError::LibraryNotFound as usize as _, Ordering::AcqRel, Ordering::Acquire);
                    return Err(UserCallError::LibraryNotFound);
                }
            };

            // SAFETY: GetProcAddress returns a valid function pointer if the function exists.
            ptr = match unsafe { GetProcAddress(win32u, PCSTR(concat!("NtUser", stringify!($name), "\u{0}").as_ptr())) } {
                Some(f) => f,
                None => {
                    _ = FUNCTION.compare_exchange(std::ptr::null_mut(), UserCallError::CallNotFound as usize as _, Ordering::AcqRel, Ordering::Acquire);
                    return Err(UserCallError::CallNotFound);
                }
            } as _;
        }

        else if (ptr as usize) < u16::MAX as usize {
            // SAFETY: All possible error values have been written by the compare_exchange calls above and are variants of UserCallError
            return Err(unsafe {
                UserCallError::try_from(ptr as usize).unwrap_unchecked()
            });
        }

        // SAFETY: All non-function values have been handled and the pointer is valid function pointer
        let function: Function = unsafe {
            std::mem::transmute(ptr)
        };

        // SAFETY: `function` is a valid function pointer
        Ok(unsafe {
            function($($paramname),*)
        })
    }}
}

pub(crate) use load_runtime_fn_body;

#[macro_export]
macro_rules! load_runtime_fn {
    (
        [ $library:literal ] $abi:literal $vis:vis fn $name:ident ($($paramname:ident: $paramtype:ty),*) -> $rettype:ty
    ) => {
        $vis unsafe extern $abi fn $name($($paramname: $paramtype),*) -> Result<$rettype, $crate::error::UserCallError> {
            $crate::load_runtime_fn_body!([ $library ] $name ($($paramname: $paramtype),*) -> $rettype)
        }
    }
}

pub use load_runtime_fn;
