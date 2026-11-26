use reliner::{
    ImportError, ImportOptions, import_containerlab, preview_containerlab, run_lab, validate_lab,
};
use serde_json::{Value, json};

const SOURCE: &str = include_str!("../examples/import-containerlab.yml");

fn options() -> ImportOptions {
    serde_json::from_str(include_str!("../examples/import-models.json")).unwrap()
}

fn source() -> Value {
    let mut yaml: serde_yaml_ng::Value = serde_yaml_ng::from_str(SOURCE).unwrap();
    yaml.apply_merge().unwrap();
    serde_json::to_value(yaml).unwrap()
}

fn imported(input: &str, options: &ImportOptions) -> Value {
    serde_json::from_str(&import_containerlab(input, options).unwrap()).unwrap()
}

fn device<'a>(doc: &'a Value, id: &str) -> &'a Value {
    doc["devices"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["id"] == id)
        .unwrap()
}

fn mapping(input: &str, options: &ImportOptions, expected: &str) {
    match import_containerlab(input, options).unwrap_err() {
        ImportError::Mapping { path, message } => {
            assert_eq!(path, expected);
            assert!(!message.is_empty());
        }
        other => panic!("expected Mapping at {expected}, got {other:?}"),
    }
}

#[test]
fn preview_lists_structure_and_explicit_configuration_without_inventing_a_model() {
    let preview = preview_containerlab(SOURCE).unwrap();
    assert_eq!(preview.source.source_name, "import-static-ipv4");
    assert_eq!(preview.nodes.len(), 4);
    assert_eq!(preview.links.len(), 3);
    assert_eq!(
        preview.nodes.iter().map(|n| n.ports.len()).sum::<usize>(),
        6
    );
    let paths: Vec<_> = preview
        .missing
        .iter()
        .map(|item| item.path.as_str())
        .collect();
    for path in [
        "/id",
        "/revision",
        "/link_delay_ns",
        "/scenario",
        "/models/pc-a",
        "/models/r1",
        "/models/r2",
        "/models/pc-b",
    ] {
        assert!(paths.contains(&path), "missing UI requirement {path}");
    }
    assert_eq!(preview.source.gaps.len(), 4);
    assert!(
        preview
            .source
            .gaps
            .iter()
            .all(|gap| gap.source_path.ends_with("/image"))
    );
}

#[test]
fn four_explicit_models_import_and_run_static_ipv4_in_both_directions() {
    let native = import_containerlab(SOURCE, &options()).unwrap();
    assert_eq!(validate_lab(&native).unwrap().devices, 4);
    let doc: Value = serde_json::from_str(&native).unwrap();
    assert_eq!(doc["format_version"], 3);
    assert_eq!(device(&doc, "pc-a")["port"]["id"], "pc-a:eth1");
    assert_eq!(device(&doc, "pc-a")["port"]["name"], "eth1");
    assert_eq!(device(&doc, "pc-a")["name"], "pc-a");
    assert_eq!(device(&doc, "r1")["routes"][0]["egress"], "r1:eth2");
    assert!(
        doc["links"]
            .as_array()
            .unwrap()
            .iter()
            .all(|l| l["delay_ns"] == "100")
    );
    let report = serde_json::to_value(run_lab(&native).unwrap()).unwrap();
    assert_eq!(report["probes"].as_array().unwrap().len(), 2);
    for probe in report["probes"].as_array().unwrap() {
        assert_eq!(probe["outcome"]["status"], "undetermined");
        assert_eq!(probe["outcome"]["modeled"]["status"], "reachable");
    }
}

#[test]
fn unknown_network_configuration_is_preserved_and_prevents_a_confirmed_result() {
    let mut input = source();
    input["topology"]["nodes"]["r2"]["vendor-policy"] = json!({"return": "deny"});
    let native = import_containerlab(&input.to_string(), &options()).unwrap();
    let doc: Value = serde_json::from_str(&native).unwrap();
    let gap = doc["import_source"]["gaps"]
        .as_array()
        .unwrap()
        .iter()
        .find(|gap| gap["source_path"] == "/topology/nodes/r2/vendor-policy")
        .unwrap();
    assert_eq!(gap["source_path"], "/topology/nodes/r2/vendor-policy");
    assert_eq!(gap["value"], json!({"return": "deny"}));
    assert_eq!(gap["scope"], json!({"kind": "device", "device_id": "r2"}));
    let report = serde_json::to_value(run_lab(&native).unwrap()).unwrap();
    assert_eq!(report["probes"][0]["outcome"]["status"], "undetermined");
    assert_eq!(
        report["probes"][0]["outcome"]["modeled"]["status"],
        "reachable"
    );
}

#[test]
fn brief_and_expanded_veth_have_the_same_ids_after_reordering() {
    let mut input = source();
    let first = imported(SOURCE, &options());
    input["topology"]["links"][0] = json!({"type": "veth", "endpoints": [
        {"node": "r1", "interface": "eth1"}, {"node": "pc-a", "interface": "eth1"}
    ]});
    input["topology"]["links"].as_array_mut().unwrap().reverse();
    let second = imported(&input.to_string(), &options());
    assert_eq!(first["links"], second["links"]);
    assert_eq!(
        preview_containerlab(SOURCE).unwrap().links,
        preview_containerlab(&input.to_string()).unwrap().links
    );
}

#[test]
fn link_ids_do_not_collide_when_endpoint_names_contain_delimiters() {
    let input = json!({"name":"names","topology":{"nodes":{"a":{},"b--c":{},"a--b":{},"c":{}},
        "links":[{"endpoints":["a:x","b--c:y"]},{"endpoints":["a--b:x","c:y"]}]}});
    let preview = preview_containerlab(&input.to_string()).unwrap();
    assert_ne!(preview.links[0].id, preview.links[1].id);
}

#[test]
fn execution_and_nonlocal_inputs_remain_literal_global_gaps() {
    let mut input = source();
    let settings = json!({"exec":["echo should-never-run"], "startup-config":"${HOME}/missing.cfg",
        "binds":["/missing:/etc/config"], "env":{"ADDRESS":"$UNKNOWN"}, "sysctls":{"net.ipv4.ip_forward": "0"}});
    input["topology"]["nodes"]["pc-a"]
        .as_object_mut()
        .unwrap()
        .extend(settings.as_object().unwrap().clone());
    let doc = imported(&input.to_string(), &options());
    let gaps = doc["import_source"]["gaps"].as_array().unwrap();
    for (key, value) in settings.as_object().unwrap() {
        let path = format!("/topology/nodes/pc-a/{key}");
        let gap = gaps.iter().find(|g| g["source_path"] == path).unwrap();
        assert_eq!(&gap["value"], value);
        assert_eq!(gap["scope"], json!({"kind":"all"}));
    }
}

#[test]
fn unknown_root_topology_defaults_kinds_and_groups_are_global() {
    let mut input = source();
    input["mgmt"] = json!({"ipv4-subnet":"172.20.0.0/24"});
    input["topology"]["defaults"] = json!({"exec":["ip route del default"]});
    input["topology"]["kinds"] = json!({"linux":{"env":{"A":"B"}}});
    input["topology"]["groups"] = json!({"core":{"mtu":9000}});
    input["topology"]["new-setting"] = json!([1, 2]);
    let doc = imported(&input.to_string(), &options());
    let global: Vec<_> = doc["import_source"]["gaps"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|gap| gap["scope"]["kind"] == "all")
        .collect();
    for gap in &global {
        assert_eq!(gap["scope"], json!({"kind":"all"}));
        assert_eq!(
            &gap["value"],
            input.pointer(gap["source_path"].as_str().unwrap()).unwrap()
        );
    }
    assert_eq!(global.len(), 5);
}

#[test]
fn runtime_identity_is_metadata_and_vendor_kind_still_has_a_device_gap() {
    let mut input = source();
    input["prefix"] = json!("");
    input["topology"]["nodes"]["r1"]["kind"] = json!("nokia_srlinux");
    input["topology"]["nodes"]["r1"]["image"] = json!("vendor/runtime:latest");
    input["topology"]["nodes"]["r1"]["labels"] = json!({"position":"1,2"});
    input["topology"]["nodes"]["r1"]["position"] = json!({"x":1,"y":2});
    let doc = imported(&input.to_string(), &options());
    let metadata = &doc["import_source"]["metadata"];
    assert_eq!(
        metadata["/topology/nodes/r1/image"],
        "vendor/runtime:latest"
    );
    assert_eq!(metadata["/topology/nodes/r1/kind"], "nokia_srlinux");
    assert_eq!(metadata["/prefix"], "");
    let gaps: Vec<_> = doc["import_source"]["gaps"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|gap| {
            gap["source_path"]
                .as_str()
                .unwrap()
                .starts_with("/topology/nodes/r1/")
        })
        .collect();
    assert_eq!(gaps.len(), 1);
    assert_eq!(gaps[0]["scope"], json!({"kind":"device","device_id":"r1"}));
    assert_eq!(gaps[0]["value"], "nokia_srlinux");
    assert_eq!(device(&doc, "r1")["kind"], "router");
}

#[test]
fn unsupported_link_is_retained_as_a_global_gap_and_never_converted() {
    let mut input = source();
    input["topology"]["links"][1]["type"] = json!("vxlan");
    input["topology"]["links"][1]["vni"] = json!(42);
    let preview = preview_containerlab(&input.to_string()).unwrap();
    assert_eq!(preview.links.len(), 2);
    assert_eq!(
        preview
            .source
            .gaps
            .iter()
            .filter(|gap| gap.source_path == "/topology/links/1")
            .count(),
        1
    );
    let doc = imported(&input.to_string(), &options());
    assert_eq!(doc["links"].as_array().unwrap().len(), 2);
    let gap = &doc["import_source"]["gaps"][0];
    assert_eq!(gap["scope"], json!({"kind":"all"}));
    assert_eq!(gap["value"], input["topology"]["links"][1]);
    assert!(gap["message"].as_str().unwrap().contains("不转换"));
}

#[test]
fn endpoint_and_link_extensions_keep_their_values_and_local_scope() {
    let mut input = source();
    input["topology"]["links"][0] = json!({"type":"veth","mtu":9000,"endpoints":[
        {"node":"pc-a","interface":"eth1","mac":"02:aa:bb:cc:dd:ee"},
        {"node":"r1","interface":"eth1"}]});
    let doc = imported(&input.to_string(), &options());
    let gaps = doc["import_source"]["gaps"].as_array().unwrap();
    let endpoint = gaps
        .iter()
        .find(|g| g["source_path"] == "/topology/links/0/endpoints/0/mac")
        .unwrap();
    assert_eq!(
        endpoint["scope"],
        json!({"kind":"port","port_id":"pc-a:eth1"})
    );
    assert_eq!(endpoint["value"], "02:aa:bb:cc:dd:ee");
    let link = gaps
        .iter()
        .find(|g| g["source_path"] == "/topology/links/0/mtu")
        .unwrap();
    assert_eq!(link["scope"]["kind"], "link");
    assert!(
        doc["links"]
            .as_array()
            .unwrap()
            .iter()
            .any(|l| l["id"] == link["scope"]["link_id"])
    );
}

#[test]
fn anchors_and_merges_are_applied_without_losing_inherited_fields() {
    let yaml = "name: merged\ntopology:\n  nodes:\n    a: &base\n      kind: linux\n      image: alpine\n      mtu: 9000\n    b:\n      <<: *base\n      image: busybox\n  links:\n    - endpoints: [a:eth1, b:eth1]\n";
    let preview = serde_json::to_value(preview_containerlab(yaml).unwrap()).unwrap();
    assert_eq!(
        preview["source"]["metadata"]["/topology/nodes/b/image"],
        "busybox"
    );
    assert!(
        preview["source"]["gaps"]
            .as_array()
            .unwrap()
            .iter()
            .any(|g| g["source_path"] == "/topology/nodes/b/mtu" && g["value"] == 9000)
    );
}

#[test]
fn linux_images_are_device_gaps_even_if_the_image_is_inherited_or_kind_is_omitted() {
    let yaml = "name: images\ntopology:\n  nodes:\n    a: &base\n      kind: linux\n      image: alpine\n    b:\n      <<: *base\n    c:\n      image: busybox\n";
    let source = serde_json::to_value(preview_containerlab(yaml).unwrap().source).unwrap();
    for (id, image) in [("a", "alpine"), ("b", "alpine"), ("c", "busybox")] {
        let path = format!("/topology/nodes/{id}/image");
        assert_eq!(source["metadata"][&path], image);
        let gap = source["gaps"]
            .as_array()
            .unwrap()
            .iter()
            .find(|gap| gap["source_path"] == path.as_str())
            .unwrap();
        assert_eq!(gap["value"], image);
        assert_eq!(gap["scope"], json!({"kind":"device", "device_id":id}));
    }
}

#[test]
fn inherited_topology_images_keep_probes_undetermined_without_per_node_duplicates() {
    for (section, inherited) in [
        ("defaults", json!({"image":"alpine"})),
        ("kinds", json!({"linux":{"image":"alpine"}})),
        ("groups", json!({"core":{"image":"alpine"}})),
    ] {
        let mut input = source();
        for node in input["topology"]["nodes"]
            .as_object_mut()
            .unwrap()
            .values_mut()
        {
            node.as_object_mut().unwrap().remove("image");
            if section == "groups" {
                node["group"] = json!("core");
            }
        }
        input["topology"][section] = inherited;
        let native = import_containerlab(&input.to_string(), &options()).unwrap();
        let doc: Value = serde_json::from_str(&native).unwrap();
        let gaps = doc["import_source"]["gaps"].as_array().unwrap();
        assert!(gaps.iter().any(|gap| gap["source_path"]
            == format!("/topology/{section}").as_str()
            && gap["scope"] == json!({"kind":"all"})));
        assert!(
            gaps.iter()
                .all(|gap| !gap["source_path"].as_str().unwrap().ends_with("/image"))
        );
        let report = serde_json::to_value(run_lab(&native).unwrap()).unwrap();
        assert!(report["probes"].as_array().unwrap().iter().all(|probe| {
            probe["outcome"]["status"] == "undetermined"
                && probe["outcome"]["modeled"]["status"] == "reachable"
        }));
    }
}

#[test]
fn duplicate_yaml_keys_and_invalid_merges_are_real_yaml_errors() {
    for input in [
        "name: first\nname: second\ntopology: {nodes: {}}",
        "name: lab\ntopology: {nodes: {a: {kind: linux, kind: bridge}}}",
        "name: lab\ntopology: {nodes: {a: {<<: 42}}}",
    ] {
        assert!(matches!(
            preview_containerlab(input),
            Err(ImportError::Yaml { .. })
        ));
    }
}

#[test]
fn malformed_endpoints_templates_missing_nodes_and_colons_are_source_errors() {
    for endpoints in [
        json!(["absent:eth1", "r1:eth1"]),
        json!(["pc-a:${IFACE}", "r1:eth1"]),
        json!(["pc-a:{{ .Interface }}", "r1:eth1"]),
        json!(["pc-a:eth1:extra", "r1:eth1"]),
        json!([{"node":"pc-a","interface":"a:b"},{"node":"r1","interface":"eth1"}]),
        json!(["pc-a:eth1"]),
        json!(["pc-a", "r1:eth1"]),
    ] {
        let mut input = source();
        input["topology"]["links"][0]["endpoints"] = endpoints;
        assert!(matches!(
            preview_containerlab(&input.to_string()),
            Err(ImportError::Source { .. })
        ));
    }
    let input = json!({"name":"lab","topology":{"nodes":{"a:b":{}},"links":[]}});
    assert!(matches!(
        preview_containerlab(&input.to_string()),
        Err(ImportError::Source { .. })
    ));
}

#[test]
fn reserved_brief_endpoints_are_global_gaps_and_keep_the_native_side() {
    for (kind, explicit, swap) in [
        ("mgmt-net", false, false),
        ("host", false, true),
        ("macvlan", true, false),
    ] {
        let yaml = format!(
            "name: reserved\ntopology:\n  nodes:\n    a: {{kind: linux}}\n  links:\n    - {}endpoints: [{}]\n",
            if explicit { "type: brief\n      " } else { "" },
            if swap {
                format!("{kind}:eth-test, a:eth1")
            } else {
                format!("a:eth1, {kind}:eth-test")
            }
        );
        let preview = preview_containerlab(&yaml).unwrap();
        assert!(preview.links.is_empty());
        assert_eq!(preview.nodes.len(), 1);
        assert_eq!(preview.nodes[0].ports[0].id, "a:eth1");
        assert_eq!(preview.source.gaps.len(), 1);
        let gap = serde_json::to_value(&preview.source.gaps[0]).unwrap();
        assert_eq!(gap["scope"], json!({"kind":"all"}));
        assert_eq!(gap["source_path"], "/topology/links/0");
        let special = if swap { 0 } else { 1 };
        assert_eq!(
            gap["value"]["endpoints"][special],
            format!("{kind}:eth-test").as_str()
        );
    }
}

#[test]
fn a_reserved_side_does_not_hide_an_unknown_or_malformed_peer() {
    for (endpoints, bad_side) in [
        (["missing:eth1", "mgmt-net:eth-test"], 0),
        (["a:eth1", "host:"], 1),
        (["host:x", "macvlan:y"], 1),
    ] {
        let input = json!({"name":"reserved", "topology":{
            "nodes":{"a":{"kind":"linux"}}, "links":[{"endpoints":endpoints}]
        }});
        let error = preview_containerlab(&input.to_string()).unwrap_err();
        let ImportError::Source { path, .. } = error else {
            panic!("expected a source error for {endpoints:?}, got {error:?}");
        };
        assert_eq!(path, format!("/topology/links/0/endpoints/{bad_side}"));
    }
}

#[test]
fn source_values_that_cannot_be_preserved_as_json_are_rejected() {
    for input in [
        "name: lab\ntopology: {nodes: {a: {kind: linux}}}\nunknown: {.nan: value}",
        "name: lab\ntopology: {nodes: {a: {kind: linux}}}\nunknown: .inf",
        "name: lab\ntopology: {nodes: {a: {kind: linux}}}\nunknown: !vendor opaque",
    ] {
        assert!(preview_containerlab(input).is_err());
    }
}

#[test]
fn every_node_and_source_interface_requires_an_explicit_model() {
    let mut opts = options();
    opts.models.remove("r1");
    mapping(SOURCE, &opts, "/models/r1");
    let mut opts = options();
    opts.models.get_mut("r1").unwrap()["ports"]
        .as_object_mut()
        .unwrap()
        .remove("eth2");
    mapping(SOURCE, &opts, "/models/r1/ports/eth2");
    let mut opts = options();
    opts.models.get_mut("pc-a").unwrap()["interface"] = json!("eth9");
    mapping(SOURCE, &opts, "/models/pc-a/interface");
}

#[test]
fn required_native_fields_report_the_mapping_location() {
    for (node, field, path) in [
        ("pc-a", "kind", "/models/pc-a/kind"),
        ("pc-a", "ipv4", "/models/pc-a/ipv4"),
        ("pc-a", "mac", "/models/pc-a/mac"),
        ("pc-a", "prefix_length", "/models/pc-a/prefix_length"),
        ("pc-a", "interface", "/models/pc-a/interface"),
        ("r1", "routes", "/models/r1/routes"),
    ] {
        let mut opts = options();
        opts.models
            .get_mut(node)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .remove(field);
        mapping(SOURCE, &opts, path);
    }
    let mut opts = options();
    opts.models.get_mut("r1").unwrap()["ports"]["eth2"]
        .as_object_mut()
        .unwrap()
        .remove("mac");
    mapping(SOURCE, &opts, "/models/r1/ports/eth2/mac");
    let mut opts = options();
    opts.scenario.as_object_mut().unwrap().remove("end_ns");
    mapping(SOURCE, &opts, "/scenario/end_ns");
    let mut opts = options();
    opts.scenario["probes"][0]
        .as_object_mut()
        .unwrap()
        .remove("deadline_ns");
    mapping(SOURCE, &opts, "/scenario/probes/0/deadline_ns");
    let mut opts = options();
    opts.models.insert(
        "r1".into(),
        json!({"kind":"switch","ports":{
            "eth1":{"mode":{"kind":"access"}}, "eth2":{"mode":{"kind":"access","vlan":10}}
        }}),
    );
    mapping(SOURCE, &opts, "/models/r1/ports/eth1/mode/vlan");
}

#[test]
fn route_and_link_state_interface_references_are_expanded() {
    let mut opts = options();
    opts.models.get_mut("r1").unwrap()["link_state"] =
        json!({"peers":[{"port":"eth2","cost":10}],"advertise":["eth1"]});
    opts.models.get_mut("r2").unwrap()["link_state"] =
        json!({"peers":[{"port":"eth1","cost":10}],"advertise":["eth2"]});
    opts.scenario["routing_convergence_ns"] = json!("50");
    let doc = imported(SOURCE, &opts);
    assert_eq!(device(&doc, "r1")["routes"][0]["egress"], "r1:eth2");
    assert_eq!(
        device(&doc, "r1")["link_state"]["peers"][0]["port"],
        "r1:eth2"
    );
    assert_eq!(device(&doc, "r1")["link_state"]["advertise"][0], "r1:eth1");
    opts.models.get_mut("r1").unwrap()["routes"][0]["egress"] = json!("absent");
    mapping(SOURCE, &opts, "/models/r1/routes/0/egress");
}

#[test]
fn model_ids_cannot_override_source_identity_or_hide_unknown_fields() {
    let mut opts = options();
    opts.models.get_mut("pc-a").unwrap()["id"] = json!("someone-else");
    mapping(SOURCE, &opts, "/models/pc-a/id");
    let mut opts = options();
    opts.models.get_mut("r1").unwrap()["ports"]["eth1"]["id"] = json!("wrong-port");
    mapping(SOURCE, &opts, "/models/r1/ports/eth1/id");
    let mut opts = options();
    opts.models
        .insert("not-in-source".into(), json!({"kind":"host"}));
    mapping(SOURCE, &opts, "/models/not-in-source");
    let mut opts = options();
    opts.models.get_mut("pc-a").unwrap()["bogus-native-field"] = json!(true);
    assert!(matches!(
        import_containerlab(SOURCE, &opts),
        Err(ImportError::Validation { .. })
    ));
}

#[test]
fn native_ipv4_vlan_and_link_semantics_are_validated_by_the_existing_validator() {
    let mut opts = options();
    opts.models.get_mut("pc-a").unwrap()["ipv4"] = json!("999.1.1.1");
    let ImportError::Validation { error } = import_containerlab(SOURCE, &opts).unwrap_err() else {
        panic!("expected native validation");
    };
    assert!(error.location.path.starts_with("/devices/"));
    let mut input = source();
    let duplicate = input["topology"]["links"][0].clone();
    input["topology"]["links"]
        .as_array_mut()
        .unwrap()
        .push(duplicate);
    assert!(matches!(
        import_containerlab(&input.to_string(), &options()),
        Err(ImportError::Validation { .. })
    ));
    let mut opts = options();
    opts.models.insert("r1".into(), json!({"kind":"switch","ports":{"eth1":{"mode":{"kind":"access","vlan":0}},"eth2":{"mode":{"kind":"access","vlan":0}}}}));
    assert!(matches!(
        import_containerlab(SOURCE, &opts),
        Err(ImportError::Validation { .. })
    ));
}

#[test]
fn bridge_role_uses_explicit_switch_port_models() {
    let mut input = source();
    input["topology"]["nodes"]["r1"]["kind"] = json!("bridge");
    let mut opts = options();
    opts.models.insert(
        "r1".into(),
        json!({"kind":"switch","stp_priority":32768,"ports":{
            "eth1":{"name":"left","mode":{"kind":"access","vlan":10}},
            "eth2":{"mode":{"kind":"trunk","allowed_vlans":[10]}}
        }}),
    );
    opts.scenario["stp_convergence_ns"] = json!("50");
    let doc = imported(&input.to_string(), &opts);
    assert_eq!(device(&doc, "r1")["ports"][0]["id"], "r1:eth1");
    assert_eq!(device(&doc, "r1")["ports"][0]["name"], "left");
    let gap = doc["import_source"]["gaps"]
        .as_array()
        .unwrap()
        .iter()
        .find(|gap| gap["source_path"] == "/topology/nodes/r1/kind")
        .unwrap();
    assert_eq!(gap["value"], "bridge");
    assert_eq!(gap["scope"], json!({"kind":"all"}));
    opts.models.get_mut("r1").unwrap()["ports"]["eth1"]
        .as_object_mut()
        .unwrap()
        .remove("mode");
    mapping(&input.to_string(), &opts, "/models/r1/ports/eth1/mode");
    mapping(&input.to_string(), &options(), "/models/r1/kind");
}

#[test]
fn ios_interface_import_maps_vlan_fields_and_keeps_unknowns_scoped_after_archive() {
    let input = "name: vlan\ntopology:\n  nodes:\n    a: {}\n    sw: {}\n    b: {}\n  links:\n    - endpoints: [a:p, sw:eth1]\n    - endpoints: [sw:eth2, b:p]\n";
    let host = |ip: &str, mac: &str| {
        json!({
            "kind":"host","interface":"p","ipv4":ip,"prefix_length":24,"mac":mac
        })
    };
    let mut opts: ImportOptions = serde_json::from_value(json!({
        "id":"ios-lab","revision":"1","link_delay_ns":"100",
        "models":{
            "a":host("192.0.2.1","02:00:00:00:00:01"),
            "b":host("192.0.2.2","02:00:00:00:00:02"),
            "sw":{"kind":"switch","ports":{
                "eth1":{"ios_interface":"Gi0/1","ios_config":"interface Gi0/1\n switchport mode access\n switchport access vlan 10"},
                "eth2":{"ios_interface":"Gi0/2","ios_config":"interface Gi0/2\n switchport mode trunk\n switchport trunk native vlan 10\n switchport trunk allowed vlan 10,20-22\n ip access-group EDGE in"}
            }}
        },
        "scenario":{"end_ns":"2000","arp_lifetime_ns":"5000","mac_lifetime_ns":"5000",
            "probes":[{"id":"ping","source":"a","destination":"b","at_ns":"0","deadline_ns":"2000"}]}
    })).unwrap();
    let native = import_containerlab(input, &opts).unwrap();
    let doc: Value = serde_json::from_str(&native).unwrap();
    let ports = &device(&doc, "sw")["ports"];
    assert_eq!(ports[0]["mode"], json!({"kind":"access","vlan":10}));
    assert_eq!(
        ports[1]["mode"],
        json!({"kind":"trunk","allowed_vlans":[10,20,21,22],"native_vlan":10})
    );
    let source = &doc["import_source"];
    let base = "/models/sw/ports/eth2/ios_config";
    assert_eq!(source["metadata"][base]["field_lines"]["native_vlan"], 3);
    assert!(
        source["gaps"]
            .as_array()
            .unwrap()
            .iter()
            .any(|gap| gap["source_path"] == format!("{base}/lines/5")
                && gap["scope"] == json!({"kind":"port","port_id":"sw:eth2"}))
    );
    let report = run_lab(&native).unwrap();
    let mut bytes = Vec::new();
    reliner::write_archive(&report, None, None, &mut bytes).unwrap();
    let restored = reliner::read_archive(bytes.as_slice()).unwrap();
    let report = serde_json::to_value(restored.report).unwrap();
    assert_eq!(report["probes"][0]["outcome"]["status"], "undetermined");
    assert_eq!(
        report["probes"][0]["outcome"]["modeled"]["status"],
        "reachable"
    );
    assert_eq!(report["import_source"], *source);

    let port = &mut opts.models.get_mut("sw").unwrap()["ports"]["eth2"];
    port["ios_interface"] = json!("Gi0/9");
    mapping(input, &opts, base);
}

#[test]
fn ios_fallback_preserves_unknowns_and_display_names_do_not_bind_interfaces() {
    let mut opts: ImportOptions =
        serde_json::from_str(include_str!("../examples/import-ios-models.json")).unwrap();
    let port = &mut opts.models.get_mut("r1").unwrap()["ports"]["eth2"];
    port["ios_config"] = json!(
        "interface Gi0/1\n switchport mode dynamic auto\n switchport trunk allowed vlan 30,10-11"
    );
    port["ios_interface"] = json!("Gi0/1");
    port["mode"] = json!({"kind":"trunk", "allowed_vlans":[30,11,10,10]});
    let root = "/models/r1/ports/eth2/ios_config";
    let doc = imported(SOURCE, &opts);
    assert!(
        doc["import_source"]["gaps"]
            .as_array()
            .unwrap()
            .iter()
            .any(|g| g["source_path"] == format!("{root}/native_vlan"))
    );
    let port = &mut opts.models.get_mut("r1").unwrap()["ports"]["eth2"];
    port["ios_config"] =
        json!("interface Gi0/1\n switchport mode access\n switchport access vlan dynamic");
    port["mode"] = json!({"kind":"access","vlan":10});
    let doc = imported(SOURCE, &opts);
    assert!(doc["import_source"]["gaps"].as_array().unwrap().iter().any(|g|
        g["source_path"] == format!("{root}/lines/3") && g["value"]["field"] == "vlan"));
    let port = &mut opts.models.get_mut("r1").unwrap()["ports"]["eth2"];
    port.as_object_mut().unwrap().remove("ios_interface");
    port["name"] = json!("Gi0/1");
    mapping(SOURCE, &opts, root);
}

#[test]
fn import_maps_native_vlan_without_certifying_the_source_device_behavior() {
    let mut opts = options();
    opts.models.insert(
        "r1".into(),
        json!({"kind":"switch","ports":{
            "eth1":{"mode":{"kind":"access","vlan":10}},
            "eth2":{"mode":{"kind":"trunk","allowed_vlans":[10,20],"native_vlan":10}}
        }}),
    );
    let input = import_containerlab(SOURCE, &opts).unwrap();
    let settings = reliner::effective_settings(&input).unwrap();
    let native = settings
        .iter()
        .find(|entry| entry.key == "native_vlan")
        .unwrap();
    assert_eq!(native.location.port_id.as_deref(), Some("r1:eth2"));
    assert_eq!(native.value, 10);
    assert_eq!(native.source, reliner::SettingSource::Explicit);
    let report = serde_json::to_value(run_lab(&input).unwrap()).unwrap();
    assert_eq!(report["model_version"], 12);
    assert!(
        report["probes"]
            .as_array()
            .unwrap()
            .iter()
            .all(|probe| probe["outcome"]["status"] == "undetermined")
    );
}

#[test]
fn host_external_node_kinds_carry_global_coverage() {
    for kind in ["bridge", "ovs-bridge", "ext-container"] {
        let yaml = format!("name: external\ntopology:\n  nodes:\n    external: {{kind: {kind}}}\n");
        let preview = preview_containerlab(&yaml).unwrap();
        assert_eq!(preview.source.gaps.len(), 1);
        let gap = serde_json::to_value(&preview.source.gaps[0]).unwrap();
        assert_eq!(gap["source_path"], "/topology/nodes/external/kind");
        assert_eq!(gap["scope"], json!({"kind":"all"}));
    }
}

#[test]
fn host_native_port_settings_and_display_names_are_preserved() {
    let mut opts = options();
    opts.models.get_mut("pc-a").unwrap()["name"] = json!("Client A");
    opts.models.get_mut("pc-a").unwrap()["port"] = json!({"name":"uplink", "ipv4_mtu_bytes":1400});
    let doc = imported(SOURCE, &opts);
    let host = device(&doc, "pc-a");
    assert_eq!(host["name"], "Client A");
    assert_eq!(
        host["port"],
        json!({"id":"pc-a:eth1", "name":"uplink", "ipv4_mtu_bytes":1400})
    );
}

#[test]
fn skipped_links_still_require_models_for_their_source_interfaces() {
    let mut input = source();
    input["topology"]["links"][1]["type"] = json!("vxlan");
    let mut opts = options();
    opts.models.get_mut("r1").unwrap()["ports"]
        .as_object_mut()
        .unwrap()
        .remove("eth2");
    opts.models.get_mut("r1").unwrap()["routes"] = json!([]);
    mapping(&input.to_string(), &opts, "/models/r1/ports/eth2");
}

#[test]
fn yaml_json_pointer_paths_escape_node_names_and_unmapped_keys() {
    let input = "name: paths\ntopology:\n  nodes:\n    a/b~c:\n      kind: linux\n      x/y~z: {nested: value}\n";
    let preview = preview_containerlab(input).unwrap();
    assert_eq!(
        preview.source.gaps[0].source_path,
        "/topology/nodes/a~1b~0c/x~1y~0z"
    );
    assert!(preview.missing.iter().any(|r| r.path == "/models/a~1b~0c"));
}
