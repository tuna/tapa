//! SLASH-dev libvrtd C ABI, loaded only for hardware VBINs.
//!
//! No SLASH headers or libraries are needed to build FRT. The declarations
//! below mirror `vrtd/vrtd.h`, `vrtd/wire.h`, and `slash/ctldev.h` from SLASH
//! dev revision 230fc629. Both service and compute use this one ABI.
//! The library is deliberately never unloaded: its function pointers remain
//! valid for every device for the process lifetime.

use std::ffi::{c_char, c_int, c_void, OsStr};
use std::sync::OnceLock;

use crate::dl::dl_sym;
use crate::error::{FrtError, Result};

pub(super) const CLOCK_REGION_USER: u32 = 1;
pub(super) const MM_CHANNEL_AUTO: c_int = 0;
pub(super) const ALLOC_TYPE_DDR: u32 = 0;
pub(super) const ALLOC_TYPE_HBM: u32 = 1;
pub(super) const ALLOC_TYPE_HBM_VNOC: u32 = 2;
pub(super) const ALLOC_DIR_BIDIRECTIONAL: u32 = 0;
pub(super) const ALLOC_DIR_HOST_TO_DEVICE: u32 = 1;
pub(super) const ALLOC_DIR_DEVICE_TO_HOST: u32 = 2;

/// `struct slash_bar_file` from SLASH-dev's `slash/ctldev.h`.
#[repr(C)]
pub(super) struct BarFile {
    pub map: *mut c_void,
    pub len: usize,
    pub fd: c_int,
    pub transport: c_int,
}

/// `struct vrtd_buffer` from SLASH-dev's `vrtd/vrtd.h`.
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
    pub qpair_count: u32,
    pub buffer_fd: c_int,
    pub transfer_hint: c_int,
    pub buf: *mut c_void,
    pub transfer_step_size: u64,
}

/// The entry points FRT uses, resolved once per process.
pub(super) struct Api {
    pub connect: unsafe extern "C" fn(*const c_char) -> c_int,
    pub get_device_by_bdf: unsafe extern "C" fn(c_int, *const c_char, *mut u32) -> c_int,
    pub design_write_file: unsafe extern "C" fn(c_int, u32, *const c_char, u8) -> c_int,
    pub clock_get_rate: unsafe extern "C" fn(c_int, u32, u32, *mut u32) -> c_int,
    pub clock_set_rate_round_down: unsafe extern "C" fn(c_int, u32, u32, u32, *mut u32) -> c_int,
    pub open_bar_file: unsafe extern "C" fn(c_int, u32, u8, *mut BarFile) -> c_int,
    pub close_bar_file: unsafe extern "C" fn(*mut BarFile),
    pub buffer_open:
        unsafe extern "C" fn(c_int, u32, u32, u32, u64, u64, c_int, *mut *mut VrtdBuffer) -> c_int,
    pub buffer_close: unsafe extern "C" fn(*mut VrtdBuffer) -> c_int,
    pub buffer_sync_to_device: unsafe extern "C" fn(*mut VrtdBuffer, u64, u64) -> c_int,
    pub buffer_sync_from_device: unsafe extern "C" fn(*mut VrtdBuffer, u64, u64) -> c_int,
}

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
    crate::dl::candidates_with_override(
        override_path,
        [
            SONAME.to_owned(),
            LIBRARY.to_owned(),
            format!("/usr/local/lib/{SONAME}"),
            format!("/usr/local/lib/{LIBRARY}"),
            format!("/usr/lib/{SONAME}"),
            format!("/usr/lib/{LIBRARY}"),
        ],
    )
}

fn load() -> std::result::Result<Api, String> {
    let library = crate::dl::open_library(
        &candidates(),
        libc::RTLD_NOW | libc::RTLD_LOCAL,
        &format!(
            "cannot load libvrtd; install SLASH's libvrtd or set {} to its path",
            frt_shm::env::FRT_VRTD_LIBRARY
        ),
    )?;
    // Every field is typed with the C signature in the pinned vrtd.h.
    Ok(Api {
        connect: dl_sym!(library, "libvrtd", "vrtd_connect"),
        get_device_by_bdf: dl_sym!(library, "libvrtd", "vrtd_get_device_by_bdf"),
        design_write_file: dl_sym!(library, "libvrtd", "vrtd_design_write_file"),
        clock_get_rate: dl_sym!(library, "libvrtd", "vrtd_clock_get_rate"),
        clock_set_rate_round_down: dl_sym!(library, "libvrtd", "vrtd_clock_set_rate_round_down"),
        open_bar_file: dl_sym!(library, "libvrtd", "vrtd_open_bar_file"),
        close_bar_file: dl_sym!(library, "libvrtd", "vrtd_close_bar_file"),
        buffer_open: dl_sym!(library, "libvrtd", "vrtd_buffer_open"),
        buffer_close: dl_sym!(library, "libvrtd", "vrtd_buffer_close"),
        buffer_sync_to_device: dl_sym!(library, "libvrtd", "vrtd_buffer_sync_to_device"),
        buffer_sync_from_device: dl_sym!(library, "libvrtd", "vrtd_buffer_sync_from_device"),
    })
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
        assert_eq!(std::mem::size_of::<BarFile>(), 24);
        assert_eq!(std::mem::offset_of!(BarFile, len), 8);
        assert_eq!(std::mem::offset_of!(BarFile, fd), 16);
        assert_eq!(std::mem::offset_of!(BarFile, transport), 20);

        assert_eq!(std::mem::size_of::<VrtdBuffer>(), 72);
        assert_eq!(std::mem::offset_of!(VrtdBuffer, alloc_arg), 16);
        assert_eq!(std::mem::offset_of!(VrtdBuffer, size), 24);
        assert_eq!(std::mem::offset_of!(VrtdBuffer, phys_addr), 32);
        assert_eq!(std::mem::offset_of!(VrtdBuffer, qpair_fd), 40);
        assert_eq!(std::mem::offset_of!(VrtdBuffer, qpair_count), 44);
        assert_eq!(std::mem::offset_of!(VrtdBuffer, buffer_fd), 48);
        assert_eq!(std::mem::offset_of!(VrtdBuffer, transfer_hint), 52);
        assert_eq!(std::mem::offset_of!(VrtdBuffer, buf), 56);
        assert_eq!(std::mem::offset_of!(VrtdBuffer, transfer_step_size), 64);
    }
}
