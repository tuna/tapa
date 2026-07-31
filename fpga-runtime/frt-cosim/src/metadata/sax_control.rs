use std::collections::HashMap;

pub fn parse_register_map(verilog: &str) -> HashMap<String, u32> {
    let mut map = HashMap::new();

    // Vitis HLS 2020.x: `localparam ADDR_A_0 = 6'h10`
    // Vitis HLS 2021+:  `localparam ADDR_A_DATA_0 = 6'h10`
    // Non-greedy capture ensures `_data_0` suffix is consumed, not part of name.
    let re = regex_lite::Regex::new(
        r"(?i)localparam\s+addr_(\w+?)_(?:data_)?0\s*=\s*[\d']*h([0-9a-fA-F]+)",
    )
    .expect("regex");
    for cap in re.captures_iter(verilog) {
        let name = cap[1].to_lowercase();
        let offset = u32::from_str_radix(&cap[2], 16).unwrap_or(0);
        map.entry(name).or_insert(offset);
    }

    // Fallback: parse comments emitted by all known Vitis HLS versions:
    //   `// 0x10 : Data signal of a`
    if map.is_empty() {
        let comment_re = regex_lite::Regex::new(r"0x([0-9a-fA-F]+)\s*:\s*Data signal of\s+(\w+)")
            .expect("comment regex");
        for cap in comment_re.captures_iter(verilog) {
            let offset = u32::from_str_radix(&cap[1], 16).unwrap_or(0);
            let name = cap[2].to_lowercase();
            // Keep first occurrence (low word of a 64-bit address pair).
            map.entry(name).or_insert(offset);
        }
    }

    map
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_legacy_localparam_format() {
        let v = "localparam ADDR_A_0 = 8'h10;\nlocalparam ADDR_N_0 = 8'h1c;\n";
        let m = parse_register_map(v);
        assert_eq!(m.get("a").copied(), Some(0x10));
        assert_eq!(m.get("n").copied(), Some(0x1c));
    }

    #[test]
    fn parses_data_localparam_format() {
        let v = "localparam ADDR_A_DATA_0 = 6'h10;\nlocalparam ADDR_N_DATA_0 = 6'h20;\n";
        let m = parse_register_map(v);
        assert_eq!(m.get("a").copied(), Some(0x10));
        assert_eq!(m.get("n").copied(), Some(0x20));
    }

    #[test]
    fn parses_comment_format_as_fallback() {
        let v = "// 0x10 : Data signal of a\n// 0x18 : Data signal of b\n";
        let m = parse_register_map(v);
        assert_eq!(m.get("a").copied(), Some(0x10));
        assert_eq!(m.get("b").copied(), Some(0x18));
    }

    #[test]
    fn keeps_low_word_offset_for_64bit_args() {
        // Both low and high words appear; only the low (first) offset should be kept.
        let v = "localparam ADDR_A_DATA_0 = 6'h10;\nlocalparam ADDR_A_DATA_1 = 6'h14;\n";
        let m = parse_register_map(v);
        assert_eq!(m.get("a").copied(), Some(0x10));
    }

    #[test]
    fn ignores_non_data_localparams() {
        let v = "localparam ADDR_AP_CTRL = 5'h00;\nlocalparam ADDR_GIE = 5'h04;\nlocalparam ADDR_A_DATA_0 = 5'h10;\n";
        let m = parse_register_map(v);
        assert!(!m.contains_key("ap_ctrl"));
        assert!(!m.contains_key("gie"));
        assert_eq!(m.get("a").copied(), Some(0x10));
    }
}
