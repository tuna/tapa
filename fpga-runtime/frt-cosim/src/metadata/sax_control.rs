use std::collections::HashMap;

pub fn parse_register_map(verilog: &str) -> HashMap<String, u32> {
    let mut map = HashMap::new();
    let re = regex_lite::Regex::new(r"(?i)localparam\s+addr_(\w+)_0\s*=\s*[\d']*h([0-9a-fA-F]+)")
        .expect("regex");
    for cap in re.captures_iter(verilog) {
        let name = cap[1].to_lowercase();
        let offset = u32::from_str_radix(&cap[2], 16).unwrap_or(0);
        map.insert(name, offset);
    }
    map
}
