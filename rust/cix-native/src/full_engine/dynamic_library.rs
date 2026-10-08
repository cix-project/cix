//! Explicit-path dynamic-library loading for installed CIX provider bridges.
//!
//! This loader rejects non-absolute paths, never accepts a bare library name,
//! never searches `PATH`, and keeps the handle alive until every resolved
//! symbol is no longer in use.

use std::ffi::c_void;
use std::path::Path;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DynamicLibraryError {
    InvalidPath,
    LoadFailed,
    MissingSymbol(&'static str),
}

pub(crate) struct DynamicLibrary(*mut c_void);

impl DynamicLibrary {
    pub(crate) fn open(path: &Path) -> Result<Self, DynamicLibraryError> {
        // Path::is_absolute rejects Windows drive-relative forms such as C:foo
        // as well as bare names. The providers also validate this boundary,
        // but the loader must remain safe if another caller is added later.
        if !path.is_absolute() {
            return Err(DynamicLibraryError::InvalidPath);
        }
        #[cfg(unix)]
        {
            Self::open_unix(path)
        }
        #[cfg(windows)]
        {
            Self::open_windows(path)
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = path;
            Err(DynamicLibraryError::LoadFailed)
        }
    }

    /// The caller supplies the exact C ABI function-pointer type declared by
    /// its CIX-owned bridge header and keeps this handle alive while using it.
    pub(crate) unsafe fn symbol<T: Copy>(
        &self,
        name: &'static str,
    ) -> Result<T, DynamicLibraryError> {
        #[cfg(unix)]
        {
            self.symbol_unix(name)
        }
        #[cfg(windows)]
        {
            self.symbol_windows(name)
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = name;
            Err(DynamicLibraryError::MissingSymbol(name))
        }
    }

    #[cfg(unix)]
    fn open_unix(path: &Path) -> Result<Self, DynamicLibraryError> {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;

        let path = CString::new(path.as_os_str().as_bytes())
            .map_err(|_| DynamicLibraryError::InvalidPath)?;
        let handle = unsafe { libc::dlopen(path.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) };
        if handle.is_null() {
            Err(DynamicLibraryError::LoadFailed)
        } else {
            Ok(Self(handle))
        }
    }

    #[cfg(unix)]
    unsafe fn symbol_unix<T: Copy>(&self, name: &'static str) -> Result<T, DynamicLibraryError> {
        use std::ffi::CString;

        let text = CString::new(name).map_err(|_| DynamicLibraryError::MissingSymbol(name))?;
        let symbol = unsafe { libc::dlsym(self.0, text.as_ptr()) };
        if symbol.is_null() {
            Err(DynamicLibraryError::MissingSymbol(name))
        } else {
            Ok(unsafe { std::mem::transmute_copy(&symbol) })
        }
    }

    #[cfg(windows)]
    fn open_windows(path: &Path) -> Result<Self, DynamicLibraryError> {
        use std::os::windows::ffi::OsStrExt;

        const LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR: u32 = 0x0000_0100;
        const LOAD_LIBRARY_SEARCH_SYSTEM32: u32 = 0x0000_0800;
        let mut path: Vec<u16> = path.as_os_str().encode_wide().collect();
        if path.contains(&0) {
            return Err(DynamicLibraryError::InvalidPath);
        }
        path.push(0);
        // Restrict dependencies to the selected DLL's directory and System32.
        // This excludes the current directory, PATH, and application directory.
        // Staged bridge dependencies must be siblings of the selected bridge.
        let handle = unsafe {
            load_library_ex_w(
                path.as_ptr(),
                std::ptr::null_mut(),
                LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32,
            )
        };
        if handle.is_null() {
            Err(DynamicLibraryError::LoadFailed)
        } else {
            Ok(Self(handle))
        }
    }

    #[cfg(windows)]
    unsafe fn symbol_windows<T: Copy>(&self, name: &'static str) -> Result<T, DynamicLibraryError> {
        use std::ffi::CString;

        let text = CString::new(name).map_err(|_| DynamicLibraryError::MissingSymbol(name))?;
        let symbol = unsafe { get_proc_address(self.0, text.as_ptr()) };
        if symbol.is_null() {
            Err(DynamicLibraryError::MissingSymbol(name))
        } else {
            Ok(unsafe { std::mem::transmute_copy(&symbol) })
        }
    }
}

impl Drop for DynamicLibrary {
    fn drop(&mut self) {
        #[cfg(unix)]
        unsafe {
            libc::dlclose(self.0);
        }
        #[cfg(windows)]
        unsafe {
            free_library(self.0);
        }
    }
}

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    #[link_name = "LoadLibraryExW"]
    fn load_library_ex_w(file_name: *const u16, file: *mut c_void, flags: u32) -> *mut c_void;
    #[link_name = "GetProcAddress"]
    fn get_proc_address(module: *mut c_void, name: *const std::ffi::c_char) -> *mut c_void;
    #[link_name = "FreeLibrary"]
    fn free_library(module: *mut c_void) -> i32;
}
