use reliner::{
    Outcome, ResourceLimit, RunLimits, RunReport, StopReason, run_lab, run_lab_with_limits,
};
use serde_json::{Value, json};

fn lab() -> Value {
    serde_json::from_str(include_str!("../examples/routing-lab.json")).unwrap()
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
        .find(|e| e["id"] == probe["outcome"]["cause"] && e["probe_id"] == probe["id"])
        .unwrap()
}

fn selected_at_r1<'a>(result: &'a Value, probe: &str) -> &'a Value {
    &result["trace"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|event| {
            event["probe_id"] == probe
                && event["frame"]["kind"] == "echo_request"
                && event["port"]["device_id"] == "r1"
                && event["route"].is_object()
        })
        .unwrap()["route"]
}

#[test]
fn publishes_the_return_prefix_and_keeps_failure_and_recovery_timing() {
    let mut doc = lab();
    let broken = report(&doc);
    assert_eq!(broken["report_version"], 6);
    assert_eq!(broken["model_version"], 10);
    assert_eq!(
        cause(&broken, 0)["fact"]["reason"],
        "awaiting_initial_convergence"
    );
    let missing = cause(&broken, 1);
    assert_eq!(missing["port"]["port_id"], "r3-lan");
    assert_eq!(missing["frame"]["kind"], "echo_reply");
    assert_eq!(missing["fact"]["kind"], "dynamic_route_miss");
    assert_eq!(missing["fact"]["reason"], "no_advertised_prefix");
    assert_eq!(
        missing["fact"]["request_evidence_id"],
        broken["probes"][1]["request_evidence_id"]
    );
    assert_eq!(cause(&broken, 2)["fact"]["kind"], "link_down");
    assert_eq!(
        selected_at_r1(&broken, "during-update")["egress_port_id"],
        "r1-r3"
    );
    assert_eq!(selected_at_r1(&broken, "detour")["egress_port_id"], "r1-r2");
    assert_eq!(selected_at_r1(&broken, "detour")["learned"]["cost"], "20");
    assert_eq!(
        selected_at_r1(&broken, "recovered")["egress_port_id"],
        "r1-r3"
    );
    let rounds: Vec<_> = broken["routing_events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|event| event["router_id"] == "r1")
        .map(|event| event["at_ns"].as_str().unwrap())
        .collect();
    assert_eq!(rounds, ["500", "2900", "4900"]);

    doc["devices"][1]["link_state"]["advertise"] = json!(["r1-lan"]);
    let fixed = report(&doc);
    for (index, ns) in [(1, "1600"), (3, "3800"), (4, "5600")] {
        assert_eq!(fixed["probes"][index]["outcome"]["status"], "reachable");
        assert_eq!(fixed["probes"][index]["outcome"]["at_ns"], ns);
    }
    doc["scenario"]["trace_limit"] = json!(0);
    let untraced = report(&doc);
    assert_eq!(untraced["routing_events"], fixed["routing_events"]);
    assert_eq!(untraced["probes"], fixed["probes"]);
    assert_eq!(untraced["evidence"], fixed["evidence"]);
}

#[test]
fn weighted_ties_are_stable_and_longest_prefix_beats_static_default() {
    let mut doc = lab();
    doc["devices"][1]["link_state"]["peers"][0]["cost"] = json!(20);
    let tied = report(&doc);
    assert_eq!(selected_at_r1(&tied, "primary")["egress_port_id"], "r1-r2");
    doc["devices"].as_array_mut().unwrap().reverse();
    doc["links"].as_array_mut().unwrap().reverse();
    let reversed = report(&doc);
    assert_eq!(
        selected_at_r1(&reversed, "primary"),
        selected_at_r1(&tied, "primary")
    );
    assert_eq!(reversed["routing_events"], tied["routing_events"]);
    doc["devices"].as_array_mut().unwrap().reverse();

    doc["devices"][1]["link_state"]["peers"][0]["cost"] = json!(5);
    doc["devices"][1]["routes"] = json!([{
        "network":"0.0.0.0", "prefix_length":0, "egress":"r1-r2", "next_hop":"198.51.100.6"
    }]);
    assert_eq!(
        selected_at_r1(&report(&doc), "primary")["egress_port_id"],
        "r1-r3"
    );
    doc["devices"][1]["routes"][0]["network"] = json!("203.0.113.0");
    doc["devices"][1]["routes"][0]["prefix_length"] = json!(24);
    let static_wins = report(&doc);
    let selected = selected_at_r1(&static_wins, "primary");
    assert_eq!(selected["egress_port_id"], "r1-r2");
    assert!(selected["learned"].is_null());
}

#[test]
fn missing_peer_enablement_leaves_the_physical_link_out_of_the_spf_graph() {
    let mut doc = lab();
    doc["devices"][3]["link_state"]["peers"]
        .as_array_mut()
        .unwrap()
        .remove(0);
    let result = report(&doc);
    assert_eq!(
        result["settings"]["routing"]["adjacency_issues"],
        json!([{
            "link_id":"r1-r3", "port_id":"r1-r3", "peer_port_id":"r3-r1", "reason":"peer_disabled"
        }])
    );
    assert_eq!(
        selected_at_r1(&result, "primary")["egress_port_id"],
        "r1-r2"
    );
    assert_eq!(result["routing_events"].as_array().unwrap().len(), 3);
}

#[test]
fn link_change_on_the_control_boundary_is_included_before_frame_arrival() {
    let mut doc = lab();
    doc["scenario"]["link_actions"][1] = json!({"link":"r2-r3", "at_ns":"2900", "state":"down"});
    doc["scenario"]["probes"][2]["at_ns"] = json!("2800");
    let result = report(&doc);
    let missing = cause(&result, 2);
    assert_eq!(missing["at_ns"], "2900");
    assert_eq!(missing["fact"]["reason"], "no_path_to_advertisement");
    let convergence = result["routing_events"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["event_id"] == missing["fact"]["convergence_event_id"])
        .unwrap();
    assert_eq!(convergence["at_ns"], "2900");
    assert_eq!(convergence["router_id"], "r1");
    assert_eq!(result["routing_events"].as_array().unwrap().len(), 6);
}

#[test]
fn a_withdrawn_covering_advertisement_survives_a_later_narrow_prefix() {
    for (covering, nested) in [(24, 30), (0, 32)] {
        let mut doc = lab();
        doc["scenario"]["warmup"] = json!([]);
        doc["scenario"]["link_actions"] = json!([
            {"link":"r3-b", "at_ns":"0", "state":"down"},
            {"link":"r2-r3", "at_ns":"0", "state":"down"}
        ]);
        doc["devices"][2]["ports"][1]["ipv4"] = json!("203.0.113.14");
        let advertiser = &mut doc["devices"][3];
        advertiser["ports"][0]["prefix_length"] = json!(covering);
        advertiser["ports"][2]["ipv4"] = json!("203.0.113.13");
        advertiser["ports"][2]["prefix_length"] = json!(nested);
        advertiser["link_state"]["advertise"] = json!(["r3-lan", "r3-r2"]);

        let result = report(&doc);
        let missing = cause(&result, 1);
        assert_eq!(result["routing_events"][0]["installed_routes"], 0);
        assert_eq!(missing["frame"]["kind"], "echo_request");
        assert_eq!(missing["fact"]["reason"], "no_path_to_advertisement");
        assert_eq!(
            missing["fact"]["convergence_event_id"],
            result["routing_events"][0]["event_id"]
        );
    }
}

#[test]
fn convergence_evidence_is_reserved_before_a_router_table_commits() {
    let mut doc = lab();
    doc["scenario"]["warmup"] = json!([]);
    doc["scenario"]["probes"].as_array_mut().unwrap().remove(0);
    let limited = run_lab_with_limits(
        &doc.to_string(),
        RunLimits {
            essential_facts: 1,
            ..Default::default()
        },
    )
    .unwrap();
    let result = serde_json::to_value(limited).unwrap();
    assert_eq!(
        result["stop_reason"],
        json!({"resource_limit":{"limit":"essential_facts"}})
    );
    assert_eq!(result["routing_events"].as_array().unwrap().len(), 1);
    assert_eq!(
        result["last_event_id"],
        result["routing_events"][0]["event_id"]
    );
    assert_eq!(result["routing_events"][0]["router_id"], "r1");
    assert_eq!(result["probes"][0]["outcome"]["status"], "incomplete");
}

#[test]
fn cumulative_work_stops_at_the_last_committed_router_in_a_round() {
    let mut doc = lab();
    doc["scenario"]["warmup"] = json!([]);
    doc["scenario"]["probes"].as_array_mut().unwrap().remove(0);
    let input = doc.to_string();
    // A triangle, three routers and one advertised prefix require 34 units per install.
    let run = |total_work, event_work| {
        let limits = RunLimits {
            event_work,
            total_work,
            ..Default::default()
        };
        let RunReport::Scenario(report) = run_lab_with_limits(&input, limits).unwrap() else {
            unreachable!("the fixture uses format 2")
        };
        report
    };
    let defaults = RunLimits::default();
    let full = run(defaults.total_work, defaults.event_work);
    for (budget, installed) in [(33, 0), (34, 1), (67, 1), (68, 2)] {
        let limited = run(budget, 34);
        assert_eq!(
            limited.stop_reason,
            Some(StopReason::ResourceLimit {
                limit: ResourceLimit::TotalWork
            })
        );
        assert_eq!(limited.routing_events, full.routing_events[..installed]);
        let last = limited.routing_events.last();
        assert_eq!(limited.last_event_id, last.map(|event| event.event_id));
        assert_eq!(limited.last_event_ns, last.map_or(0, |event| event.at_ns));
        assert_eq!(limited.trace.entries, full.trace.entries[..installed]);
        assert!(matches!(
            limited.probes[0].outcome,
            Outcome::Incomplete { .. }
        ));
    }
    assert_eq!(
        run(33, 33).stop_reason,
        Some(StopReason::ResourceLimit {
            limit: ResourceLimit::EventWork
        })
    );
}

#[test]
fn a_down_connected_route_does_not_hide_a_learned_path_to_the_same_network() {
    let mut doc = lab();
    doc["devices"][1]["link_state"]["advertise"] = json!(["r1-lan", "r1-b-lan"]);
    doc["devices"][1]["ports"]
        .as_array_mut()
        .unwrap()
        .push(json!({
            "id":"r1-b-lan", "name":"Local B", "ipv4":"203.0.113.2",
            "prefix_length":24, "mac":"02:00:00:00:01:04"
        }));
    doc["devices"].as_array_mut().unwrap().push(json!({
        "kind":"switch", "id":"sw-b", "name":"SW-B", "ports":[
            {"id":"sw-b-r1", "name":"R1", "mode":{"kind":"access", "vlan":10}},
            {"id":"sw-b-r3", "name":"R3", "mode":{"kind":"access", "vlan":10}},
            {"id":"sw-b-host", "name":"Host", "mode":{"kind":"access", "vlan":10}}
        ]
    }));
    doc["links"][4]["ports"] = json!(["r3-lan", "sw-b-r3"]);
    doc["links"].as_array_mut().unwrap().extend([
        json!({"id":"r1-b-local", "ports":["r1-b-lan","sw-b-r1"], "delay_ns":"100"}),
        json!({"id":"b-access", "ports":["sw-b-host","b-eth0"], "delay_ns":"100"}),
    ]);
    for change in doc["scenario"]["link_actions"].as_array_mut().unwrap() {
        change["link"] = json!("r1-b-local");
    }
    let result = report(&doc);
    assert_eq!(
        selected_at_r1(&result, "primary")["egress_port_id"],
        "r1-b-lan"
    );
    assert_eq!(selected_at_r1(&result, "detour")["egress_port_id"], "r1-r3");
    assert_eq!(result["probes"][3]["outcome"]["status"], "reachable");
}

#[test]
fn routing_replaces_equal_sized_tables_under_a_tight_resident_limit() {
    let mut doc = lab();
    doc["scenario"]["warmup"] = json!([]);
    doc["devices"][0]["port"]["ipv4_mtu_bytes"] = json!(68);
    let run = run_lab_with_limits(
        &doc.to_string(),
        RunLimits {
            state_entries: 2,
            ..Default::default()
        },
    )
    .unwrap();
    let result = serde_json::to_value(run).unwrap();
    assert!(result["stop_reason"].is_null());
    assert_eq!(result["routing_events"].as_array().unwrap().len(), 9);
}

#[test]
fn link_changes_inside_the_horizon_commit_without_scheduling_past_its_end() {
    let mut doc = lab();
    let near_end = (u64::MAX - 100).to_string();
    doc["scenario"]["end_ns"] = json!(u64::MAX.to_string());
    doc["scenario"]["link_actions"] = json!([{"link":"r1-r3", "at_ns":near_end, "state":"down"}]);
    let result = report(&doc);
    assert!(result["stop_reason"].is_null());
    assert_eq!(result["link_events"][0]["at_ns"], near_end);
    assert_eq!(result["routing_events"].as_array().unwrap().len(), 3);
}
