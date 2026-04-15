//! Conformance and round-trip tests for `tapa-graphir`.

use tapa_graphir::module::definition::AnyModuleDefinition as ModDef;
use tapa_graphir::interface::AnyInterface as Iface;
use tapa_graphir::project::Project;

fn fixture(name: &str) -> String {
    let path = format!("{}/../testdata/graphir/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path}: {e}"))
}

// -- vadd_project fixture tests --

#[test]
fn parse_vadd_project() {
    let p = Project::from_json(&fixture("vadd_project.json")).expect("parse");
    assert_eq!(p.part_num.as_deref(), Some("xcu280-fsvh2892-2L-e"), "part_num");
    assert_eq!(p.modules.top_name.as_deref(), Some("VecAdd"), "top_name");
    assert_eq!(p.modules.module_definitions.len(), 3, "module count");
}

#[test]
fn grouped_module_structure() {
    let p = Project::from_json(&fixture("vadd_project.json")).expect("parse");
    let vecadd = p.modules.module_definitions.iter()
        .find(|m| m.name() == "VecAdd").expect("VecAdd");
    match vecadd {
        ModDef::Grouped { base, grouped, .. } => {
            assert_eq!(base.ports.len(), 3, "port count");
            assert_eq!(grouped.submodules.len(), 1, "submodule count");
            assert_eq!(grouped.wires.len(), 1, "wire count");
            let area = grouped.submodules[0].area.as_ref().expect("area");
            assert_eq!(area.ff, 150, "ff");
        }
        _ => panic!("VecAdd should be Grouped"),
    }
}

#[test]
fn verilog_module_structure() {
    let p = Project::from_json(&fixture("vadd_project.json")).expect("parse");
    let add = p.modules.module_definitions.iter()
        .find(|m| m.name() == "Add").expect("Add");
    match add {
        ModDef::Verilog { verilog, .. } => {
            assert!(verilog.verilog.contains("module Add"), "has Verilog source");
        }
        _ => panic!("Add should be Verilog"),
    }
}

#[test]
fn stub_module_structure() {
    let p = Project::from_json(&fixture("vadd_project.json")).expect("parse");
    let fifo = p.modules.module_definitions.iter()
        .find(|m| m.name() == "fifo_w32_d2").expect("fifo stub");
    match fifo {
        ModDef::Stub { base, .. } => {
            assert!(base.hierarchical_name.is_none(), "stub has null hierarchical_name");
        }
        _ => panic!("fifo should be Stub"),
    }
}

#[test]
fn expression_tokens_parse() {
    let p = Project::from_json(&fixture("vadd_project.json")).expect("parse");
    let vecadd = p.modules.module_definitions.iter()
        .find(|m| m.name() == "VecAdd").expect("VecAdd");
    if let ModDef::Grouped { base, .. } = vecadd {
        let port = base.ports.iter().find(|p| p.name == "a_offset").expect("a_offset");
        let range = port.range.as_ref().expect("has range");
        assert_eq!(range.left[0].repr, "63", "left value");
        assert_eq!(range.right[0].repr, "0", "right value");
    }
}

#[test]
fn interfaces_parse() {
    let p = Project::from_json(&fixture("vadd_project.json")).expect("parse");
    let ifaces = p.ifaces.as_ref().expect("has ifaces");
    assert_eq!(ifaces["VecAdd"].len(), 4, "VecAdd interface count");
}

#[test]
fn ap_ctrl_interface_typed_fields() {
    let p = Project::from_json(&fixture("vadd_project.json")).expect("parse");
    let ifaces = p.ifaces.as_ref().expect("ifaces");
    let ctrl = &ifaces["VecAdd"][0];
    match ctrl {
        Iface::ApCtrl { ap_start_port, ap_ready_port, ap_done_port, .. } => {
            assert_eq!(ap_start_port.as_deref(), Some("ap_start"), "ap_start_port");
            assert_eq!(ap_ready_port.as_deref(), Some("ap_ready"), "ap_ready_port");
            assert_eq!(ap_done_port.as_deref(), Some("ap_done"), "ap_done_port");
        }
        _ => panic!("should be ApCtrl"),
    }
}

#[test]
fn handshake_interface_typed_fields() {
    let p = Project::from_json(&fixture("vadd_project.json")).expect("parse");
    let ifaces = p.ifaces.as_ref().expect("ifaces");
    let hs = &ifaces["VecAdd"][1];
    match hs {
        Iface::HandShake { valid_port, data_ports, .. } => {
            assert_eq!(valid_port.as_deref(), Some("a_offset_ap_vld"), "valid_port");
            assert_eq!(data_ports, &["a_offset"], "data_ports");
        }
        _ => panic!("should be HandShake"),
    }
}

// -- all_variants fixture: all 8 module + 11 interface discriminators --

#[test]
fn all_8_module_discriminators_parse() {
    let p = Project::from_json(&fixture("all_variants.json")).expect("parse");
    assert_eq!(p.modules.module_definitions.len(), 8, "must have 8 module defs");

    let types: Vec<&str> = p.modules.module_definitions.iter().map(|m| match m {
        ModDef::Grouped { .. } => "grouped_module",
        ModDef::Verilog { .. } => "verilog_module",
        ModDef::Aux { .. } => "aux_module",
        ModDef::AuxSplit { .. } => "aux_split_module",
        ModDef::Stub { .. } => "stub_module",
        ModDef::PassThrough { .. } => "pass_through_module",
        ModDef::InternalVerilog { .. } => "internal_verilog_module",
        ModDef::InternalGrouped { .. } => "internal_grouped_module",
    }).collect();

    for expected in [
        "grouped_module", "verilog_module", "aux_module", "aux_split_module",
        "stub_module", "pass_through_module", "internal_verilog_module", "internal_grouped_module",
    ] {
        assert!(types.contains(&expected), "missing module type: {expected}");
    }
}

#[test]
fn all_11_interface_discriminators_parse() {
    let p = Project::from_json(&fixture("all_variants.json")).expect("parse");
    let ifaces = p.ifaces.as_ref().expect("has ifaces");
    let top_ifaces = &ifaces["Top"];
    assert_eq!(top_ifaces.len(), 11, "must have 11 interfaces");

    let types: Vec<&str> = top_ifaces.iter().map(|i| match i {
        Iface::HandShake { .. } => "handshake",
        Iface::FeedForward { .. } => "feed_forward",
        Iface::FalsePath { .. } => "false_path",
        Iface::Clock { .. } => "clock",
        Iface::FalsePathReset { .. } => "fp_reset",
        Iface::FeedForwardReset { .. } => "ff_reset",
        Iface::ApCtrl { .. } => "ap_ctrl",
        Iface::NonPipeline { .. } => "non_pipeline",
        Iface::Unknown { .. } => "unknown",
        Iface::TapaPeek { .. } => "tapa_peek",
        Iface::Aux { .. } => "aux",
    }).collect();

    for expected in [
        "handshake", "feed_forward", "false_path", "clock", "fp_reset",
        "ff_reset", "ap_ctrl", "non_pipeline", "unknown", "tapa_peek", "aux",
    ] {
        assert!(types.contains(&expected), "missing interface type: {expected}");
    }
}

#[test]
fn all_variants_round_trip() {
    let json = fixture("all_variants.json");
    let p1 = Project::from_json(&json).expect("parse 1");
    let serialized = p1.to_json().expect("serialize");
    let p2 = Project::from_json(&serialized).expect("parse 2");
    assert_eq!(p1.modules.module_definitions.len(), p2.modules.module_definitions.len(), "modules");
    let n1 = p1.ifaces.as_ref().unwrap()["Top"].len();
    let n2 = p2.ifaces.as_ref().unwrap()["Top"].len();
    assert_eq!(n1, n2, "interfaces round-trip");
}

// -- Round-trip tests --

#[test]
fn vadd_project_round_trip() {
    let json = fixture("vadd_project.json");
    let p1 = Project::from_json(&json).expect("parse 1");
    let serialized = p1.to_json().expect("serialize");
    let p2 = Project::from_json(&serialized).expect("parse 2");
    assert_eq!(p1.modules.module_definitions.len(), p2.modules.module_definitions.len(), "modules");
    assert_eq!(p1.part_num, p2.part_num, "part_num");
    assert_eq!(p1.modules.top_name, p2.modules.top_name, "top_name");
}

#[test]
fn normalized_output_is_sorted() {
    let json = fixture("vadd_project.json");
    let p = Project::from_json(&json).expect("parse");
    let serialized = p.to_json().expect("serialize");
    let p2 = Project::from_json(&serialized).expect("re-parse");
    let names: Vec<_> = p2.modules.module_definitions.iter().map(|m| m.name().to_string()).collect();
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(names, sorted, "sorted by name");
}

// -- Negative tests --

#[test]
fn invalid_module_type_rejected() {
    let json = r#"{"modules": {"name": "$root", "module_definitions": [
        {"name": "X", "module_type": "nonexistent_module", "parameters": [], "ports": []}
    ]}, "blackboxes": []}"#;
    let err = Project::from_json(json).unwrap_err();
    assert!(!err.to_string().is_empty(), "has error message");
}

#[test]
fn invalid_interface_type_rejected() {
    let json = r#"{"modules": {"name": "$root", "module_definitions": []}, "blackboxes": [],
        "ifaces": {"M": [{"type": "bad", "ports": [], "role": "sink", "origin_info": ""}]}}"#;
    let err = Project::from_json(json).unwrap_err();
    assert!(!err.to_string().is_empty(), "has error message");
}

#[test]
fn invalid_token_type_rejected() {
    let json = r#"{"modules": {"name": "$root", "module_definitions": [
        {"name": "X", "module_type": "verilog_module", "parameters": [],
         "ports": [{"name": "p", "type": "input wire", "range": {
             "left": [{"type": "unknown", "repr": "1"}], "right": [{"type": "lit", "repr": "0"}]
         }}], "verilog": "", "submodules_module_names": []}
    ]}, "blackboxes": []}"#;
    let err = Project::from_json(json).unwrap_err();
    assert!(!err.to_string().is_empty(), "has error message");
}

// -- BlackBox tests --

#[test]
fn blackbox_round_trip() {
    let original = b"Hello, TAPA blackbox content!";
    let bb = tapa_graphir::blackbox::BlackBox::from_binary("test.v".to_owned(), original);
    let decoded = bb.get_binary().expect("decode");
    assert_eq!(decoded, original, "BlackBox round-trip");
}

#[test]
fn blackbox_malformed_base64_rejected_by_from_json() {
    let json = r#"{"modules": {"name": "$root", "module_definitions": []},
        "blackboxes": [{"path": "bad.v", "base64": "!!!not-valid-base64!!!"}]}"#;
    let err = Project::from_json(json).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("blackboxes[0]"), "error has field path: {msg}");
}

#[test]
fn blackbox_malformed_zlib_rejected_by_from_json() {
    use base64::Engine;
    let bad = base64::engine::general_purpose::STANDARD.encode(b"not-zlib");
    let json = format!(
        r#"{{"modules": {{"name": "$root", "module_definitions": []}},
            "blackboxes": [{{"path": "bad.v", "base64": "{bad}"}}]}}"#
    );
    let err = Project::from_json(&json).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("blackboxes[0]"), "error has field path: {msg}");
}

// -- Unknown-field preservation round-trip --

#[test]
fn unknown_fields_preserved_through_round_trip() {
    // Inject unknown fields at project, module, and interface levels.
    let json = r#"{
        "modules": {
            "name": "$root",
            "module_definitions": [
                {
                    "name": "M",
                    "module_type": "stub_module",
                    "parameters": [],
                    "ports": [
                        {"name": "p", "type": "input wire", "range": null,
                         "future_port_field": "port_extra_value"}
                    ],
                    "metadata": null,
                    "future_module_field": 42
                }
            ],
            "top_name": null
        },
        "blackboxes": [],
        "ifaces": {
            "M": [
                {"type": "clock", "clk_port": "clk", "rst_port": null,
                 "ports": ["clk"], "role": "sink", "origin_info": "test",
                 "future_iface_field": [1, 2, 3]}
            ]
        },
        "future_project_field": "hello"
    }"#;

    let p = Project::from_json(json).expect("parse with unknown fields");

    // Round-trip through serialize/parse.
    let serialized = p.to_json().expect("serialize");
    let reparsed: serde_json::Value =
        serde_json::from_str(&serialized).expect("re-parse as Value");

    // Project-level unknown field preserved.
    assert_eq!(
        reparsed["future_project_field"], "hello",
        "project-level unknown field preserved"
    );

    // Module-level unknown field preserved.
    let module = &reparsed["modules"]["module_definitions"][0];
    assert_eq!(
        module["future_module_field"], 42,
        "module-level unknown field preserved"
    );

    // Port-level unknown field preserved.
    let port = &module["ports"][0];
    assert_eq!(
        port["future_port_field"], "port_extra_value",
        "port-level unknown field preserved"
    );

    // Interface-level unknown field preserved.
    let iface = &reparsed["ifaces"]["M"][0];
    assert_eq!(
        iface["future_iface_field"],
        serde_json::json!([1, 2, 3]),
        "interface-level unknown field preserved"
    );
}
