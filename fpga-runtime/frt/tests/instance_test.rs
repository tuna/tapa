#[test]
#[ignore = "requires verilator in PATH and compatible fixture package"]
fn open_cosim_verilator() {
    use frt::{Instance, Simulator};
    let path = std::path::Path::new("../frt-cosim/tests/fixtures/kernel.xo");
    if !path.exists() {
        return;
    }
    let _instance = Instance::open_cosim(path, Simulator::Verilator).expect("open cosim instance");
}
