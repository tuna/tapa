use std::collections::HashMap;

pub fn parse_register_map(verilog: &str) -> HashMap<String, u32> {
    let mut map = HashMap::new();

    // Vitis HLS 2022.1+: `localparam ADDR_A_DATA_0 = 6'h10`
    // Non-greedy capture ensures the `_data_0` suffix is consumed, not part
    // of the argument name.
    let re =
        regex_lite::Regex::new(r"(?i)localparam\s+addr_(\w+?)_data_0\s*=\s*[\d']*h([0-9a-fA-F]+)")
            .expect("regex");
    for cap in re.captures_iter(verilog) {
        let name = cap[1].to_lowercase();
        let offset = u32::from_str_radix(&cap[2], 16).unwrap_or(0);
        map.entry(name).or_insert(offset);
    }

    map
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_data_localparam_format() {
        let v = "localparam ADDR_A_DATA_0 = 6'h10;\nlocalparam ADDR_N_DATA_0 = 6'h20;\n";
        let m = parse_register_map(v);
        assert_eq!(m.get("a").copied(), Some(0x10));
        assert_eq!(m.get("n").copied(), Some(0x20));
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
