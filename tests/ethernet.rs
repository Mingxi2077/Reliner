use reliner::run_experiment;
use serde_json::{Value, json};

fn lab() -> Value {
    serde_json::from_str(include_str!("../examples/vlan-lab.json")).unwrap()
}

fn permit_vlan(document: &mut Value) {
    document["devices"][2]["ports"][0]["mode"]["allowed_vlans"] = json!([10, 20]);
    document["revision"] = json!("2");
}

fn report(document: &Value) -> Value {
    serde_json::to_value(reliner::run_lab(&document.to_string()).unwrap()).unwrap()
}

fn native_vlan_lab() -> Value {
    let mut document: Value =
        serde_json::from_str(include_str!("../examples/native-vlan-lab.json")).unwrap();
    document["devices"][2]["ports"][0]["mode"]["native_vlan"] = json!(10);
    document
}

#[test]
fn native_vlan_frames_cross_a_trunk_without_a_tag() {
    let mut document = native_vlan_lab();
    let connected = report(&document);
    assert_eq!(connected["probes"][0]["outcome"]["status"], "reachable");
    assert_eq!(connected["model_version"], 12);

    document["devices"][2]["ports"][0]["mode"]
        .as_object_mut()
        .unwrap()
        .remove("native_vlan");
    let broken = report(&document);
    let outcome = &broken["probes"][0]["outcome"];
    assert_eq!(outcome["status"], "timeout");
    let evidence = broken["evidence"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == outcome["cause"])
        .unwrap();
    assert_eq!(evidence["port"]["port_id"], "sw2-uplink");
    assert_eq!(evidence["fact"]["reason"]["rule"], "trunk_requires_tag");
}

#[test]
fn untagged_frame_uses_the_receiving_native_vlan_and_its_allow_list() {
    let mut document = native_vlan_lab();
    document["devices"][2]["ports"][0]["mode"]["native_vlan"] = json!(20);
    document["devices"][2]["ports"][0]["mode"]["allowed_vlans"] = json!([10]);
    let broken = report(&document);
    let outcome = &broken["probes"][0]["outcome"];
    assert_eq!(outcome["status"], "timeout");
    let evidence = broken["evidence"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == outcome["cause"])
        .unwrap();
    assert_eq!(evidence["port"]["port_id"], "sw2-uplink");
    assert_eq!(
        evidence["fact"]["reason"],
        json!({"rule":"trunk_vlan_not_allowed", "vlan":20, "allowed_vlans":[10]})
    );
}

#[test]
fn non_native_vlan_remains_tagged_across_the_trunk() {
    let mut document = native_vlan_lab();
    document["devices"][1]["ports"][1]["mode"]["native_vlan"] = json!(20);
    document["devices"][2]["ports"][0]["mode"]
        .as_object_mut()
        .unwrap()
        .remove("native_vlan");
    assert_eq!(
        report(&document)["probes"][0]["outcome"]["status"],
        "reachable"
    );
}

#[test]
fn mismatched_native_vlans_preserve_the_actual_egress_drop_in_the_archive() {
    let input = include_str!("../examples/native-vlan-lab.json");
    let modeled = reliner::run_lab(input).unwrap();
    let mut archive = Vec::new();
    reliner::write_archive(&modeled, None, None, &mut archive).unwrap();
    let reopened = reliner::read_archive(archive.as_slice()).unwrap();
    assert_eq!(reopened.report, modeled);
    let report = serde_json::to_value(reopened.report).unwrap();
    let outcome = &report["probes"][0]["outcome"];
    assert_eq!(outcome["status"], "timeout");
    let evidence = report["evidence"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == outcome["cause"])
        .unwrap();
    assert_eq!(evidence["port"]["port_id"], "sw2-access");
    assert_eq!(evidence["fact"]["stage"], "egress");
    assert_eq!(
        evidence["fact"]["reason"],
        json!({"rule":"access_vlan_mismatch", "vlan":20, "access_vlan":10})
    );
}

#[test]
fn explicitly_configured_native_vlans_appear_on_effective_settings_pages() {
    let workspace = reliner::Workspace::new(reliner::WorkspaceConfig::default()).unwrap();
    let doc = workspace
        .open_document(native_vlan_lab().to_string())
        .unwrap();
    let first = workspace
        .effective_settings_page(doc.id, doc.revision, 0, 3)
        .unwrap()
        .page;
    assert_eq!(first.next_offset, Some(3));
    let entry = &first.entries[2];
    assert_eq!(entry.key, "native_vlan");
    assert_eq!(entry.location.path, "/devices/1/ports/1/mode/native_vlan");
    assert_eq!(entry.location.port_id.as_deref(), Some("sw1-uplink"));
    assert_eq!(entry.source, reliner::SettingSource::Explicit);
    assert_eq!(entry.value, 10);
    let last = workspace
        .effective_settings_page(doc.id, doc.revision, 3, 1)
        .unwrap()
        .page;
    assert_eq!(
        last.entries[0].location.port_id.as_deref(),
        Some("sw2-uplink")
    );
    assert_eq!(last.next_offset, None);
}

#[test]
fn invalid_native_vlan_points_to_its_original_port_and_field() {
    let mut document = native_vlan_lab();
    document["devices"][2]["ports"][0]["mode"]["native_vlan"] = json!(4095);
    let error = reliner::validate_lab(&document.to_string()).unwrap_err();
    assert_eq!(error.code, "vlan_out_of_range");
    assert_eq!(error.location.path, "/devices/2/ports/0/mode/native_vlan");
    assert_eq!(error.location.device_id.as_deref(), Some("sw2"));
    assert_eq!(error.location.port_id.as_deref(), Some("sw2-uplink"));
}

#[test]
fn legacy_files_cannot_opt_into_native_vlan() {
    let mut document = lab();
    document["devices"][2]["ports"][0]["mode"]["native_vlan"] = json!(10);
    let error = reliner::validate_lab(&document.to_string()).unwrap_err();
    assert_eq!(error.code, "invalid_native_vlan");
    assert_eq!(error.location.path, "/devices/2/ports/0/mode/native_vlan");
}

fn add_filtered_branch(document: &mut Value) {
    let sw1_ports = document["devices"][1]["ports"].as_array_mut().unwrap();
    sw1_ports.push(json!({
        "id": "sw1-side", "name": "G0/3", "mode": {"kind": "access", "vlan": 20}
    }));
    document["devices"].as_array_mut().unwrap().push(json!({
        "kind": "host", "id": "side", "name": "Side", "ipv4": "192.0.2.30",
        "prefix_length": 24, "mac": "02:00:00:00:00:0c",
        "port": {"id": "side-eth0", "name": "eth0"}
    }));
    document["links"].as_array_mut().unwrap().push(json!({
        "id": "side-link", "ports": ["sw1-side", "side-eth0"], "delay_ns": "100"
    }));
}

#[test]
fn delivered_echo_reply_follows_arp_resolution() {
    let mut document = lab();
    permit_vlan(&mut document);
    let result = report(&document);

    assert_eq!(result["topology_id"], "vlan-lab");
    assert_eq!(result["revision"], "2");
    assert_eq!(result["outcome"]["status"], "reachable");
    assert_eq!(result["outcome"]["at_ns"], "1200");
    let evidence = result["evidence"].as_array().unwrap();
    assert_eq!(evidence[0]["fact"]["kind"], "arp_resolved");
    assert_eq!(evidence[0]["at_ns"], "600");
    assert_eq!(evidence[1]["fact"]["kind"], "echo_reply_received");
    assert_eq!(result["outcome"]["evidence_id"], evidence[1]["id"]);
}

#[test]
fn timeout_points_to_the_trunk_that_rejected_the_arp_request() {
    let result = report(&lab());
    let outcome = &result["outcome"];
    assert_eq!(outcome["status"], "timeout");
    assert_eq!(outcome["at_ns"], "2000");
    assert_eq!(outcome["waiting_for"], "arp_reply");

    let evidence = result["evidence"].as_array().unwrap();
    let cause = evidence
        .iter()
        .find(|item| item["id"] == outcome["cause"])
        .unwrap();
    assert_eq!(cause["port"]["device_id"], "sw2");
    assert_eq!(cause["port"]["port_id"], "sw2-uplink");
    assert_eq!(cause["at_ns"], "200");
    assert_eq!(cause["frame"]["kind"], "arp_request");
    assert!(cause["frame"]["parent_id"].is_string());
    assert_eq!(cause["fact"]["stage"], "ingress");
    assert_eq!(
        cause["fact"]["reason"],
        json!({
            "rule": "trunk_vlan_not_allowed", "vlan": 10, "allowed_vlans": [20]
        })
    );
}

#[test]
fn a_filtered_broadcast_branch_does_not_prevent_an_echo_reply() {
    let mut document = lab();
    permit_vlan(&mut document);
    add_filtered_branch(&mut document);
    let result = report(&document);

    assert_eq!(result["outcome"]["status"], "reachable");
    let evidence = result["evidence"].as_array().unwrap();
    let side: Vec<_> = evidence
        .iter()
        .filter(|item| item["port"]["port_id"] == "sw1-side")
        .collect();
    assert_eq!(side.len(), 1, "learned unicast should stay off the branch");
    assert_eq!(side[0]["frame"]["kind"], "arp_request");
    assert_eq!(side[0]["fact"]["stage"], "egress");
}

#[test]
fn a_filtered_broadcast_branch_is_not_used_to_explain_a_timeout() {
    let mut document = lab();
    add_filtered_branch(&mut document);
    let result = report(&document);
    let evidence = result["evidence"].as_array().unwrap();
    assert_eq!(evidence[0]["port"]["port_id"], "sw1-side");
    assert_eq!(evidence[1]["port"]["port_id"], "sw2-uplink");
    assert_eq!(result["outcome"]["cause"], evidence[1]["id"]);

    let mut disconnected = document;
    disconnected["links"].as_array_mut().unwrap().remove(2);
    let result = report(&disconnected);
    assert_eq!(result["outcome"]["status"], "timeout");
    assert!(result["outcome"]["cause"].is_null());
}

#[test]
fn deadline_has_priority_over_an_echo_reply_at_the_same_time() {
    let mut document = lab();
    permit_vlan(&mut document);
    document["probe"]["deadline_ns"] = json!("1200");
    let late = report(&document);
    assert_eq!(late["outcome"]["status"], "timeout");
    assert_eq!(late["outcome"]["waiting_for"], "echo_reply");
    assert!(late["outcome"]["cause"].is_null());
    assert_eq!(late["evidence"].as_array().unwrap().len(), 1);

    document["probe"]["deadline_ns"] = json!("1201");
    assert_eq!(report(&document)["outcome"]["status"], "reachable");
}

#[test]
fn overflowing_a_frame_arrival_does_not_commit_the_partial_event() {
    let mut document = lab();
    add_filtered_branch(&mut document);
    document["links"][0]["delay_ns"] = json!("18446744073709551600");
    document["probe"]["deadline_ns"] = json!("18446744073709551615");
    let result = report(&document);
    assert_eq!(result["outcome"]["status"], "incomplete");
    assert_eq!(result["outcome"]["reason"], "time_overflow");
    assert_eq!(result["outcome"]["at_ns"], "0");
    assert!(result["evidence"].as_array().unwrap().is_empty());
}

#[test]
fn unsupported_configuration_is_rejected_instead_of_being_ignored() {
    let mut document = lab();
    document["devices"][2]["ports"][0]["mode"]["voice_vlan"] = json!(10);
    let error = run_experiment(&document.to_string()).unwrap_err();
    assert!(error.to_string().contains("voice_vlan"));

    document["format_version"] = json!(2);
    let error = run_experiment(&document.to_string()).unwrap_err();
    assert!(error.to_string().contains("format_version 2"));
}

#[test]
fn a_physical_switch_loop_is_rejected() {
    let mut document = lab();
    for (switch, port) in [(1, "sw1-extra"), (2, "sw2-extra")] {
        document["devices"][switch]["ports"]
            .as_array_mut()
            .unwrap()
            .push(json!({
                "id": port, "name": "G0/4", "mode": {"kind": "trunk", "allowed_vlans": [10]}
            }));
    }
    document["links"].as_array_mut().unwrap().push(json!({
        "id": "loop", "ports": ["sw1-extra", "sw2-extra"], "delay_ns": "100"
    }));
    let error = run_experiment(&document.to_string()).unwrap_err();
    assert!(error.to_string().contains("环路"));
}

#[test]
fn reversing_unordered_document_lists_does_not_change_the_report() {
    let mut baseline = lab();
    add_filtered_branch(&mut baseline);
    let mut reordered = baseline.clone();
    reordered["links"].as_array_mut().unwrap().reverse();
    for device in reordered["devices"].as_array_mut().unwrap().iter_mut() {
        if let Some(ports) = device.get_mut("ports").and_then(Value::as_array_mut) {
            ports.reverse();
        }
    }
    reordered["devices"].as_array_mut().unwrap().reverse();
    assert_eq!(report(&baseline), report(&reordered));
}
