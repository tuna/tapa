//! SLASH VBIN backends.

mod api;
mod device;
mod sim;
mod vbin;
mod zmq;

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
        Platform::Simulation => Ok(Box::new(sim::SimDevice::open(vbin)?)),
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
    fn simulation_dispatches_without_loading_libvrtd_and_emulation_is_rejected() {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = temp.path().join("design.vbin");
        vbin(&path, Platform::Simulation);
        let Err(error) = open(&path) else {
            panic!("simulation VBIN without vpp_sim unexpectedly opened");
        };
        assert!(error.to_string().contains("vpp_sim"), "{error}");
        assert!(!error.to_string().contains("libvrtd"), "{error}");

        vbin(&path, Platform::Emulation);
        let Err(error) = open(&path) else {
            panic!("emulation VBIN unexpectedly opened");
        };
        assert!(error.to_string().contains("unsupported"), "{error}");
    }
}
