//! Static Verilog support assets emitted alongside generated RTL.

/// Every `.v` file under `assets/verilog/`, embedded at compile time.
/// Adding an asset is a filesystem-only operation.
#[derive(rust_embed::RustEmbed)]
#[folder = "assets/verilog/"]
pub struct VerilogAssets;
