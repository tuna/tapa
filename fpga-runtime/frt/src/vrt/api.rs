//! The pinned libvrtd C ABI, loaded only when a hardware VBIN is opened.
//!
//! No SLASH headers or libraries are needed to build FRT. The declarations
//! below mirror `vrtd/vrtd.h`, `vrtd/wire.h`, and `slash/ctldev.h` from the
//! pinned SLASH release. The library is deliberately never unloaded: its
//! function pointers remain valid for every device for the process lifetime.

use std::ffi::{c_char, c_int, c_void, CStr, CString, OsStr};
use std::sync::OnceLock;

use crate::error::{FrtError, Result};

pub(super) const CLOCK_REGION_USER: u32 = 1;
pub(super) const ALLOC_TYPE_DDR: u32 = 0;
pub(super) const ALLOC_TYPE_HBM: u32 = 1;
pub(super) const ALLOC_TYPE_HBM_VNOC: u32 = 2;
pub(super) const ALLOC_DIR_BIDIRECTIONAL: u32 = 0;
pub(super) const ALLOC_DIR_HOST_TO_DEVICE: u32 = 1;
pub(super) const ALLOC_DIR_DEVICE_TO_HOST: u32 = 2;

/// `struct slash_bar_file` from the pinned `slash/ctldev.h`.
#[repr(C)]
pub(super) struct BarFile {
    pub map: *mut c_void,
    pub len: usize,
    pub fd: c_int,
    /// C `_Bool`; an integer avoids importing invalid non-zero representations
    /// into Rust's `bool` if a future C implementation uses another value.
    pub mock: u8,
    pub mock_path: *mut c_char,
}

/// `struct vrtd_buffer` from the pinned `vrtd/vrtd.h`.
#[repr(C)]
pub(super) struct VrtdBuffer {
    pub sock_fd: c_int,
    pub dev: u32,
    pub alloc_type: u32,
    pub alloc_dir: u32,
    pub alloc_arg: u64,
    pub size: u64,
    pub phys_addr: u64,
    pub qpair_fd: c_int,
    pub buf: *mut c_void,
}

/// The entry points FRT uses, resolved once per process.
pub(super) struct Api {
    pub connect: unsafe extern "C" fn(*const c_char) -> c_int,
    pub get_device_by_bdf: unsafe extern "C" fn(c_int, *const c_char, *mut u32) -> c_int,
    pub design_write_file: unsafe extern "C" fn(c_int, u32, *const c_char) -> c_int,
    pub clock_get_rate: unsafe extern "C" fn(c_int, u32, u32, *mut u32) -> c_int,
    pub clock_set_rate: unsafe extern "C" fn(c_int, u32, u32, u32, *mut u32) -> c_int,
    pub open_bar_file: unsafe extern "C" fn(c_int, u32, u8, *mut BarFile) -> c_int,
    pub close_bar_file: unsafe extern "C" fn(*mut BarFile),
    pub buffer_open:
        unsafe extern "C" fn(c_int, u32, u32, u32, u64, u64, *mut *mut VrtdBuffer) -> c_int,
    pub buffer_close: unsafe extern "C" fn(*mut VrtdBuffer) -> c_int,
    pub buffer_sync_to_device: unsafe extern "C" fn(*mut VrtdBuffer, u64, u64) -> c_int,
    pub buffer_sync_from_device: unsafe extern "C" fn(*mut VrtdBuffer, u64, u64) -> c_int,
}

// SAFETY: these are immutable function pointers into a library intentionally
// kept loaded for the process lifetime. libvrtd serializes independent calls
// on each connection; VrtDevice itself is only accessed exclusively.
unsafe impl Send for Api {}
// SAFETY: as above; Api has no interior mutability.
unsafe impl Sync for Api {}

const LIBRARY: &str = "libvrtd.so";
const SONAME: &str = "libvrtd.so.1";

/// Load libvrtd on first hardware use. Simulation and archive parsing do not
/// call this function and therefore need neither a board nor a SLASH install.
pub(super) fn api() -> Result<&'static Api> {
    static API: OnceLock<std::result::Result<Api, String>> = OnceLock::new();
    API.get_or_init(load)
        .as_ref()
        .map_err(|reason| FrtError::Vrt(reason.clone()))
}

fn candidates() -> Vec<String> {
    candidates_with_override(std::env::var_os(frt_shm::env::FRT_VRTD_LIBRARY).as_deref())
}

/// The explicit path wins. The remaining entries cover the loader's soname
/// search and SLASH's documented `/usr/local` and system install prefixes.
fn candidates_with_override(override_path: Option<&OsStr>) -> Vec<String> {
    let mut paths = Vec::new();
    if let Some(path) = override_path.filter(|path| !path.is_empty()) {
        paths.push(path.to_string_lossy().into_owned());
    }
    paths.extend([
        SONAME.to_owned(),
        LIBRARY.to_owned(),
        format!("/usr/local/lib/{SONAME}"),
        format!("/usr/local/lib/{LIBRARY}"),
        format!("/usr/lib/{SONAME}"),
        format!("/usr/lib/{LIBRARY}"),
    ]);
    paths
}

fn open_library() -> std::result::Result<*mut c_void, String> {
    let mut reasons = Vec::new();
    for candidate in candidates() {
        let name = CString::new(candidate.clone())
            .map_err(|error| format!("bad libvrtd path `{candidate}`: {error}"))?;
        // SAFETY: name is NUL-terminated. A successful handle is intentionally
        // leaked, so all resolved entry points stay valid for the process.
        let library = unsafe { libc::dlopen(name.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) };
        if !library.is_null() {
            return Ok(library);
        }
        // SAFETY: dlerror returns either null or a borrowed NUL-terminated
        // thread-local string, copied here before another loader call.
        let error = unsafe { libc::dlerror() };
        let reason = if error.is_null() {
            "unknown loader error".to_owned()
        } else {
            // SAFETY: a non-null dlerror result is a C string.
            unsafe { CStr::from_ptr(error) }
                .to_string_lossy()
                .into_owned()
        };
        reasons.push(format!("{candidate}: {reason}"));
    }
    Err(format!(
        "cannot load libvrtd; install SLASH's libvrtd or set {} to its path: {}",
        frt_shm::env::FRT_VRTD_LIBRARY,
        reasons.join("; ")
    ))
}

fn load() -> std::result::Result<Api, String> {
    let library = open_library()?;
    // Every field is typed with the C signature in the pinned vrtd.h.
    macro_rules! sym {
        ($name:literal) => {
            // SAFETY: the initialized field has the symbol's pinned C type.
            unsafe { resolve(library, $name) }?
        };
    }
    Ok(Api {
        connect: sym!("vrtd_connect"),
        get_device_by_bdf: sym!("vrtd_get_device_by_bdf"),
        design_write_file: sym!("vrtd_design_write_file"),
        clock_get_rate: sym!("vrtd_clock_get_rate"),
        clock_set_rate: sym!("vrtd_clock_set_rate"),
        open_bar_file: sym!("vrtd_open_bar_file"),
        close_bar_file: sym!("vrtd_close_bar_file"),
        buffer_open: sym!("vrtd_buffer_open"),
        buffer_close: sym!("vrtd_buffer_close"),
        buffer_sync_to_device: sym!("vrtd_buffer_sync_to_device"),
        buffer_sync_from_device: sym!("vrtd_buffer_sync_from_device"),
    })
}

/// Resolve one symbol as its pinned C function type.
///
/// # Safety
/// `F` must exactly match the named symbol's C signature.
unsafe fn resolve<F: Copy>(library: *mut c_void, name: &str) -> std::result::Result<F, String> {
    assert_eq!(
        std::mem::size_of::<F>(),
        std::mem::size_of::<*mut c_void>(),
        "F must be a function pointer"
    );
    let symbol = CString::new(name).map_err(|error| format!("bad symbol `{name}`: {error}"))?;
    // SAFETY: library is a live dlopen handle and symbol is NUL-terminated.
    let pointer = unsafe { libc::dlsym(library, symbol.as_ptr()) };
    if pointer.is_null() {
        return Err(format!(
            "libvrtd has no `{name}` required by the pinned C API"
        ));
    }
    // SAFETY: the caller guarantees F is the symbol's function pointer type;
    // the size assertion rules out a data pointer or differently sized value.
    Ok(unsafe { std::mem::transmute_copy::<*mut c_void, F>(&pointer) })
}

#[cfg(test)]
mod tests {
    use super::{candidates_with_override, BarFile, VrtdBuffer, LIBRARY, SONAME};
    use std::ffi::OsStr;

    #[test]
    fn an_explicit_library_precedes_the_soname_and_install_paths() {
        let paths = candidates_with_override(Some(OsStr::new("/board/libvrtd-custom.so")));
        assert_eq!(paths[0], "/board/libvrtd-custom.so");
        assert_eq!(paths[1], SONAME);
        assert_eq!(paths[2], LIBRARY);
        assert!(paths.contains(&format!("/usr/local/lib/{SONAME}")));
        assert!(paths.contains(&format!("/usr/lib/{LIBRARY}")));

        let defaults = candidates_with_override(None);
        assert_eq!(defaults[0], SONAME);
        assert!(!defaults.iter().any(|path| path.contains("custom")));
    }

    #[test]
    #[cfg(target_pointer_width = "64")]
    fn ffi_structs_match_the_pinned_64_bit_layouts() {
        assert_eq!(std::mem::size_of::<BarFile>(), 32);
        assert_eq!(std::mem::offset_of!(BarFile, len), 8);
        assert_eq!(std::mem::offset_of!(BarFile, fd), 16);
        assert_eq!(std::mem::offset_of!(BarFile, mock), 20);
        assert_eq!(std::mem::offset_of!(BarFile, mock_path), 24);

        assert_eq!(std::mem::size_of::<VrtdBuffer>(), 56);
        assert_eq!(std::mem::offset_of!(VrtdBuffer, alloc_arg), 16);
        assert_eq!(std::mem::offset_of!(VrtdBuffer, size), 24);
        assert_eq!(std::mem::offset_of!(VrtdBuffer, phys_addr), 32);
        assert_eq!(std::mem::offset_of!(VrtdBuffer, qpair_fd), 40);
        assert_eq!(std::mem::offset_of!(VrtdBuffer, buf), 48);
    }
}
