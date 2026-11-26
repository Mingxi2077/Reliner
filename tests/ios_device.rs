use reliner::{ImportOptions, import_containerlab, preview_ios_device, run_lab};
use serde_json::{Value, json};

fn preview(config: &str) -> Value {
    serde_json::to_value(preview_ios_device(config).unwrap()).unwrap()
}

const SOURCE: &str = "name: ios\ntopology:\n  nodes:\n    a: {}\n    sw: {}\n    b: {}\n  links:\n    - endpoints: [a:p, sw:Gi0/1]\n    - endpoints: [sw:cable, b:p]\n";
const CONFIG: &str = "vlan 10\n name private-label\ninterface GigabitEthernet0/1\n switchport mode access\n switchport access vlan 10\ninterface Gi0/2\n switchport mode access\n switchport access vlan 10\n ip access-group private-label in\ninterface Gi0/3\n shutdown\ninterface Loopback1\n description private-label\nend";

fn options(config: &str, ports: Value) -> ImportOptions {
    let host =
        |ipv4, mac| json!({"kind":"host","interface":"p","ipv4":ipv4,"prefix_length":24,"mac":mac});
    serde_json::from_value(json!({
        "id":"ios", "revision":"1","link_delay_ns":"100",
        "models":{"a":host("192.0.2.1","02:00:00:00:00:01"),"b":host("192.0.2.2","02:00:00:00:00:02"),
            "sw":{"kind":"switch","ios_device_config":config,"ports":ports}},
        "scenario":{"end_ns":"2000","arp_lifetime_ns":"5000","mac_lifetime_ns":"5000",
            "probes":[{"id":"ping","source":"a","destination":"b","at_ns":"0","deadline_ns":"2000"}]}
    })).unwrap()
}

#[test]
fn scope_lines_and_opaque_content_survive_preview() {
    let config = "\u{feff}!\r\n\r\nbanner motd ^Cprivate-label\r\ninterface Gi0/99\r\n!\r\n^C\r\nmacro name private-label\r\nend\r\ninterface Gi0/88\r\n@\r\nvlan database\r\nvlan 10 name private-label\r\n exit\r\nvlan 20-21\r\n name private-label\r\ninterface Gi0/1\r\n switchport mode access\r\n switchport access vlan 10\r\n shutdown\r\n channel-group 1 mode active\r\ninterface range Gi0/3 - 9\r\n switchport access vlan 99\r\nusername private-label secret private-label\r\nend";
    let doc = preview(config);
    assert_eq!(doc["interfaces"].as_array().unwrap().len(), 1);
    let port = &doc["interfaces"][0];
    assert_eq!(port["interface"], "Gi0/1");
    assert_eq!(port["line"], 16);
    assert_eq!(port["field_lines"], json!({"kind":17,"vlan":18}));
    assert_eq!(
        port["context"]["shutdown"],
        json!([{"line":19,"value":true}])
    );
    assert_eq!(doc["global"]["vlan_database_lines"], json!([11]));
    assert_eq!(
        doc["global"]["vlan_definitions"],
        json!([{"line":12,"vlans":[10]},{"line":14,"vlans":[20,21]}])
    );
    assert!(
        doc["gaps"]
            .as_array()
            .unwrap()
            .iter()
            .any(|g| g["field"] == "unsupported_interface" && g["line"] == 21)
    );
    assert!(!doc.to_string().contains("private-label"));
}

#[test]
fn global_tagging_applies_across_the_file() {
    let text = "interface Te1/0/1\n switchport mode trunk\n switchport trunk allowed vlan 10,20\n switchport trunk native vlan 10\n";
    for (setting, expected) in [
        ("vlan dot1q tag native", json!(true)),
        ("no vlan dot1q tag native", json!(false)),
        (
            "vlan dot1q tag native\nno vlan dot1q tag native",
            Value::Null,
        ),
    ] {
        let doc = preview(&format!("{text}{setting}"));
        assert_eq!(doc["global"]["native_tagging"], expected);
        let port = &doc["interfaces"][0];
        if expected == false {
            assert_eq!(port["mode"]["native_vlan"], 10);
        } else {
            assert!(port["mode"].get("native_vlan").is_none());
            assert_eq!(
                port["context"]["configured_native_vlan"],
                json!({"value":10,"line":4})
            );
        }
    }
}

#[test]
fn ambiguous_or_broken_config_reports_original_lines_without_payloads() {
    for (source, line) in [
        ("interface Gi00/01\n!\ninterface GigabitEthernet0/1", 3),
        ("!\nbanner motd #private-label\ninterface Gi0/1", 2),
        ("macro name private-label\ninterface Gi0/1", 1),
        (
            "\n!\ninterface Gi0/1\n switchport access vlan private-label",
            4,
        ),
    ] {
        let err = preview_ios_device(source).unwrap_err().to_string();
        assert!(
            err.contains(&format!("/ios_device_config/lines/{line}")),
            "{err}"
        );
        assert!(!err.contains("private-label"));
    }
}

#[test]
fn bind_source_and_named_ports_run_and_archive_conditional_result() {
    let opts = options(
        CONFIG,
        json!({"Gi0/1":{},"cable":{"ios_interface":"gi0/2"},"spare":{"mode":{"kind":"access","vlan":20}}}),
    );
    let native = import_containerlab(SOURCE, &opts).unwrap();
    assert!(!native.contains("private-label"));
    let doc: Value = serde_json::from_str(&native).unwrap();
    let source = &doc["import_source"];
    let root = "/models/sw/ios_device_config";
    assert_eq!(
        source["metadata"][root]["bindings"],
        json!({"gigabitethernet0/1":"Gi0/1","gigabitethernet0/2":"cable"})
    );
    let gaps = source["gaps"].as_array().unwrap();
    for (path, scope) in [
        (
            format!("{root}/lines/9"),
            json!({"kind":"port","port_id":"sw:cable"}),
        ),
        (
            format!("{root}/lines/10"),
            json!({"kind":"device","device_id":"sw"}),
        ),
        (
            "/models/sw/ports/spare/unbound_port".into(),
            json!({"kind":"port","port_id":"sw:spare"}),
        ),
    ] {
        assert!(
            gaps.iter()
                .any(|g| g["source_path"] == path && g["scope"] == scope)
        );
    }
    let report = run_lab(&native).unwrap();
    let mut archive = Vec::new();
    reliner::write_archive(&report, None, None, &mut archive).unwrap();
    let saved =
        serde_json::to_value(reliner::read_archive(archive.as_slice()).unwrap().report).unwrap();
    assert_eq!(saved["import_source"], *source);
    assert_eq!(saved["probes"][0]["outcome"]["status"], "undetermined");
    assert_eq!(
        saved["probes"][0]["outcome"]["modeled"]["status"],
        "reachable"
    );
}

#[test]
fn rejects_binding_collisions_inline_sources_and_mode_conflicts() {
    for (ports, path) in [
        (
            json!({"Gi0/1":{},"cable":{"ios_interface":"GigabitEthernet0/01"}}),
            "/models/sw/ports/cable",
        ),
        (
            json!({"Gi0/1":{},"cable":{"ios_interface":"Gi0/99"}}),
            "/models/sw/ports/cable/ios_interface",
        ),
        (
            json!({"Gi0/1":{},"cable":{"name":"Gi0/2"}}),
            "/models/sw/ports/cable",
        ),
        (
            json!({"Gi0/1":{},"cable":{"ios_config":"private-label"}}),
            "/models/sw/ports/cable/ios_config",
        ),
        (
            json!({"Gi0/1":{"mode":{"kind":"access","vlan":99}},"cable":{"ios_interface":"Gi0/2"}}),
            "/models/sw/ios_device_config/lines/5",
        ),
    ] {
        let err =
            serde_json::to_value(import_containerlab(SOURCE, &options(CONFIG, ports)).unwrap_err())
                .unwrap();
        assert_eq!(err["kind"], "mapping");
        assert_eq!(err["path"], path);
        assert!(!err.to_string().contains("private-label"));
    }
}

#[test]
fn native_tagging_changes_the_mode_used_by_the_simulator() {
    let trunk = CONFIG.replace(
        "interface Gi0/2\n switchport mode access\n switchport access vlan 10",
        "interface Gi0/2\n switchport mode trunk\n switchport trunk allowed vlan 10\n switchport trunk native vlan 10",
    );
    for (tagging, reachable) in [
        ("no vlan dot1q tag native", true),
        ("vlan dot1q tag native", false),
    ] {
        let config = format!("{tagging}\n{trunk}");
        let opts = options(
            &config,
            json!({"Gi0/1":{},"cable":{"ios_interface":"Gi0/2"}}),
        );
        let native = import_containerlab(SOURCE, &opts).unwrap();
        let report = serde_json::to_value(run_lab(&native).unwrap()).unwrap();
        let outcome = &report["probes"][0]["outcome"];
        assert_eq!(outcome["status"], "undetermined");
        assert_eq!(outcome["modeled"]["status"] == "reachable", reachable);
    }
}

#[test]
fn single_line_and_multiline_text_do_not_create_stanzas() {
    let config = "banner login #private-label#\nmacro auto execute private-label {\ninterface Gi0/7\n}\nbanner motd \u{3}\ninterface Gi0/8\nend\n\u{3}\ninterface Gi0/1\n switchport mode access\n switchport access vlan 10";
    let doc = preview(config);
    assert_eq!(doc["interfaces"].as_array().unwrap().len(), 1);
    assert_eq!(doc["interfaces"][0]["line"], 9);
    assert_eq!(doc["interfaces"][0]["field_lines"]["vlan"], 11);
    assert!(!doc.to_string().contains("private-label"));
}

#[test]
fn vlan_database_state_is_evidence_and_does_not_rewrite_port_vlans() {
    let config = "vlan database\n vlan 10 state suspend\n exit\nvlan 20\n shutdown\n name private-label\ninterface Gi0/1\n switchport mode access\n switchport access vlan 10";
    let doc = preview(config);
    let vlans = &doc["global"]["vlan_definitions"];
    assert_eq!(
        vlans[0]["context"]["state"],
        json!([{"line":2,"value":"suspend"}])
    );
    assert_eq!(
        vlans[1]["context"]["shutdown"],
        json!([{"line":5,"value":true}])
    );
    assert_eq!(doc["interfaces"][0]["mode"]["vlan"], 10);
    assert!(!doc.to_string().contains("private-label"));
}

fn full_trunks(count: usize) -> String {
    (1..=count).map(|n| format!("interface Gi0/{n}\n switchport mode trunk\n switchport trunk native vlan 1\n switchport trunk allowed vlan all\n")).collect()
}

fn assert_source_limit(config: &str, line: usize, limit: &str) {
    let Err(reliner::ImportError::Source { path, message }) = preview_ios_device(config) else {
        panic!("expected a whole-device source limit at line {line}");
    };
    assert_eq!(path, format!("/ios_device_config/lines/{line}"));
    assert!(message.contains(limit), "{message}");
}

#[test]
fn vlan_entry_budget_is_shared_across_database_and_ports() {
    let repeated = "vlan 1-4094\n".repeat(65);
    assert_source_limit(&repeated, 65, "262144");
    assert_source_limit(&full_trunks(65), 260, "262144");
    let exact = format!("{}vlan 1-128\n", full_trunks(64));
    assert!(preview_ios_device(&exact).is_ok());
    assert_source_limit(&(exact + "vlan 1\n"), 258, "262144");
    let same_port = format!(
        "interface Gi0/1\n{}",
        " switchport trunk allowed vlan all\n".repeat(65)
    );
    assert_source_limit(&same_port, 66, "262144");
}

#[test]
fn input_and_gap_limits_report_original_lines_even_in_opaque_text() {
    let mut large = String::from("\u{feff}!\r\nbanner motd #\r\n");
    large.push_str(&"x".repeat(1_048_576 - large.len()));
    large.push('\n');
    assert_source_limit(&large, 3, "1048576");
    let many_lines = format!("macro name foo\n{}@\n", "!\n".repeat(16_383));
    assert_source_limit(&many_lines, 16_385, "16384");
    let unknowns = "unknown command\n".repeat(4097);
    assert_source_limit(&unknowns, 4097, "4096");
}

#[test]
fn forty_eight_fully_populated_trunks_import_without_list_loss() {
    let config = full_trunks(48);
    let mut ports = serde_json::Map::new();
    for n in 1..=48 {
        let key = if n == 2 {
            "cable".into()
        } else {
            format!("Gi0/{n}")
        };
        ports.insert(
            key,
            json!({"ios_interface":format!("GigabitEthernet0/{n}")}),
        );
    }
    let native = import_containerlab(SOURCE, &options(&config, Value::Object(ports))).unwrap();
    let doc: Value = serde_json::from_str(&native).unwrap();
    let metadata = &doc["import_source"]["metadata"]["/models/sw/ios_device_config"]["interfaces"];
    for port in metadata.as_array().unwrap() {
        assert_eq!(port["context"]["allowed_vlan_list"], "1-4094");
    }
    assert!(native.len() <= reliner::WorkspaceConfig::default().bytes_per_document);
    reliner::Workspace::new(reliner::WorkspaceConfig::default())
        .unwrap()
        .open_document(native)
        .unwrap();
    let sw = doc["devices"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["id"] == "sw")
        .unwrap();
    let ports = sw["ports"].as_array().unwrap();
    assert_eq!(ports.len(), 48);
    for port in ports {
        let list = port["mode"]["allowed_vlans"].as_array().unwrap();
        assert_eq!(list.len(), 4094);
        assert!(
            list.iter()
                .enumerate()
                .all(|(i, v)| v.as_u64() == Some(i as u64 + 1))
        );
    }
}

#[test]
fn same_line_port_and_device_gaps_keep_distinct_ids_and_the_original_location() {
    let config = format!("vlan dot1q tag native\n{}", full_trunks(2));
    let opts = options(
        &config,
        json!({"Gi0/1":{},"cable":{"ios_interface":"Gi0/2"}}),
    );
    let native = import_containerlab(SOURCE, &opts).unwrap();
    let doc: Value = serde_json::from_str(&native).unwrap();
    let gaps = doc["import_source"]["gaps"].as_array().unwrap();
    let at_line: Vec<_> = gaps
        .iter()
        .filter(|g| g["source_path"] == "/models/sw/ios_device_config/lines/1")
        .collect();
    assert_eq!(at_line.len(), 3);
    let ids: std::collections::BTreeSet<_> =
        at_line.iter().map(|g| g["id"].as_str().unwrap()).collect();
    assert_eq!(ids.len(), 3);
    assert!(at_line.iter().all(|g| g["value"]["line"] == 1));
    run_lab(&native).unwrap();
}

#[test]
fn nested_macro_and_banner_preserve_spans_without_body_or_phantom_ports() {
    let config = "macro auto execute private-label { echo \" } \" # }\n nested {\n }\ninterface Gi0/19\n private-label\n}\nbanner motd ^C\ninterface Gi0/20\nprivate-label^C\nunknown private-label\ninterface Gi0/1\n switchport mode access\n switchport access vlan 1";
    let doc = preview(config);
    assert_eq!(doc["interfaces"].as_array().unwrap().len(), 1);
    assert_eq!(
        doc["global"]["opaque_sections"],
        json!([{"field":"macro","line":1,"end_line":6},{"field":"banner","line":7,"end_line":9}])
    );
    assert!(
        doc["gaps"]
            .as_array()
            .unwrap()
            .iter()
            .any(|g| g["line"] == 10 && g["field"] == "global_command")
    );
    assert!(!doc.to_string().contains("private-label"));
}

#[test]
fn gap_quota_covers_ports_and_mapping_only_diagnostics() {
    let config = format!(
        "interface Gi0/1\n switchport mode access\n switchport access vlan 1\n{}",
        " unknown private-label\n".repeat(4095)
    );
    assert!(preview_ios_device(&config).is_ok());
    let opts = options(
        &config,
        json!({"Gi0/1":{},"cable":{"mode":{"kind":"access","vlan":1}}}),
    );
    let Err(reliner::ImportError::Source { path, message }) = import_containerlab(SOURCE, &opts)
    else {
        panic!("expected a mapping-only gap to exhaust the budget")
    };
    assert_eq!(path, "/models/sw/ports/cable");
    assert!(message.contains("4096"));
    assert_source_limit(
        &("interface Gi0/1\n".to_owned() + &" unknown private-label\n".repeat(4097)),
        4098,
        "4096",
    );
}

#[test]
fn legacy_interface_preview_still_expands_all_none_and_overlapping_ranges() {
    for (list, expected) in [
        ("all", (1..=4094).collect::<Vec<u16>>()),
        ("none", Vec::new()),
        ("4093-4094,2-5,3-4,1,4094", vec![1, 2, 3, 4, 5, 4093, 4094]),
    ] {
        let cfg = format!(
            "interface Gi0/1\n switchport mode trunk\n switchport trunk allowed vlan {list}"
        );
        let port = reliner::preview_ios(&cfg).unwrap();
        assert_eq!(port.mode["allowed_vlans"], json!(expected));
        assert_eq!(port.field_lines["allowed_vlans"], 3);
    }
}

fn import_two_switches(first: &str, second: &str) -> Result<String, reliner::ImportError> {
    let mut opts = options(first, json!({"Gi0/1":{},"cable":{"ios_interface":"Gi0/2"}}));
    opts.models.insert("sx".into(),json!({"kind":"switch","ports":{"Gi0/1":{"mode":{"kind":"access","vlan":1}}},"ios_device_config":second}));
    let source = SOURCE.replace("    sw: {}", "    sw: {}\n    sx: {}");
    import_containerlab(&source, &opts)
}

#[test]
fn whole_device_vlan_entries_share_one_import_budget() {
    let first = full_trunks(48);
    let at_limit = format!("{}vlan 1-128\n", "vlan 1-4094\n".repeat(16));
    let over = format!("{at_limit}vlan 1\n");
    assert!(preview_ios_device(&over).is_ok());
    let Err(reliner::ImportError::Source { path, message }) = import_two_switches(&first, &over)
    else {
        panic!("expected cumulative VLAN limit")
    };
    assert_eq!(path, "/models/sx/ios_device_config/lines/18");
    assert!(message.contains("262144"));
    assert!(import_two_switches(&first, &at_limit).is_ok());
}

#[test]
fn whole_device_gaps_share_one_import_budget() {
    let access = "interface Gi0/1\n switchport mode access\n switchport access vlan 1\n";
    let first = format!(
        "{access}{}{}",
        access.replace("Gi0/1", "Gi0/2"),
        "unknown\n".repeat(2047)
    );
    let at_limit = format!("{access}{}", "unknown\n".repeat(2047));
    let over = format!("{at_limit}unknown\nunknown\n");
    assert!(preview_ios_device(&over).is_ok());
    let Err(reliner::ImportError::Source { path, message }) = import_two_switches(&first, &over)
    else {
        panic!("expected cumulative gap limit")
    };
    assert_eq!(path, "/models/sx/ios_device_config/lines/2052");
    assert!(message.contains("4096"));
    assert!(import_two_switches(&first, &at_limit).is_ok());
}

#[test]
fn raw_byte_and_line_limits_apply_independently_to_each_source() {
    let padding = format!(
        "banner motd #\n{}#\n",
        format!("{}\n", "x".repeat(70)).repeat(8300)
    );
    let first = format!("{padding}{}", full_trunks(2));
    let second =
        format!("{padding}interface Gi0/1\n switchport mode access\n switchport access vlan 1\n");
    assert!(first.len() + second.len() > 1_048_576);
    assert!(first.lines().count() + second.lines().count() > 16_384);
    assert!(import_two_switches(&first, &second).is_ok());
}
