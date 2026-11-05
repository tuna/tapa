//! The dlopen/dlsym scaffolding shared by the lazily loaded C libraries
//! (XRT, libvrtd, libzmq): each backend keeps only its candidate list and
//! symbol table.

use std::ffi::{c_int, c_void, CStr, CString};

/// Open the first loadable library among `candidates`, reporting every
/// loader reason in order. `header` leads the error when none loads.
pub fn open_library(
    candidates: &[String],
    flags: c_int,
    header: &str,
) -> Result<*mut c_void, String> {
    let mut reasons = Vec::new();
    for candidate in candidates {
        let name = CString::new(candidate.as_str())
            .map_err(|error| format!("bad library path `{candidate}`: {error}"))?;
        // SAFETY: name is NUL-terminated. A successful handle is intentionally
        // never closed, so every resolved pointer stays valid for the process.
        let library = unsafe { libc::dlopen(name.as_ptr(), flags) };
        if !library.is_null() {
            return Ok(library);
        }
        // SAFETY: dlerror returns null or a borrowed NUL-terminated
        // thread-local string, copied here before another loader call.
        let error = unsafe { libc::dlerror() };
        if !error.is_null() {
            // SAFETY: a non-null dlerror result is a C string.
            let reason = unsafe { CStr::from_ptr(error) }
                .to_string_lossy()
                .into_owned();
            reasons.push(format!("{candidate}: {reason}"));
        }
    }
    Err(format!("{header}: {}", reasons.join("; ")))
}

/// The `name` symbol of `library` as a function pointer of type `F`.
///
/// # Safety
/// `F` must be the symbol's C function signature.
pub unsafe fn resolve<F: Copy>(
    library: *mut c_void,
    library_name: &str,
    name: &str,
) -> Result<F, String> {
    assert_eq!(
        std::mem::size_of::<F>(),
        std::mem::size_of::<*mut c_void>(),
        "`F` must be a function pointer"
    );
    let symbol = CString::new(name).map_err(|error| format!("bad symbol `{name}`: {error}"))?;
    // SAFETY: library is a live dlopen handle and symbol is NUL-terminated.
    let pointer = unsafe { libc::dlsym(library, symbol.as_ptr()) };
    if pointer.is_null() {
        return Err(format!("{library_name} has no `{name}`"));
    }
    // SAFETY: the caller guarantees `F` is the symbol's function type, and
    // the sizes match (asserted above).
    Ok(unsafe { std::mem::transmute_copy::<*mut c_void, F>(&pointer) })
}

/// Resolve one symbol into an Api field, propagating the load error.
macro_rules! dl_sym {
    ($library:expr, $library_name:expr, $name:literal) => {
        // SAFETY: the field this initializes is typed as the symbol's pinned
        // C signature.
        unsafe { $crate::dl::resolve($library, $library_name, $name) }?
    };
}

pub(crate) use dl_sym;
