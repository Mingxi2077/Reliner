use reliner::run_lab;
use serde_json::{Value, json};

const CONGESTION: &str = include_str!("../examples/congestion-lab.json");

fn lab() -> Value {
    serde_json::from_str(CONGESTION).unwrap()
}

fn report(doc: &Value) -> Value {
    serde_json::to_value(run_lab(&doc.to_string()).unwrap()).unwrap()
}

fn cause(result: &Value, index: usize) -> &Value {
    let probe = &result["probes"][index];
    assert_eq!(probe["outcome"]["status"], "timeout");
    result["evidence"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == probe["outcome"]["cause"] && item["probe_id"] == probe["id"])
        .unwrap()
}

#[test]
fn burst_drops_at_transit_egress_and_a_second_waiting_slot_repairs_it() {
    let mut doc = lab();
    let failed = report(&doc);
    assert_eq!(failed["report_version"], 5);
    for (index, at) in ["4232", "7232", "8048"].into_iter().enumerate() {
        assert_eq!(failed["probes"][index]["outcome"]["status"], "reachable");
        assert_eq!(failed["probes"][index]["outcome"]["at_ns"], at);
    }
    let drop = cause(&failed, 3);
    assert_eq!(drop["at_ns"], "5100");
    assert_eq!(drop["port"]["port_id"], "r1-transit");
    assert_eq!(drop["frame"]["kind"], "echo_request");
    assert_eq!(drop["fact"]["kind"], "transmit_queue_full");
    assert_eq!(drop["fact"]["link_id"], "r1-r2");
    assert_eq!(drop["fact"]["frame_bytes"], 102);
    assert_eq!(drop["fact"]["queue_frames"], 1);
    assert_eq!(failed["model_version"], 14);
    assert_eq!(drop["frame"]["parent_id"], drop["fact"]["event_id"]);
    assert_ne!(drop["frame"]["id"], drop["frame"]["parent_id"]);

    doc["scenario"]["trace_limit"] = json!(1);
    let truncated = report(&doc);
    assert_eq!(truncated["trace"]["status"], "truncated");
    assert_eq!(truncated["probes"], failed["probes"]);
    assert_eq!(truncated["evidence"], failed["evidence"]);

    doc["links"][1]["transmission"]["queue_frames"] = json!(2);
    let fixed = report(&doc);
    assert_eq!(fixed["probes"][3]["outcome"]["status"], "reachable");
    assert_eq!(fixed["probes"][3]["outcome"]["at_ns"], "8864");
}

#[test]
fn queued_slots_release_at_start_and_directions_are_independent() {
    let mut doc = lab();
    doc["scenario"]["probes"][3]["at_ns"] = json!("5815");
    let mut at_release = doc["scenario"]["probes"][3].clone();
    at_release["id"] = json!("at-release");
    at_release["at_ns"] = json!("5816");
    doc["scenario"]["probes"]
        .as_array_mut()
        .unwrap()
        .push(at_release);
    let result = report(&doc);
    assert_eq!(cause(&result, 3)["at_ns"], "5915");
    assert_eq!(result["probes"][4]["outcome"]["at_ns"], "8864");
    assert_eq!(result["probes"][4]["outcome"]["status"], "reachable");

    doc["links"][1]["transmission"]["queue_frames"] = json!(0);
    doc["scenario"]["probes"]
        .as_array_mut()
        .unwrap()
        .truncate(3);
    let reverse = &mut doc["scenario"]["probes"][2];
    reverse["source"] = json!("pc-b");
    reverse["destination"] = json!("pc-a");
    let duplex = report(&doc);
    for probe in &duplex["probes"].as_array().unwrap()[1..] {
        assert_eq!(probe["outcome"]["status"], "reachable");
        assert_eq!(probe["outcome"]["at_ns"], "7232");
    }
}

#[test]
fn link_change_at_a_queued_start_does_not_debit_the_recovered_direction() {
    let mut doc = lab();
    doc["scenario"]["trace_limit"] = json!(128);
    doc["scenario"]["link_actions"] = json!([
        {"link":"r1-r2", "state":"down", "at_ns":"5916"},
        {"link":"r1-r2", "state":"up", "at_ns":"6000"}
    ]);
    doc["scenario"]["probes"][3]["at_ns"] = json!("5900");
    let result = report(&doc);
    for (index, planned_at) in [(1, "6016"), (2, "6832")] {
        let evidence = cause(&result, index);
        assert_eq!(evidence["fact"]["kind"], "in_flight_link_loss");
        assert_eq!(evidence["fact"]["planned_arrival_ns"], planned_at);
        assert_eq!(
            evidence["fact"]["invalidated_by"],
            result["link_events"][0]["event_id"]
        );
    }
    assert!(
        result["trace"]["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| { entry["event"] == "transmit_cancelled" && entry["at_ns"] == "5916" })
    );
    assert_eq!(result["probes"][3]["outcome"]["status"], "reachable");
    assert_eq!(result["probes"][3]["outcome"]["at_ns"], "8132");
}

#[test]
fn queued_arrival_ties_use_admission_order_at_a_shared_egress() {
    let mut doc = lab();
    doc["links"][0]["delay_ns"] = json!("1000");
    doc["links"][0]["transmission"] = doc["links"][1]["transmission"].clone();
    doc["links"][1]["transmission"]["queue_frames"] = json!(0);
    doc["devices"].as_array_mut().unwrap().push(json!({
        "kind":"host", "id":"pc-c", "name":"PC-C", "ipv4":"172.16.0.2",
        "prefix_length":24, "gateway":"172.16.0.1", "mac":"02:00:00:00:00:0c",
        "port":{"id":"c-eth0", "name":"eth0"}
    }));
    doc["devices"][1]["ports"]
        .as_array_mut()
        .unwrap()
        .push(json!({
            "id":"r1-extra", "name":"Extra", "ipv4":"172.16.0.1",
            "prefix_length":24, "mac":"02:00:00:00:01:03"
        }));
    doc["devices"][2]["routes"]
        .as_array_mut()
        .unwrap()
        .push(json!({
            "network":"172.16.0.0", "prefix_length":24, "egress":"r2-transit",
            "next_hop":"198.51.100.1"
        }));
    doc["links"].as_array_mut().unwrap().push(json!({
        "id":"c-r1", "ports":["c-eth0", "r1-extra"], "delay_ns":"2132"
    }));
    doc["scenario"]["warmup"]
        .as_array_mut()
        .unwrap()
        .push(json!({
            "at_ns":"0", "port":"c-eth0", "neighbor":"172.16.0.1"
        }));
    doc["scenario"]["end_ns"] = json!("20000");
    let probes = doc["scenario"]["probes"].as_array_mut().unwrap();
    probes.remove(0);
    for probe in probes.iter_mut() {
        probe["deadline_ns"] = json!("20000");
    }
    probes[2]["source"] = json!("pc-c");
    probes[2]["at_ns"] = json!("5500");
    let result = report(&doc);
    assert_eq!(result["probes"][1]["outcome"]["status"], "reachable");
    assert_eq!(cause(&result, 2)["at_ns"], "7632");
    assert_eq!(cause(&result, 2)["port"]["port_id"], "r1-transit");
}

fn cold_direct_lab() -> Value {
    json!({
        "format_version":2, "id":"cold-direct", "revision":"1",
        "devices":[
            {"kind":"host", "id":"a", "name":"A", "ipv4":"192.0.2.1", "prefix_length":24,
             "mac":"02:00:00:00:00:01", "port":{"id":"a-eth0", "name":"eth0"}},
            {"kind":"host", "id":"b", "name":"B", "ipv4":"192.0.2.2", "prefix_length":24,
             "mac":"02:00:00:00:00:02", "port":{"id":"b-eth0", "name":"eth0"}}
        ],
        "links":[{"id":"ab", "ports":["a-eth0", "b-eth0"], "delay_ns":"1000",
                  "transmission":{"rate_bps":"1000000000", "queue_frames":2}}],
        "scenario":{
            "end_ns":"10000", "arp_lifetime_ns":"1000000", "mac_lifetime_ns":"1000000",
            "probes":[
                {"id":"one", "source":"a", "destination":"b", "at_ns":"0", "deadline_ns":"9000"},
                {"id":"two", "source":"a", "destination":"b", "at_ns":"0", "deadline_ns":"9000", "payload_bytes":0},
                {"id":"three", "source":"a", "destination":"b", "at_ns":"0", "deadline_ns":"9000"}
            ]
        }
    })
}

#[test]
fn a_single_arp_event_accounts_for_every_release_and_clears_a_resolved_drop() {
    let mut doc = cold_direct_lab();
    doc["scenario"]["warmup"] = json!([{"at_ns":"488", "port":"b-eth0", "neighbor":"192.0.2.1"}]);
    let result = report(&doc);
    assert_eq!(cause(&result, 2)["at_ns"], "2000");
    assert_eq!(cause(&result, 2)["frame"]["kind"], "echo_request");
    assert_eq!(result["probes"][0]["outcome"]["at_ns"], "6144");
    assert_eq!(result["probes"][1]["outcome"]["at_ns"], "6656");

    doc["links"][0]["transmission"]["queue_frames"] = json!(1);
    let narrower = report(&doc);
    // An earlier redundant ARP drop is superseded by the actual Echo drop.
    assert_eq!(cause(&narrower, 2)["at_ns"], "2000");
    assert_eq!(cause(&narrower, 2)["frame"]["kind"], "echo_request");

    doc["links"][0]["transmission"]["queue_frames"] = json!(0);
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
    let below = run(7);
    assert_eq!(
        below["stop_reason"]["resource_limit"]["limit"],
        "essential_facts"
    );
    assert_eq!(below["evidence"].as_array().unwrap().len(), 2);
    let exact = run(8);
    assert!(exact["stop_reason"].is_null());
    assert_eq!(exact["evidence"].as_array().unwrap().len(), 8);
    for index in 0..3 {
        assert_eq!(cause(&exact, index)["at_ns"], "2000");
        assert_eq!(cause(&exact, index)["frame"]["kind"], "echo_request");
    }
}

#[test]
fn reservations_survive_the_horizon_and_obey_state_preflight() {
    let mut doc = cold_direct_lab();
    doc["links"][0]["transmission"]["queue_frames"] = json!(0);
    doc["scenario"]["end_ns"] = json!("100");
    for probe in doc["scenario"]["probes"].as_array_mut().unwrap() {
        probe["deadline_ns"] = json!("100");
    }
    let result = report(&doc);
    let drop = cause(&result, 1);
    assert_eq!(drop["fact"]["kind"], "transmit_queue_full");
    assert_eq!(drop["fact"]["frame_bytes"], 64);
    assert_eq!(drop["frame"]["kind"], "arp_request");

    let limits = reliner::RunLimits {
        state_entries: 1,
        ..Default::default()
    };
    let limited = reliner::run_lab_with_limits(&doc.to_string(), limits).unwrap();
    let limited = serde_json::to_value(limited).unwrap();
    assert_eq!(
        limited["stop_reason"]["resource_limit"]["limit"],
        "state_entries"
    );
    assert!(limited["last_event_id"].is_null());
    assert!(limited["evidence"].as_array().unwrap().is_empty());
}

#[test]
fn rated_standby_invalidates_its_queue_and_reopens_with_tagged_frame_timing() {
    let mut doc: Value =
        serde_json::from_str(include_str!("../examples/failover-lab.json")).unwrap();
    doc["links"][2]["transmission"] = json!({"rate_bps": "3000000000", "queue_frames": 1});
    doc["devices"][2]["ports"][2]["mode"]["allowed_vlans"] = json!([10, 20]);
    doc["scenario"]["link_actions"] = json!([
        {"link":"inter-switch-primary", "state":"down", "at_ns":"2000"},
        {"link":"inter-switch-primary", "state":"up", "at_ns":"2400"},
        {"link":"inter-switch-primary", "state":"down", "at_ns":"2450"}
    ]);
    doc["scenario"]["probes"] = json!([
        {"id":"first", "source":"pc-a", "destination":"pc-b", "at_ns":"2100", "deadline_ns":"4600"},
        {"id":"queued", "source":"pc-a", "destination":"pc-b", "at_ns":"2100", "deadline_ns":"4600"},
        {"id":"new-generation", "source":"pc-a", "destination":"pc-b", "at_ns":"2350", "deadline_ns":"4600"}
    ]);
    let result = report(&doc);
    for (index, arrival) in [(0, "2683"), (1, "2966")] {
        let evidence = cause(&result, index);
        assert_eq!(evidence["fact"]["kind"], "in_flight_protection_loss");
        assert_eq!(evidence["fact"]["planned_arrival_ns"], arrival);
        assert_eq!(
            evidence["fact"]["invalidated_by"],
            result["link_events"][1]["event_id"]
        );
    }
    assert_eq!(result["probes"][2]["outcome"]["status"], "reachable");
    assert_eq!(result["probes"][2]["outcome"]["at_ns"], "3716");
}
