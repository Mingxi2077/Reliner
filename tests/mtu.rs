use reliner::run_lab;
use serde_json::{Value, json};

const MTU: &str = include_str!("../examples/mtu-lab.json");
const IPV4: &str = include_str!("../examples/ipv4-lab.json");
const VLAN: &str = include_str!("../examples/vlan-lab.json");

fn lab() -> Value {
    serde_json::from_str(MTU).unwrap()
}

fn report(doc: &Value) -> Value {
    serde_json::to_value(run_lab(&doc.to_string()).unwrap()).unwrap()
}

fn mtu_cause(result: &Value, probe: usize) -> &Value {
    let probe = &result["probes"][probe];
    assert_eq!(probe["outcome"]["status"], "timeout");
    let evidence = result["evidence"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == probe["outcome"]["cause"])
        .unwrap();
    assert_eq!(evidence["probe_id"], probe["id"]);
    assert_eq!(evidence["fact"]["kind"], "ipv4_mtu_exceeded");
    assert_eq!(evidence["fact"]["ipv4_total_bytes"], 1201);
    assert_eq!(evidence["fact"]["mtu_bytes"], 1200);
    evidence
}

#[test]
fn small_and_exact_fit_pass_and_one_byte_over_has_egress_evidence() {
    let mut doc = lab();
    let result = report(&doc);
    assert_eq!(result["report_version"], 4);
    assert_eq!(result["model_version"], 14);
    assert_eq!(result["settings"]["ipv4_dont_fragment"], true);
    assert_eq!(result["trace"]["status"], "disabled");
    assert_eq!(result["probes"][0]["outcome"]["at_ns"], "1600");
    assert_eq!(result["probes"][1]["outcome"]["at_ns"], "2600");
    assert_eq!(result["probes"][0]["outcome"]["status"], "reachable");
    assert_eq!(result["probes"][1]["outcome"]["status"], "reachable");
    assert_eq!(result["probes"][2]["payload_bytes"], 1173);
    assert_eq!(result["probes"][2]["outcome"]["at_ns"], "3800");
    assert_eq!(result["probes"][2]["outcome"]["waiting_for"], "echo_reply");
    assert!(result["probes"][2]["request_evidence_id"].is_null());
    let cause = mtu_cause(&result, 2);
    assert_eq!(cause["at_ns"], "3100");
    assert_eq!(cause["fact"]["packet_kind"], "echo_request");
    assert_eq!(cause["port"]["port_id"], "r1-transit");
    assert_eq!(cause["frame"]["kind"], "echo_request");

    doc["scenario"]["trace_limit"] = json!(100);
    let traced = report(&doc);
    assert_eq!(traced["probes"], result["probes"]);
    assert_eq!(traced["evidence"], result["evidence"]);
    assert!(
        traced["trace"]["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|event| {
                event["event_id"] == cause["fact"]["event_id"]
                    && event["route"]["egress_port_id"] == "r1-transit"
            })
    );
    let entries = traced["trace"]["entries"].as_array().unwrap();
    let route_check = |id: &str| {
        &entries
            .iter()
            .find(|event| {
                event["probe_id"] == id
                    && event["frame"]["kind"] == "echo_request"
                    && event["route"]["egress_port_id"] == "r1-transit"
            })
            .unwrap()["route"]["ipv4_mtu"]
    };
    assert_eq!(
        route_check("at-mtu"),
        &json!({
            "packet_kind": "echo_request", "ipv4_total_bytes": 1200, "mtu_bytes": 1200, "passed": true,
        })
    );
    assert_eq!(
        route_check("over-mtu"),
        &json!({
            "packet_kind": "echo_request", "ipv4_total_bytes": 1201, "mtu_bytes": 1200, "passed": false,
        })
    );
}

#[test]
fn a_reverse_egress_limit_preserves_request_arrival_evidence() {
    for (pointer, port, at) in [
        ("/devices/2/ports/0", "r2-transit", "3400"),
        ("/devices/3/port", "b-eth0", "3300"),
    ] {
        let mut doc = lab();
        doc["devices"][1]["ports"][1]["ipv4_mtu_bytes"] = json!(1500);
        doc.pointer_mut(pointer).unwrap()["ipv4_mtu_bytes"] = json!(1200);
        doc["scenario"]["trace_limit"] = json!(100);
        let result = report(&doc);
        let cause = mtu_cause(&result, 2);
        assert_eq!(cause["port"]["port_id"], port);
        assert_eq!(cause["at_ns"], at);
        assert_eq!(cause["fact"]["packet_kind"], "echo_reply");
        assert_ne!(cause["frame"]["parent_id"], cause["frame"]["id"]);
        let request_id = &result["probes"][2]["request_evidence_id"];
        let request = result["evidence"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["id"] == *request_id)
            .unwrap();
        assert_eq!(request["fact"]["kind"], "echo_request_received");
        assert_eq!(request["at_ns"], "3300");
        let event = result["trace"]["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|event| event["event_id"] == cause["fact"]["event_id"])
            .unwrap();
        assert_eq!(event["frame"], cause["frame"]);
        assert_eq!(event["route"]["ipv4_mtu"]["packet_kind"], "echo_reply");
        if port == "b-eth0" {
            assert_eq!(cause["frame"], request["frame"]);
            assert_eq!(cause["frame"]["kind"], "echo_request");
        }
    }
}

#[test]
fn oversized_local_request_never_starts_arp() {
    let mut doc = lab();
    doc["devices"][0]["port"]["ipv4_mtu_bytes"] = json!(1200);
    doc["scenario"]["warmup"] = json!([]);
    doc["scenario"]["probes"].as_array_mut().unwrap().drain(..2);
    doc["scenario"]["trace_limit"] = json!(100);
    let result = report(&doc);
    let cause = mtu_cause(&result, 0);
    assert_eq!(
        result["probes"][0]["outcome"]["waiting_for"],
        "probe_deadline"
    );
    assert_eq!(cause["port"]["port_id"], "a-eth0");
    assert_eq!(cause["at_ns"], "3000");
    assert_eq!(cause["frame"]["kind"], "echo_request");
    let events: Vec<_> = result["trace"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["event"].as_str().unwrap())
        .collect();
    assert_eq!(events, ["probe_start", "probe_deadline"]);
}

#[test]
fn accepts_ipv4_length_boundaries_and_rejects_out_of_range_input() {
    let mut doc: Value = serde_json::from_str(IPV4).unwrap();
    doc["devices"][1]["ports"][1]["ipv4_mtu_bytes"] = json!(65535);
    doc["scenario"]["probes"][0]["payload_bytes"] = json!(65507);
    let result = report(&doc);
    assert_eq!(result["probes"][0]["outcome"]["status"], "reachable");
    assert_eq!(result["probes"][1]["payload_bytes"], 56);
    doc["scenario"]["probes"][0]["payload_bytes"] = json!(65508);
    assert!(
        run_lab(&doc.to_string())
            .unwrap_err()
            .to_string()
            .contains("65507")
    );

    doc["scenario"]["probes"][0]["payload_bytes"] = json!(0);
    doc["devices"][1]["ports"][1]["ipv4_mtu_bytes"] = json!(68);
    assert_eq!(report(&doc)["probes"][0]["outcome"]["status"], "reachable");
    doc["devices"][1]["ports"][1]["ipv4_mtu_bytes"] = json!(67);
    assert!(
        run_lab(&doc.to_string())
            .unwrap_err()
            .to_string()
            .contains("r1-transit")
    );

    let mut legacy: Value = serde_json::from_str(VLAN).unwrap();
    legacy["devices"][0]["port"]["ipv4_mtu_bytes"] = json!(1500);
    assert!(
        run_lab(&legacy.to_string())
            .unwrap_err()
            .to_string()
            .contains("format_version 2")
    );
}

#[test]
fn route_mtu_follows_ttl_and_precedes_arp_on_a_down_egress() {
    let mut doc = lab();
    doc["scenario"]["warmup"] = json!([]);
    doc["scenario"]["probes"].as_array_mut().unwrap().drain(..2);
    doc["scenario"]["link_actions"] = json!([
        {"link": "r1-r2", "at_ns": "3300", "state": "down"}
    ]);
    let result = report(&doc);
    let cause = mtu_cause(&result, 0);
    assert_eq!(cause["port"]["port_id"], "r1-transit");
    assert_eq!(cause["at_ns"], "3300");

    doc["scenario"]["probes"][0]["ttl"] = json!(1);
    let ttl = report(&doc);
    let facts = ttl["evidence"].as_array().unwrap();
    assert_eq!(facts.last().unwrap()["fact"]["kind"], "ipv4_ttl_expired");

    doc["scenario"]["probes"][0]["ttl"] = json!(64);
    doc["scenario"]["probes"][0]["payload_bytes"] = json!(1172);
    let link = report(&doc);
    let facts = link["evidence"].as_array().unwrap();
    assert_eq!(facts.last().unwrap()["fact"]["kind"], "link_down");
    assert_eq!(facts.last().unwrap()["frame"]["kind"], "arp_request");
}

#[test]
fn tagged_links_pass_ipv4_at_the_limit_and_keep_failover_evidence_in_v4() {
    let mut doc: Value =
        serde_json::from_str(include_str!("../examples/failover-lab.json")).unwrap();
    doc["devices"][0]["port"]["ipv4_mtu_bytes"] = json!(1500);
    doc["devices"][3]["port"]["ipv4_mtu_bytes"] = json!(1499);
    doc["scenario"]["probes"][0]["payload_bytes"] = json!(1472);
    let result = report(&doc);
    assert_eq!(result["report_version"], 4);
    let first = &result["probes"][0];
    let facts = result["evidence"].as_array().unwrap();
    let request = facts
        .iter()
        .find(|item| item["id"] == first["request_evidence_id"])
        .unwrap();
    assert_eq!(request["fact"]["kind"], "echo_request_received");
    assert_eq!(request["port"]["port_id"], "pc-b-eth0");
    let reply = facts
        .iter()
        .find(|item| item["id"] == first["outcome"]["cause"])
        .unwrap();
    assert_eq!(reply["fact"]["packet_kind"], "echo_reply");
    assert_eq!(reply["frame"], request["frame"]);
    assert_eq!(reply["port"]["port_id"], "pc-b-eth0");
    assert_eq!(reply["fact"]["kind"], "ipv4_mtu_exceeded");
    assert_eq!(reply["fact"]["ipv4_total_bytes"], 1500);
    assert_eq!(reply["fact"]["mtu_bytes"], 1499);
    assert_eq!(
        result["link_events"][0]["protection"]["active_link_id"],
        "inter-switch-standby"
    );
    assert!(facts.iter().any(|item| item["probe_id"] == "on-standby"
        && item["fact"]["reason"]["rule"] == "trunk_vlan_not_allowed"
        && item["protected_link"]["link_id"] == "inter-switch-standby"));
}

#[test]
fn optional_mtu_fields_require_a_number_when_present() {
    for pointer in [
        "/devices/0/port/ipv4_mtu_bytes",
        "/devices/1/ports/1/ipv4_mtu_bytes",
        "/scenario/probes/0/payload_bytes",
    ] {
        let mut doc = lab();
        *doc.pointer_mut(pointer).unwrap() = Value::Null;
        let result = run_lab(&doc.to_string());
        assert!(result.is_err(), "null accepted at {pointer}");
    }
}
