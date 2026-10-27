//! SLASH VBIN backends.

mod api;
mod device;
mod vbin;

use crate::device::Device;
use crate::error::{FrtError, Result};
use std::path::Path;
use vbin::{Platform, Vbin};

/// Extract once and dispatch on the platform recorded in `system_map.xml`.
/// Parsing and rejection happen before any attempt to load libvrtd.
pub(crate) fn open(path: &Path) -> Result<Box<dyn Device>> {
    let vbin = Vbin::open(path)?;
    match vbin.platform {
        Platform::Hardware => Ok(Box::new(device::VrtDevice::open(&vbin)?)),
        Platform::Simulation => Err(FrtError::Vrt(
            "SLASH simulation VBINs are unsupported; this build supports hardware VBINs only"
                .into(),
        )),
        Platform::Emulation => Err(FrtError::Vrt(
            "SLASH emulation VBINs are unsupported: TAPA packages contain RTL, not HLS C models"
                .into(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::{open, Platform};
    use std::path::Path;
    use tar::{Builder, Header};

    fn vbin(path: &Path, platform: Platform) {
        let platform = match platform {
            Platform::Hardware => "Hardware",
            Platform::Simulation => "Simulation",
            Platform::Emulation => "Emulation",
        };
        let xml = format!(
            "<SystemMap><Platform>{platform}</Platform><ClockFrequency>300000000</ClockFrequency><Kernel><Name>k</Name><BaseAddress>0x20200000000</BaseAddress><Range>0x1000</Range></Kernel></SystemMap>"
        );
        let mut bytes = Vec::new();
        let mut builder = Builder::new(&mut bytes);
        let mut header = Header::new_gnu();
        header.set_size(xml.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder
            .append_data(&mut header, "system_map.xml", xml.as_bytes())
            .expect("append system map");
        builder.finish().expect("finish VBIN");
        drop(builder);
        std::fs::write(path, bytes).expect("write VBIN");
    }

    #[test]
    fn non_hardware_modes_are_rejected_without_loading_libvrtd() {
        for (platform, expected) in [
            (Platform::Simulation, "unsupported"),
            (Platform::Emulation, "unsupported"),
        ] {
            let temp = tempfile::tempdir().expect("temp dir");
            let path = temp.path().join("design.vbin");
            vbin(&path, platform);
            let Err(error) = open(&path) else {
                panic!("non-hardware VBIN unexpectedly opened");
            };
            assert!(error.to_string().contains(expected), "{error}");
        }
    }
}
