use reliner::run_lab;
use serde_json::{Value, json};

fn lab() -> Value {
    serde_json::from_str(include_str!("../examples/ipv4-lab.json")).unwrap()
}

fn report(document: &Value) -> Value {
    serde_json::to_value(run_lab(&document.to_string()).unwrap()).unwrap()
}

fn fact<'a>(report: &'a Value, id: &Value) -> &'a Value {
    report["evidence"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| &item["id"] == id)
        .unwrap()
}

fn outage(document: &mut Value, down: &str, up: &str) {
    document["scenario"]["link_actions"] = json!([
        {"link": "r1-r2", "at_ns": down, "state": "down"},
        {"link": "r1-r2", "at_ns": up, "state": "up"}
    ]);
}

#[test]
fn missing_source_gateway_rejects_before_arp_and_closes_at_the_deadline() {
    let mut doc = lab();
    doc["devices"][0].as_object_mut().unwrap().remove("gateway");
    doc["scenario"]["probes"]
        .as_array_mut()
        .unwrap()
        .truncate(1);
    doc["scenario"]["warmup"] = json!([]);
    doc["scenario"]["trace_limit"] = json!(16);
    let run = run_lab(&doc.to_string()).unwrap();
    let result = serde_json::to_value(&run).unwrap();
    let probe = &result["probes"][0];
    let outcome = &probe["outcome"];
    assert_eq!(outcome["waiting_for"], "probe_deadline");
    assert_eq!(outcome["at_ns"], "1800");
    let cause = fact(&result, &outcome["cause"]);
    assert_eq!(cause["fact"]["kind"], "ipv4_route_miss");
    assert_eq!(cause["at_ns"], "1000");
    assert_eq!(cause["port"]["port_id"], "a-eth0");
    let events: Vec<_> = result["trace"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["event"].as_str().unwrap())
        .collect();
    assert_eq!(events, ["probe_start", "probe_deadline"]);
    let counts = run.probe_counts();
    assert_eq!(counts.rejected_before_send, 1);
    assert_eq!(counts.timed_out, 0);
    assert_eq!(result["model_version"], 14);
    assert_eq!(report(&lab())["model_version"], 14);
}

#[test]
fn immediate_link_drop_points_back_to_the_incoming_echo() {
    for (cold, kind) in [(false, "echo_request"), (true, "arp_request")] {
        let mut doc = lab();
        doc["scenario"]["link_actions"] = json!([{"link":"r1-r2","at_ns":"1050","state":"down"}]);
        if cold {
            doc["scenario"]["warmup"].as_array_mut().unwrap().remove(1);
        }
        let untraced = report(&doc);
        doc["scenario"]["trace_limit"] = json!(128);
        let result = report(&doc);
        let cause = fact(&result, &result["probes"][0]["outcome"]["cause"]);
        let parent = result["trace"]["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["at_ns"] == "1100" && e["port"]["port_id"] == "r1-lan")
            .unwrap();
        assert_eq!(parent["frame"]["kind"], "echo_request");
        assert_eq!(cause["fact"]["kind"], "link_down");
        assert_eq!(cause["frame"]["kind"], kind);
        assert_eq!(cause["frame"]["parent_id"], parent["frame"]["id"]);
        assert_ne!(cause["frame"]["id"], parent["frame"]["id"]);
        assert_eq!(untraced["evidence"], result["evidence"]);
        assert_eq!(result["model_version"], 14);
    }
}

#[test]
fn missing_return_gateway_keeps_request_evidence_and_waits_for_echo() {
    let mut doc = lab();
    doc["devices"][3].as_object_mut().unwrap().remove("gateway");
    let result = report(&doc);
    let probe = &result["probes"][0];
    let request = fact(&result, &probe["request_evidence_id"]);
    let loss = fact(&result, &probe["outcome"]["cause"]);
    assert_eq!(probe["outcome"]["waiting_for"], "echo_reply");
    assert_eq!(loss["fact"]["kind"], "ipv4_route_miss");
    assert_eq!(loss["fact"]["request_evidence_id"], request["id"]);
    assert_ne!(loss["id"], request["id"]);
    assert_eq!(loss["frame"]["kind"], "echo_reply");
    assert_eq!(loss["frame"]["parent_id"], request["frame"]["id"]);
    assert_ne!(loss["frame"]["id"], request["frame"]["id"]);
    assert_eq!(result["model_version"], 14);
}

#[test]
fn cold_static_ipv4_uses_interface_neighbors_and_independent_return_lookup() {
    let mut doc = lab();
    doc["scenario"]["warmup"] = json!([]);
    doc["scenario"]["probes"]
        .as_array_mut()
        .unwrap()
        .truncate(1);
    doc["scenario"]["probes"][0]["deadline_ns"] = json!("2600");
    let routed = report(&doc);
    assert_eq!(routed["probes"][0]["outcome"]["status"], "reachable");
    assert_eq!(routed["probes"][0]["outcome"]["at_ns"], "2200");

    doc["devices"][2]["routes"] = json!([]);
    let failed = report(&doc);
    let probe = &failed["probes"][0];
    let request = fact(&failed, &probe["request_evidence_id"]);
    let cause = fact(&failed, &probe["outcome"]["cause"]);
    assert_eq!(probe["outcome"]["status"], "timeout");
    assert_eq!(request["fact"]["kind"], "echo_request_received");
    assert_eq!(request["port"]["device_id"], "pc-b");
    assert_eq!(cause["port"]["device_id"], "r2");
    assert_eq!(cause["at_ns"], "2000");
    assert_eq!(cause["frame"]["kind"], "echo_reply");
    assert_eq!(cause["fact"]["kind"], "ipv4_route_miss");
    assert_eq!(cause["fact"]["destination"], "192.0.2.10");
    assert_eq!(cause["fact"]["request_evidence_id"], request["id"]);
}

#[test]
fn longest_prefix_beats_declaration_order_and_ttl_expires_at_router() {
    let mut doc = lab();
    doc["devices"][1]["routes"].as_array_mut().unwrap().insert(
        0,
        json!({
            "network": "0.0.0.0", "prefix_length": 0, "egress": "r1-lan", "next_hop": "192.0.2.10"
        }),
    );
    assert_eq!(report(&doc)["probes"][0]["outcome"]["status"], "reachable");

    doc["scenario"]["probes"][0]["ttl"] = json!(2);
    let result = report(&doc);
    let failed = &result["probes"][0];
    assert!(failed["request_evidence_id"].is_null());
    let cause = fact(&result, &failed["outcome"]["cause"]);
    assert_eq!(cause["port"]["device_id"], "r2");
    assert_eq!(cause["fact"]["kind"], "ipv4_ttl_expired");
    assert_eq!(cause["fact"]["received_ttl"], 1);
}

#[test]
fn four_routed_runs_keep_probe_identity_and_necessary_evidence_without_trace() {
    for interrupted in [false, true] {
        for missing_return in [false, true] {
            let mut doc = lab();
            if interrupted {
                outage(&mut doc, "1200", "1300");
            }
            if missing_return {
                doc["devices"][2]["routes"] = json!([]);
            }
            let result = report(&doc);
            assert_eq!(result, report(&doc));
            assert!(result["trace"]["entries"].as_array().unwrap().is_empty());
            assert_eq!(result["probes"][0]["icmp"]["sequence"], 1);
            assert_eq!(result["probes"][1]["icmp"]["sequence"], 2);
            for index in 0..2 {
                let probe = &result["probes"][index];
                let outcome = &probe["outcome"];
                if index == 0 && interrupted {
                    let cause = fact(&result, &outcome["cause"]);
                    assert_eq!(outcome["status"], "timeout");
                    assert!(probe["request_evidence_id"].is_null());
                    assert_eq!(cause["frame"]["kind"], "echo_request");
                    assert_eq!(cause["fact"]["kind"], "in_flight_link_loss");
                    assert_eq!(cause["at_ns"], "1200");
                    assert_eq!(cause["fact"]["planned_arrival_ns"], "1200");
                    assert_eq!(cause["fact"]["sent_generation"], "0");
                    assert_eq!(
                        cause["fact"]["invalidated_by"],
                        result["link_events"][0]["event_id"]
                    );
                } else if missing_return {
                    let cause = fact(&result, &outcome["cause"]);
                    let request = fact(&result, &probe["request_evidence_id"]);
                    assert_eq!(outcome["status"], "timeout");
                    assert_eq!(cause["fact"]["request_evidence_id"], request["id"]);
                    assert_eq!(cause["fact"]["kind"], "ipv4_route_miss");
                    assert_eq!(cause["port"]["device_id"], "r2");
                    assert_eq!(request["port"]["device_id"], "pc-b");
                } else {
                    assert_eq!(outcome["status"], "reachable");
                    assert_eq!(outcome["at_ns"], if index == 0 { "1600" } else { "2600" });
                }
            }
        }
    }
}

#[test]
fn old_frame_stays_lost_when_link_recovers_before_its_arrival() {
    let mut doc = lab();
    outage(&mut doc, "1120", "1140");
    doc["scenario"]["link_actions"]
        .as_array_mut()
        .unwrap()
        .extend([
            json!({"link": "r1-r2", "at_ns": "1160", "state": "down"}),
            json!({"link": "r1-r2", "at_ns": "1180", "state": "up"}),
        ]);
    let result = report(&doc);
    let first = &result["probes"][0]["outcome"];
    assert_eq!(first["status"], "timeout");
    let cause = fact(&result, &first["cause"]);
    assert_eq!(
        cause["fact"]["invalidated_by"],
        result["link_events"][0]["event_id"]
    );
    assert_eq!(result["probes"][1]["outcome"]["status"], "reachable");
}

#[test]
fn trace_limit_does_not_change_outcomes_or_evidence_ids() {
    let mut doc = lab();
    outage(&mut doc, "1200", "1300");
    let off = report(&doc);
    doc["scenario"]["trace_limit"] = json!(12);
    let on = report(&doc);
    assert_eq!(off["probes"], on["probes"]);
    assert_eq!(off["evidence"], on["evidence"]);
    assert_eq!(off["link_events"], on["link_events"]);
    assert_eq!(on["trace"]["entries"].as_array().unwrap().len(), 12);
    assert_eq!(on["trace"]["status"], "truncated");
}

#[test]
fn invalid_static_next_hop_and_ambiguous_link_actions_are_rejected() {
    let mut doc = lab();
    doc["devices"][1]["routes"][0]["next_hop"] = json!("203.0.113.20");
    assert!(
        run_lab(&doc.to_string())
            .unwrap_err()
            .to_string()
            .contains("下一跳")
    );

    let mut doc = lab();
    outage(&mut doc, "1200", "1200");
    assert!(
        run_lab(&doc.to_string())
            .unwrap_err()
            .to_string()
            .contains("同一时刻")
    );
}

#[test]
fn expired_and_late_arp_events_obey_their_boundaries() {
    let mut doc = lab();
    doc["scenario"]["arp_lifetime_ns"] = json!("1000");
    doc["scenario"]["trace_limit"] = json!(100);
    doc["scenario"]["warmup"]
        .as_array_mut()
        .unwrap()
        .push(json!({
            "at_ns": "400", "port": "a-eth0", "neighbor": "192.0.2.1"
        }));
    doc["scenario"]["probes"][0]["at_ns"] = json!("1200");
    doc["scenario"]["probes"][0]["deadline_ns"] = json!("2800");
    let result = report(&doc);
    let timeline = result["trace"]["entries"].as_array().unwrap();
    let at_first: Vec<_> = timeline
        .iter()
        .filter(|event| event["probe_id"] == "first")
        .collect();
    assert_eq!(at_first[0]["event"], "probe_start");
    assert_eq!(at_first[1]["frame"]["kind"], "echo_request");
    assert_eq!(at_first[1]["at_ns"], "1300");
    assert!(
        timeline
            .iter()
            .any(|event| event["at_ns"] == "1200" && event["event"] == "table_expiry")
    );

    let mut doc = lab();
    doc["scenario"]["warmup"] = json!([]);
    doc["scenario"]["trace_limit"] = json!(100);
    doc["scenario"]["probes"][0]["deadline_ns"] = json!("1200");
    doc["scenario"]["probes"][1]["at_ns"] = json!("1200");
    let result = report(&doc);
    assert_eq!(result["probes"][0]["outcome"]["waiting_for"], "arp_reply");
    assert_eq!(result["probes"][1]["outcome"]["status"], "reachable");
    let at_cutoff: Vec<_> = result["trace"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|event| event["at_ns"] == "1200")
        .map(|event| event["event"].as_str().unwrap())
        .collect();
    assert_eq!(
        at_cutoff,
        ["probe_deadline", "frame_arrival", "probe_start"]
    );
    assert!(
        !result["trace"]["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|event| event["probe_id"] == "first" && event["frame"]["kind"] == "echo_request")
    );
}

#[test]
fn a_hard_stop_preserves_completed_probes_and_ignores_trace_capacity() {
    use reliner::{RunLimits, run_lab_with_limits};
    let mut doc = lab();
    let settings = RunLimits {
        total_work: 64,
        ..RunLimits::default()
    };
    let run = |doc: &Value, limits| {
        serde_json::to_value(run_lab_with_limits(&doc.to_string(), limits).unwrap()).unwrap()
    };
    let off = run(&doc, settings);
    doc["scenario"]["trace_limit"] = json!(100);
    let on = run(&doc, settings);
    let no_trace_capacity = run(
        &doc,
        RunLimits {
            trace_entries: 0,
            ..settings
        },
    );
    assert_eq!(off["probes"][0]["outcome"]["status"], "reachable");
    assert_eq!(off["probes"][1]["outcome"]["status"], "incomplete");
    assert_eq!(
        off["stop_reason"],
        json!({"resource_limit":{"limit":"total_work"}})
    );
    assert_eq!(off["probes"], on["probes"]);
    assert_eq!(off["evidence"], on["evidence"]);
    assert_eq!(off["last_event_id"], on["last_event_id"]);
    assert_eq!(off["last_event_id"], no_trace_capacity["last_event_id"]);
    assert_eq!(no_trace_capacity["trace"]["status"], "truncated");
}

#[test]
fn full_evidence_storage_stops_before_the_reply_event_commits() {
    use reliner::{RunLimits, run_lab_with_limits};
    let limited = run_lab_with_limits(
        &lab().to_string(),
        RunLimits {
            essential_facts: 1,
            ..RunLimits::default()
        },
    )
    .unwrap();
    let result = serde_json::to_value(limited).unwrap();
    assert_eq!(result["last_event_ns"], "1500");
    assert_eq!(
        result["stop_reason"],
        json!({"resource_limit":{"limit":"essential_facts"}})
    );
    assert_eq!(result["evidence"].as_array().unwrap().len(), 1);
    assert_eq!(result["probes"][0]["outcome"]["status"], "incomplete");
    assert_eq!(
        result["probes"][0]["request_evidence_id"],
        result["evidence"][0]["id"]
    );
    assert_eq!(
        result["evidence"][0]["fact"]["kind"],
        "echo_request_received"
    );
}

#[test]
fn an_already_recorded_arp_cause_does_not_charge_a_second_blocking_candidate() {
    let mut doc = lab();
    doc["links"][0]["delay_ns"] = json!("1");
    doc["links"].as_array_mut().unwrap().push(json!({
        "id":"return-link","ports":["r1-return","r2-return"],"delay_ns":"1"
    }));
    for (index, port, ip, mac) in [
        (1, "r1-return", "198.18.0.1", "02:00:00:00:01:fe"),
        (2, "r2-return", "198.18.0.2", "02:00:00:00:02:fe"),
    ] {
        doc["devices"][index]["ports"]
            .as_array_mut()
            .unwrap()
            .push(json!({
                "id":port,"name":port,"ipv4":ip,"prefix_length":30,"mac":mac
            }));
    }
    doc["devices"][2]["routes"]
        .as_array_mut()
        .unwrap()
        .push(json!({
            "network":"203.0.113.20","prefix_length":32,"egress":"r2-return","next_hop":"198.18.0.1"
        }));
    doc["scenario"]["end_ns"] = json!("450");
    doc["scenario"]["arp_lifetime_ns"] = json!("30");
    doc["scenario"]["trace_limit"] = json!(128);
    doc["scenario"]["warmup"] = json!([
        {"at_ns":"196","port":"a-eth0","neighbor":"192.0.2.1"},
        {"at_ns":"102","port":"r2-transit","neighbor":"198.51.100.1"},
        {"at_ns":"295","port":"r2-return","neighbor":"198.18.0.1"}
    ]);
    doc["scenario"]["link_actions"] = json!([{"link":"r1-r2","at_ns":"350","state":"down"}]);
    doc["scenario"]["probes"] = json!([
        {"id":"looped","source":"pc-a","destination":"pc-b","at_ns":"200","deadline_ns":"450"}
    ]);

    let reference = report(&doc);
    let probe = &reference["probes"][0];
    assert_eq!(probe["outcome"]["status"], "timeout");
    assert_eq!(fact(&reference, &probe["outcome"]["cause"])["at_ns"], "401");
    assert_eq!(reference["evidence"].as_array().unwrap().len(), 1);
    assert!(
        reference["trace"]["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["at_ns"] == "403"
                && row["event"] == "frame_arrival"
                && row["frame"]["kind"] == "arp_request")
    );

    let run = |essential_facts| {
        serde_json::to_value(
            reliner::run_lab_with_limits(
                &doc.to_string(),
                reliner::RunLimits {
                    essential_facts,
                    ..Default::default()
                },
            )
            .unwrap(),
        )
        .unwrap()
    };
    let below = run(1);
    assert_eq!(below["last_event_ns"], "350");
    assert_eq!(below["probes"][0]["outcome"]["status"], "incomplete");
    let exact = run(2);
    assert!(exact["stop_reason"].is_null());
    for key in [
        "probes",
        "evidence",
        "trace",
        "link_events",
        "last_event_id",
    ] {
        assert_eq!(exact[key], reference[key], "{key}");
    }
}
