//! Static Verilog support assets emitted alongside generated RTL.

/// Every `.v` file under `assets/verilog/`, embedded at compile time.
/// Adding an asset is a filesystem-only operation.
#[derive(rust_embed::RustEmbed)]
#[folder = "assets/verilog/"]
pub struct VerilogAssets;

#[cfg(test)]
mod tests {
    use super::VerilogAssets;

    #[test]
    fn handshake_head_pipelines_ready_valid_and_data() {
        let asset = VerilogAssets::get("tapa_hs_pipeline.v").expect("pipeline asset");
        let source = std::str::from_utf8(&asset.data).expect("Verilog is UTF-8");

        // The floorplan pass pipelines all three handshake paths in Head.
        // These assertions prevent a combinational valid/data shortcut from
        // reintroducing the source-to-Tail long path.
        assert!(source.contains("if_read_reg <= if_read;"));
        assert!(source.contains("if_write_reg <= if_write;"));
        assert!(source.contains("if_din_reg <= if_din;"));
        assert!(source.contains("GRACE_PERIOD = BODY_LEVEL * 2 + 2"));

        // Resetting every wide Head/Body register creates a long, high-fanout
        // control net. The resettable Tail drains these fixed-latency stages
        // while reset remains asserted.
        let head = source
            .split("module tapa_hs_pipeline_head #(")
            .nth(1)
            .and_then(|text| text.split("endmodule").next())
            .expect("Head module");
        let body = source
            .split("module tapa_hs_pipeline_body #(")
            .nth(1)
            .and_then(|text| text.split("endmodule").next())
            .expect("Body module");
        assert!(!head.contains("if (reset)"));
        assert!(!body.contains("if (reset)"));
    }
}
