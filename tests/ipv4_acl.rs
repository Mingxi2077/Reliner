use reliner::{RunLimits, read_archive, run_lab, run_lab_with_limits, write_archive};
use serde_json::{Value, json};

const LAB: &str = include_str!("../examples/acl-lab.json");

fn lab() -> Value {
    serde_json::from_str(LAB).unwrap()
}

fn report(doc: &Value) -> Value {
    serde_json::to_value(run_lab(&doc.to_string()).unwrap()).unwrap()
}

fn decision(report: &Value) -> &Value {
    report["evidence"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["fact"]["kind"] == "ipv4_acl_decision")
        .unwrap()
}

#[test]
fn shadowed_host_exception_blocks_reply_and_reordering_repairs_the_same_probe() {
    let mut doc = lab();
    let broken = report(&doc);
    let row = decision(&broken);
    let probe = &broken["probes"][0];
    assert_eq!(broken["report_version"], 9);
    assert_eq!(broken["model_version"], 13);
    assert_eq!(probe["outcome"]["status"], "timeout");
    assert_eq!(probe["outcome"]["cause"], row["id"]);
    assert!(probe["request_evidence_id"].is_string());
    assert_eq!(row["probe_id"], probe["id"]);
    assert_eq!(row["port"]["port_id"], "r1-a");
    assert_eq!(row["frame"]["kind"], "echo_reply");
    assert_eq!(row["at_ns"], "700");
    assert_eq!(
        row["fact"],
        json!({"kind":"ipv4_acl_decision", "event_id":row["frame"]["id"],
        "direction":"egress","source":"203.0.113.20","destination":"192.0.2.10",
        "action":"deny","matched_rule":{"rule_number":1,"network":"203.0.113.0","prefix_length":24}})
    );
    doc["devices"][1]["ports"][0]["ipv4_acl"]["egress"]
        .as_array_mut()
        .unwrap()
        .swap(0, 1);
    let fixed = report(&doc);
    assert_eq!(fixed["probes"][0]["outcome"]["status"], "reachable");
    assert_eq!(fixed["probes"][0]["outcome"]["at_ns"], "800");
    let fact = &decision(&fixed)["fact"];
    assert_eq!(fact["action"], "permit");
    assert_eq!(
        fact["matched_rule"],
        json!({"rule_number":1,"network":"203.0.113.20","prefix_length":32})
    );
}

#[test]
fn ingress_precedes_ttl_and_implicit_deny_is_packet_specific() {
    for rules in [
        json!([]),
        json!([{"action":"permit","network":"203.0.113.20","prefix_length":32}]),
    ] {
        let mut doc = lab();
        doc["devices"][1]["ports"][0]["ipv4_acl"] = json!({"ingress":rules});
        doc["scenario"]["probes"][0]["ttl"] = json!(1);
        let result = report(&doc);
        let e = decision(&result);
        assert_eq!(e["fact"]["direction"], "ingress");
        assert_eq!(e["port"]["port_id"], "r1-a");
        assert_eq!(e["frame"]["kind"], "echo_request");
        assert_eq!(e["fact"]["source"], "192.0.2.10");
        assert_eq!(e["fact"]["matched_rule"], Value::Null);
        assert_eq!(e["fact"]["action"], "deny");
        assert_eq!(e["at_ns"], "300");
        assert_eq!(result["probes"][0]["outcome"]["cause"], e["id"]);
        assert!(result["probes"][0]["request_evidence_id"].is_null());
    }
}

#[test]
fn egress_rejection_precedes_mtu_and_neighbor_resolution() {
    let mut doc = lab();
    let port = &mut doc["devices"][1]["ports"][1];
    port["ipv4_acl"] = json!({"egress":[]});
    port["ipv4_mtu_bytes"] = json!(68);
    doc["scenario"]["trace_limit"] = json!(64);
    let blocked = report(&doc);
    let e = decision(&blocked);
    assert_eq!(e["port"]["port_id"], "r1-b");
    assert_eq!(e["fact"]["action"], "deny");
    assert_eq!(e["id"], blocked["probes"][0]["outcome"]["cause"]);
    let entries = blocked["trace"]["entries"].as_array().unwrap();
    let arrival = entries
        .iter()
        .find(|t| t["event_id"] == e["fact"]["event_id"])
        .unwrap();
    assert_eq!(arrival["route"]["egress_port_id"], "r1-b");
    assert!(arrival["route"].get("ipv4_mtu").is_none());
    assert!(!entries.iter().any(|t| t["port"]["port_id"] == "b-eth0"));

    doc["devices"][1]["ports"][1]["ipv4_acl"]["egress"] =
        json!([{"action":"permit","network":"192.0.2.0","prefix_length":24}]);
    let allowed = report(&doc);
    assert_eq!(decision(&allowed)["fact"]["action"], "permit");
    let cause = &allowed["probes"][0]["outcome"]["cause"];
    let mtu = allowed["evidence"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| &e["id"] == cause)
        .unwrap();
    assert_eq!(mtu["fact"]["kind"], "ipv4_mtu_exceeded");
    assert_eq!(mtu["frame"], decision(&allowed)["frame"]);
}

#[test]
fn both_interfaces_bind_the_same_frame_and_survive_trace_truncation() {
    let mut doc = lab();
    let any = json!({"action":"permit","network":"0.0.0.0","prefix_length":0});
    doc["devices"][1]["ports"][0]["ipv4_acl"] = json!({"ingress":[any]});
    doc["devices"][1]["ports"][1]["ipv4_acl"] = json!({"egress":[
        {"action":"deny","network":"192.0.2.9","prefix_length":32}, any
    ]});
    let baseline = report(&doc);
    let checks: Vec<_> = baseline["evidence"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["fact"]["kind"] == "ipv4_acl_decision")
        .collect();
    assert_eq!(checks.len(), 2);
    let (ingress, egress) = (checks[0], checks[1]);
    assert_eq!(ingress["frame"], egress["frame"]);
    assert_eq!(ingress["port"]["port_id"], "r1-a");
    assert_eq!(egress["port"]["port_id"], "r1-b");
    assert_eq!(egress["fact"]["matched_rule"]["rule_number"], 2);
    assert_eq!(baseline["probes"][0]["outcome"]["at_ns"], "800");
    for limit in [1, 100] {
        doc["scenario"]["trace_limit"] = json!(limit);
        let traced = report(&doc);
        assert_eq!(traced["evidence"], baseline["evidence"]);
        assert_eq!(traced["probes"], baseline["probes"]);
        if limit == 100 {
            let event = traced["trace"]["entries"]
                .as_array()
                .unwrap()
                .iter()
                .find(|e| e["event_id"] == egress["fact"]["event_id"])
                .unwrap();
            assert_eq!(event["frame"], ingress["frame"]);
            assert_eq!(event["port"], ingress["port"]);
            assert_eq!(event["route"]["egress_port_id"], egress["port"]["port_id"]);
        }
    }
}

#[test]
fn work_and_fact_limits_stop_before_the_acl_event_commits() {
    let mut doc = lab();
    let permit = json!({"action":"permit","network":"0.0.0.0","prefix_length":0});
    doc["devices"][1]["ports"][0]["ipv4_acl"] = json!({"ingress":[permit]});
    doc["devices"][1]["ports"][1]["ipv4_acl"] = json!({"egress":[permit]});
    let run = |limits| {
        serde_json::to_value(run_lab_with_limits(&doc.to_string(), limits).unwrap()).unwrap()
    };
    for (limits, name) in [
        (
            RunLimits {
                event_work: 8,
                ..Default::default()
            },
            "event_work",
        ),
        (
            RunLimits {
                total_work: 3 + 3 + 5 + 8,
                ..Default::default()
            },
            "total_work",
        ),
        (
            RunLimits {
                essential_facts: 2,
                ..Default::default()
            },
            "essential_facts",
        ),
    ] {
        let stopped = run(limits);
        assert_eq!(
            stopped["stop_reason"],
            json!({"resource_limit":{"limit":name}})
        );
        assert_eq!(stopped["last_event_ns"], "200");
        assert_eq!(stopped["probes"][0]["outcome"]["status"], "incomplete");
        assert_eq!(stopped["evidence"].as_array().unwrap().len(), 1);
    }
    let exact = run(RunLimits {
        event_work: 9,
        ..Default::default()
    });
    assert_eq!(exact["probes"][0]["outcome"]["status"], "reachable");
}

#[test]
fn invalid_policies_reject_and_semantic_errors_locate_the_declaration() {
    let base = "/devices/1/ports/0/ipv4_acl";
    let rule = json!({"action":"permit","network":"0.0.0.0","prefix_length":0});
    for (value, suffix) in [
        (json!({}), ""),
        (json!({"egress":vec![rule.clone();65]}), "/egress"),
        (
            json!({"ingress":[{"action":"deny","network":"192.0.2.1","prefix_length":24}]}),
            "/ingress/0/network",
        ),
        (
            json!({"egress":[{"action":"permit","network":"192.0.2.1","prefix_length":33}]}),
            "/egress/0/prefix_length",
        ),
    ] {
        let mut doc = lab();
        *doc.pointer_mut(base).unwrap() = value;
        let err = run_lab(&doc.to_string()).unwrap_err();
        assert_eq!(err.location.path, format!("{base}{suffix}"));
        assert_eq!(err.location.port_id.as_deref(), Some("r1-a"));
    }
    for policy in [
        Value::Null,
        json!({"egress":null}),
        json!({"outbound":[]}),
        json!({"egress":[{"action":"pass","network":"0.0.0.0","prefix_length":0}]}),
        json!({"egress":[{"action":"permit","network":"::","prefix_length":0}]}),
    ] {
        let mut doc = lab();
        *doc.pointer_mut(base).unwrap() = policy;
        assert!(run_lab(&doc.to_string()).is_err());
    }
    let mut doc = lab();
    doc["devices"][1]["ports"][0]["ipv4_acl"]["egress"] = json!(vec![rule; 64]);
    assert_eq!(report(&doc)["probes"][0]["outcome"]["status"], "reachable");
    doc["devices"][0]["port"]["ipv4_acl"] = json!({"egress":[]});
    assert!(run_lab(&doc.to_string()).is_err());
}

#[test]
fn archives_preserve_the_packet_decision_and_reject_inconsistent_facts() {
    for format in [2, 3] {
        let mut doc = lab();
        doc["format_version"] = json!(format);
        if format == 3 {
            doc["import_source"] = json!({
                "format":"containerlab", "source_name":"test.clab.yml",
                "gaps":[{"id":"unmapped","source_path":"/topology/nodes/r1/exec/0",
                    "scope":{"kind":"device","device_id":"r1"},"value":"unknown","message":"未映射配置"}]
            });
        }
        let ran = run_lab(&doc.to_string()).unwrap();
        let mut bytes = Vec::new();
        write_archive(&ran, None, None, &mut bytes).unwrap();
        assert_eq!(read_archive(bytes.as_slice()).unwrap().report, ran);
        let original: Value = serde_json::from_slice(&bytes).unwrap();
        if format == 3 {
            let report = &original["report"];
            assert_eq!(report["probes"][0]["outcome"]["status"], "undetermined");
            assert_eq!(
                report["probes"][0]["outcome"]["modeled"]["status"],
                "timeout"
            );
            assert_eq!(decision(report)["model_only"], true);
        }
        let index = original["report"]["evidence"]
            .as_array()
            .unwrap()
            .iter()
            .position(|e| e["fact"]["kind"] == "ipv4_acl_decision")
            .unwrap();
        for (field, bad) in [
            ("matched_rule/rule_number", json!(0)),
            ("matched_rule/network", json!("198.51.100.0")),
            ("source", json!("203.0.113.21")),
            ("event_id", json!("0")),
        ] {
            let mut archive = original.clone();
            *archive
                .pointer_mut(&format!("/report/evidence/{index}/fact/{field}"))
                .unwrap() = bad;
            assert!(read_archive(serde_json::to_vec(&archive).unwrap().as_slice()).is_err());
        }
        let mut archive = original;
        archive["report"]["report_version"] = json!(8);
        assert!(read_archive(serde_json::to_vec(&archive).unwrap().as_slice()).is_err());
    }
}

fn check_archive(report: &Value, trace: Option<&Value>, valid: bool) {
    let archive = json!({"archive_version":1, "report":report, "trace":trace});
    let bytes = serde_json::to_vec(&archive).unwrap();
    let typed_report = serde_json::from_value(report.clone()).unwrap();
    let typed_trace = trace.map(|value| serde_json::from_value(value.clone()).unwrap());
    for result in [
        read_archive(bytes.as_slice()).map(|_| ()),
        write_archive(&typed_report, typed_trace.as_ref(), None, std::io::sink()),
    ] {
        if valid {
            result.unwrap();
        } else {
            assert!(
                matches!(result, Err(reliner::ArchiveError::InvalidReport(_))),
                "{result:?}"
            );
        }
    }
}

#[test]
fn archive_rejects_delivery_from_a_denied_frame_but_accepts_a_distinct_copy() {
    let mut doc = lab();
    doc["devices"][1]["ports"][0]["ipv4_acl"]["egress"]
        .as_array_mut()
        .unwrap()
        .reverse();
    let mut ran = report(&doc);
    let rows = ran["evidence"].as_array_mut().unwrap();
    let acl = rows
        .iter_mut()
        .find(|e| e["fact"]["kind"] == "ipv4_acl_decision")
        .unwrap();
    acl["fact"]["action"] = json!("deny");
    let denied_frame = acl["frame"].clone();
    let reply = rows.last().unwrap();
    assert_eq!(reply["fact"]["kind"], "echo_reply_received");
    assert_eq!(reply["frame"]["parent_id"], denied_frame["id"]);
    check_archive(&ran, None, false);

    // A sibling can be delivered. Its parent need not have retained evidence, either.
    let last = ran["evidence"].as_array().unwrap().len() - 1;
    ran["evidence"][last]["frame"]["parent_id"] = denied_frame["parent_id"].clone();
    check_archive(&ran, None, true);
    ran["evidence"][last]["frame"]["parent_id"] = json!("5");
    check_archive(&ran, None, true);
    ran["evidence"][last]["frame"] = denied_frame;
    check_archive(&ran, None, false);
}

#[test]
fn archive_checks_the_order_of_acl_decisions_on_one_arrival() {
    let mut doc = lab();
    doc["devices"][1]["ports"][1]["ipv4_acl"] = json!({"ingress":[
        {"action":"permit","network":"0.0.0.0","prefix_length":0}
    ]});
    let ran = report(&doc);
    let ingress = ran["evidence"]
        .as_array()
        .unwrap()
        .iter()
        .position(|e| e["fact"]["direction"] == "ingress")
        .unwrap();
    let egress = ingress + 1;
    assert_eq!(
        ran["evidence"][ingress]["frame"],
        ran["evidence"][egress]["frame"]
    );
    assert_eq!(ran["evidence"][egress]["fact"]["direction"], "egress");
    check_archive(&ran, None, true);
    for (field, value) in [("action", "deny"), ("direction", "egress")] {
        let mut invalid = ran.clone();
        invalid["evidence"][ingress]["fact"][field] = json!(value);
        check_archive(&invalid, None, false);
    }
    let mut invalid = ran;
    invalid["evidence"][ingress]["at_ns"] = json!("701");
    check_archive(&invalid, None, false);
}

#[test]
fn archive_checks_children_retained_only_in_the_trace() {
    let mut doc = lab();
    doc["scenario"]["trace_limit"] = json!(64);
    let mut ran = report(&doc);
    let denied = decision(&ran)["frame"]["id"].clone();
    check_archive(&ran, None, true);
    let entries = ran["trace"]["entries"].as_array_mut().unwrap();
    let i = entries
        .iter()
        .position(|entry| entry["event_id"] == denied)
        .unwrap();
    let mut child = entries[i].clone();
    child["at_ns"] = json!("800");
    child["frame"]["parent_id"] = denied;
    child["frame"]["id"] = json!("10");
    child["event_id"] = json!("10");
    child["route"] = Value::Null;
    entries.insert(i + 1, child);
    check_archive(&ran, None, false);

    let detached = ran["trace"].clone();
    ran["trace"] = json!({"status":"evicted","entries":[]});
    check_archive(&ran, Some(&detached), false);
    let mut detached = detached;
    detached["entries"].as_array_mut().unwrap().remove(i + 1);
    check_archive(&ran, Some(&detached), true);
}

#[test]
fn archive_allows_only_trace_work_before_the_denying_stage() {
    let mut doc = lab();
    doc["scenario"]["trace_limit"] = json!(64);
    let ran = report(&doc);
    check_archive(&ran, None, true);
    let mut ingress = ran.clone();
    ingress["evidence"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|e| e["fact"]["kind"] == "ipv4_acl_decision")
        .unwrap()["fact"]["direction"] = json!("ingress");
    check_archive(&ingress, None, false);

    let event_id = decision(&ran)["fact"]["event_id"].clone();
    let mut egress = ran;
    let arrival = egress["trace"]["entries"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|entry| entry["event_id"] == event_id)
        .unwrap();
    arrival["route"]["ipv4_mtu"] = json!({
        "packet_kind":"echo_reply","ipv4_total_bytes":84,"mtu_bytes":1500,"passed":true
    });
    check_archive(&egress, None, false);
}

#[test]
fn archive_scopes_denied_frame_references_to_their_probe() {
    let mut doc = lab();
    let mut second = doc["scenario"]["probes"][0].clone();
    second["id"] = json!("second");
    doc["scenario"]["probes"]
        .as_array_mut()
        .unwrap()
        .push(second);
    let mut ran = report(&doc);
    let mut decisions = ran["evidence"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .filter(|e| e["fact"]["kind"] == "ipv4_acl_decision");
    let first = decisions.next().unwrap();
    let second = decisions.next().unwrap();
    assert_ne!(first["probe_id"], second["probe_id"]);
    second["frame"]["parent_id"] = first["frame"]["id"].clone();
    check_archive(&ran, None, true);
}
