use reliner::{RunLimits, run_lab, run_lab_with_limits};
use serde_json::{Value, json};

fn lab() -> Value {
    serde_json::from_str(include_str!("../examples/stp-lab.json")).unwrap()
}

fn report(doc: &Value) -> Value {
    serde_json::to_value(run_lab(&doc.to_string()).unwrap()).unwrap()
}

#[test]
fn unrelated_tree_change_preserves_learned_unicast_on_a_busy_access_link() {
    let host = |id: &str, last: u8| {
        json!({
            "kind":"host","id":id,"name":id,"ipv4":format!("192.0.2.{last}"),"prefix_length":24,
            "mac":format!("02:00:00:00:00:{last:02x}"),"port":{"id":format!("{id}0"),"name":"eth0"}
        })
    };
    let switch = |id: &str, ports: &[&str]| {
        json!({
            "kind":"switch","id":id,"name":id,"ports":ports.iter().map(|p| json!({
                "id":p,"name":p,"mode":{"kind":"access","vlan":10}
            })).collect::<Vec<_>>()
        })
    };
    let link = |id: &str, a: &str, b: &str| json!({"id":id,"ports":[a,b],"delay_ns":"10"});
    let mut doc = json!({
        "format_version":2,"id":"split","revision":"1",
        "devices":[host("a",10),host("b",11),host("c",12),switch("s",&["sa","sb","sc"]),
                   switch("x",&["xy"]),switch("y",&["yx"])],
        "links":[link("a","a0","sa"),link("b","b0","sb"),link("c","c0","sc"),link("xy","xy","yx")],
        "scenario":{"end_ns":"8000","stp_convergence_ns":"100","trace_limit":200,
            "arp_lifetime_ns":"50000","mac_lifetime_ns":"50000","link_actions":[],
            "warmup":[{"at_ns":"200","port":"a0","neighbor":"192.0.2.11"},
                      {"at_ns":"2000","port":"a0","neighbor":"192.0.2.12"}],
            "probes":[{"id":"b","source":"a","destination":"b","at_ns":"5000","deadline_ns":"7800"},
                      {"id":"c","source":"a","destination":"c","at_ns":"5001","deadline_ns":"7800"}]}
    });
    doc["links"][2]["transmission"] = json!({"rate_bps":"1000000000","queue_frames":0});
    let check_unicast = |doc: &Value| {
        let result = report(doc);
        for (probe, at) in result["probes"]
            .as_array()
            .unwrap()
            .iter()
            .zip(["5040", "6673"])
        {
            assert_eq!(probe["outcome"]["at_ns"], at);
            assert_eq!(probe["outcome"]["status"], "reachable");
        }
        assert!(
            !result["trace"]["entries"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["probe_id"] == "b" && e["port"]["port_id"] == "c0")
        );
    };
    check_unicast(&doc);
    doc["scenario"]["link_actions"] = json!([{"at_ns":"4000","link":"xy","state":"down"}]);
    check_unicast(&doc);

    // Repeat with the busy switch also participating in a managed tree.
    doc["devices"][3] = switch("s", &["sa", "sb", "sc", "sz"]);
    doc["devices"]
        .as_array_mut()
        .unwrap()
        .push(switch("z", &["zs"]));
    doc["links"]
        .as_array_mut()
        .unwrap()
        .push(link("sz", "sz", "zs"));
    check_unicast(&doc);
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

fn bridge<'a>(event: &'a Value, id: &str) -> &'a Value {
    event["bridges"]
        .as_array()
        .unwrap()
        .iter()
        .find(|bridge| bridge["bridge_id"] == id)
        .unwrap()
}

fn port<'a>(event: &'a Value, id: &str) -> &'a Value {
    event["links"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|link| link["ports"].as_array().unwrap())
        .find(|port| port["port_id"] == id)
        .unwrap()
}

#[test]
fn independent_domains_keep_their_convergence_clocks_and_recovery_gates() {
    let mut doc = lab();
    doc["scenario"]["stp_convergence_ns"] = json!("100");
    doc["scenario"]["warmup"][0]["at_ns"] = json!("300");
    doc["devices"][3]["ports"][0]["mode"]["allowed_vlans"] = json!([10, 20]);
    for link in doc["links"].as_array_mut().unwrap() {
        link["delay_ns"] = json!("5");
    }
    for (id, port) in [("x", "xy"), ("y", "yx")] {
        doc["devices"].as_array_mut().unwrap().push(json!({
            "kind":"switch","name":id,"id":id,"ports":[{"id":port,"name":port,"mode":{"kind":"access","vlan":10}}]
        }));
    }
    doc["links"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id":"xy","ports":["xy","yx"],"delay_ns":"5"}));
    doc["scenario"]["probes"] = json!([
        {"id":"during","source":"pc-a","destination":"pc-b","at_ns":"760","deadline_ns":"850"},
        {"id":"after","source":"pc-a","destination":"pc-b","at_ns":"800","deadline_ns":"950"}
    ]);
    let local_down = json!({"link":"s1-s3","at_ns":"700","state":"down"});
    for recover in [false, true] {
        let mut actions = vec![local_down.clone()];
        if recover {
            actions.push(json!({"link":"s1-s3","at_ns":"730","state":"up"}));
        }
        doc["scenario"]["link_actions"] = json!(actions);
        let baseline = report(&doc);
        actions.insert(0, json!({"link":"xy","at_ns":"650","state":"down"}));
        doc["scenario"]["link_actions"] = json!(actions);
        let changed = report(&doc);
        let loss = cause(&changed, 0);
        assert_eq!(
            loss["fact"]["kind"],
            if recover {
                "stp_awaiting_convergence"
            } else {
                "link_down"
            }
        );
        assert_eq!(loss["at_ns"], cause(&baseline, 0)["at_ns"]);
        let events = changed["stp_events"].as_array().unwrap();
        assert_eq!(
            events.iter().map(|e| &e["at_ns"]).collect::<Vec<_>>(),
            ["100", "750", "800"]
        );
        assert_eq!(port(&events[1], "s3-s1"), port(&events[0], "s3-s1"));
        assert_eq!(port(&events[1], "xy")["role"], "disabled");
        for i in 1..=2 {
            assert_eq!(
                events[i]["triggered_by_link_event_id"],
                changed["link_events"][i - 1]["event_id"]
            );
        }
        if recover {
            assert_eq!(loss["stp_event_id"], events[0]["event_id"]);
        }
        assert_eq!(changed["probes"][1]["outcome"]["status"], "reachable");
        assert_eq!(
            changed["probes"][1]["outcome"]["at_ns"],
            baseline["probes"][1]["outcome"]["at_ns"]
        );
    }

    doc["scenario"]["link_actions"] = json!([
        {"link":"xy","at_ns":"650","state":"down"},
        {"link":"s1-s3","at_ns":"650","state":"down"}
    ]);
    let simultaneous = report(&doc);
    let events = simultaneous["stp_events"].as_array().unwrap();
    assert_eq!(events.len(), 3);
    assert_eq!(events[1]["at_ns"], "750");
    assert_eq!(events[2]["at_ns"], "750");
    assert_ne!(events[1]["event_id"], events[2]["event_id"]);
    assert_eq!(port(&events[1], "s3-s1")["role"], "root");
    assert_eq!(port(&events[2], "s3-s1")["role"], "disabled");

    // Initial history costs 18 facts, the two outages cost 2 and the X-Y update costs 5.
    let limited = run_lab_with_limits(
        &doc.to_string(),
        RunLimits {
            essential_facts: 25,
            ..Default::default()
        },
    )
    .unwrap();
    let limited = serde_json::to_value(limited).unwrap();
    assert_eq!(
        limited["stop_reason"],
        json!({"resource_limit":{"limit":"essential_facts"}})
    );
    assert_eq!(limited["last_event_id"], events[1]["event_id"]);
    assert_eq!(limited["stp_events"].as_array().unwrap(), &events[..2]);
}

#[test]
fn path_cost_repair_explains_the_port_and_keeps_the_original_probes() {
    let mut doc = lab();
    let broken = report(&doc);
    assert_eq!(broken["report_version"], 7);
    assert_eq!(broken["model_version"], 14);
    let first = &broken["stp_events"][0];
    let alternate = &broken["stp_events"][1];
    let restored = &broken["stp_events"][2];
    assert_eq!(first["at_ns"], "500");
    assert_eq!(bridge(first, "sw3")["root_bridge_id"], "sw1");
    assert_eq!(bridge(first, "sw3")["root_path_cost"], "1");
    assert_eq!(port(first, "s3-s1")["role"], "root");
    assert_eq!(port(first, "s3-s2")["role"], "alternate");
    let loss = cause(&broken, 0);
    assert_eq!(loss["port"]["port_id"], "s3-s1");
    assert_eq!(loss["fact"]["reason"]["rule"], "trunk_vlan_not_allowed");
    assert_eq!(loss["stp_event_id"], first["event_id"]);
    assert_eq!(cause(&broken, 1)["fact"]["kind"], "link_down");
    assert_eq!(alternate["at_ns"], "3200");
    assert_eq!(
        alternate["triggered_by_link_event_id"],
        broken["link_events"][0]["event_id"]
    );
    assert_eq!(port(alternate, "s3-s1")["role"], "disabled");
    assert_eq!(port(alternate, "s3-s2")["role"], "root");
    assert_eq!(broken["probes"][2]["outcome"]["status"], "reachable");
    assert_eq!(broken["probes"][2]["outcome"]["at_ns"], "5100");
    assert_eq!(restored["at_ns"], "6000");
    assert_eq!(cause(&broken, 3)["stp_event_id"], restored["event_id"]);
    assert_eq!(cause(&broken, 3)["port"]["port_id"], "s3-s1");

    doc["devices"][3]["ports"][0]["stp_cost"] = json!(10);
    let fixed = report(&doc);
    assert_eq!(
        bridge(&fixed["stp_events"][0], "sw3")["root_path_cost"],
        "2"
    );
    assert_eq!(port(&fixed["stp_events"][0], "s3-s1")["role"], "alternate");
    for (i, at) in ["2300", "3500", "4300", "7000"].into_iter().enumerate() {
        assert_eq!(fixed["probes"][i]["outcome"]["status"], "reachable");
        assert_eq!(fixed["probes"][i]["outcome"]["at_ns"], at);
    }
    doc["scenario"]["trace_limit"] = json!(0);
    let untraced = report(&doc);
    assert_eq!(untraced["stp_events"], fixed["stp_events"]);
    assert_eq!(untraced["evidence"], fixed["evidence"]);
    assert_eq!(untraced["probes"], fixed["probes"]);
}

#[test]
fn withdrawn_path_loses_an_in_flight_required_frame_at_the_control_boundary() {
    let mut doc = lab();
    let mut in_flight = doc["scenario"]["probes"][3].clone();
    in_flight["at_ns"] = json!("5700");
    in_flight["id"] = json!("in-flight");
    doc["scenario"]["probes"]
        .as_array_mut()
        .unwrap()
        .push(in_flight);
    let result = report(&doc);
    let loss = cause(&result, 4);
    assert_eq!(loss["fact"]["kind"], "in_flight_stp_loss");
    assert_eq!(loss["fact"]["link_id"], "s2-s3");
    assert_eq!(loss["fact"]["planned_arrival_ns"], "6000");
    assert_eq!(loss["at_ns"], "6000");
    assert_eq!(
        loss["fact"]["invalidated_by"],
        result["stp_events"][2]["event_id"]
    );
}

#[test]
fn a_filtered_branch_beside_a_selected_path_cannot_steal_the_cause() {
    let mut doc = lab();
    doc["devices"][2]["ports"][0]["mode"]["allowed_vlans"] = json!([20]);
    let result = report(&doc);
    let actual = cause(&result, 0);
    assert_eq!(actual["port"]["port_id"], "s3-s1");
    assert_eq!(actual["at_ns"], "1700");
}

#[test]
fn parallel_links_break_root_port_ties_by_the_senders_port() {
    let mut doc = lab();
    doc["devices"][1]["ports"].as_array_mut().unwrap().push(json!({
        "id":"s1-parallel", "name":"Parallel", "stp_cost":100, "mode":{"kind":"trunk", "allowed_vlans":[10,20]}
    }));
    doc["devices"][3]["ports"]
        .as_array_mut()
        .unwrap()
        .push(json!({
            "id":"s3-zparallel", "name":"Parallel", "mode":{"kind":"trunk", "allowed_vlans":[10,20]}
        }));
    doc["links"].as_array_mut().unwrap().push(json!({
        "id":"parallel", "ports":["s3-zparallel","s1-parallel"], "delay_ns":"100"
    }));
    let chosen = report(&doc);
    assert_eq!(
        bridge(&chosen["stp_events"][0], "sw3")["root_port_id"],
        "s3-zparallel"
    );
    assert_eq!(
        bridge(&chosen["stp_events"][0], "sw3")["root_path_cost"],
        "1"
    );
    assert_eq!(port(&chosen["stp_events"][0], "s3-s1")["role"], "alternate");
    assert_eq!(chosen["probes"][0]["outcome"]["status"], "reachable");
    doc["devices"].as_array_mut().unwrap().reverse();
    doc["links"].as_array_mut().unwrap().reverse();
    assert_eq!(report(&doc)["stp_events"], chosen["stp_events"]);
}

#[test]
fn link_changes_at_the_boundary_are_included_and_tree_publication_is_atomic() {
    let mut doc = lab();
    doc["scenario"]["link_actions"][1] = json!({"link":"s2-s3", "at_ns":"3200", "state":"down"});
    let result = report(&doc);
    assert_eq!(result["stp_events"].as_array().unwrap().len(), 2);
    let split = &result["stp_events"][1];
    assert_eq!(bridge(split, "sw3")["root_bridge_id"], "sw3");
    assert_eq!(port(split, "s3-s1")["role"], "disabled");
    assert_eq!(port(split, "s3-s2")["role"], "disabled");
    assert!(result["probes"][2]["outcome"]["cause"].is_null());

    let limited = run_lab_with_limits(
        &doc.to_string(),
        RunLimits {
            essential_facts: 12,
            ..Default::default()
        },
    )
    .unwrap();
    let limited = serde_json::to_value(limited).unwrap();
    assert_eq!(
        limited["stop_reason"],
        json!({"resource_limit":{"limit":"essential_facts"}})
    );
    assert!(limited["last_event_id"].is_null());
    assert!(limited["stp_events"].is_null());
    assert_eq!(limited["probes"][0]["outcome"]["status"], "incomplete");
}

#[test]
fn a_late_frame_from_the_old_tree_cannot_relearn_the_source_on_a_retained_link() {
    let mut doc = lab();
    doc["devices"][3]["ports"][0]["mode"]["allowed_vlans"] = json!([10, 20]);
    doc["scenario"]["probes"].as_array_mut().unwrap().push(json!({
        "id":"late-from-b", "source":"pc-b", "destination":"pc-a", "at_ns":"5750", "deadline_ns":"7400"
    }));
    let result = report(&doc);
    assert_eq!(result["probes"][4]["outcome"]["at_ns"], "6450");
    assert_eq!(result["probes"][4]["outcome"]["status"], "reachable");
    assert_eq!(result["probes"][3]["outcome"]["at_ns"], "6800");
    assert_eq!(result["probes"][3]["outcome"]["status"], "reachable");
}

#[test]
fn recomputed_tree_replaces_its_own_resident_entries() {
    let mut doc = lab();
    doc["scenario"]["warmup"] = json!([]);
    doc["devices"][0]["port"]["ipv4_mtu_bytes"] = json!(68);
    let run = run_lab_with_limits(
        &doc.to_string(),
        RunLimits {
            state_entries: 13,
            ..Default::default()
        },
    )
    .unwrap();
    let result = serde_json::to_value(run).unwrap();
    assert!(result["stop_reason"].is_null());
    assert_eq!(result["stp_events"].as_array().unwrap().len(), 3);
}

#[test]
fn a_physically_restored_tree_link_waits_for_control_before_forwarding() {
    let mut doc = lab();
    doc["devices"][3]["ports"][0]["mode"]["allowed_vlans"] = json!([10, 20]);
    doc["scenario"]["link_actions"][1]["at_ns"] = json!("2800");
    doc["scenario"]["probes"][1]["at_ns"] = json!("2800");
    doc["scenario"]["probes"][2]["at_ns"] = json!("3100");
    let result = report(&doc);
    let held = cause(&result, 1);
    assert_eq!(held["at_ns"], "2900");
    assert_eq!(held["fact"]["kind"], "stp_awaiting_convergence");
    assert_eq!(
        held["fact"]["up_event_id"],
        result["link_events"][1]["event_id"]
    );
    assert_eq!(result["stp_events"].as_array().unwrap().len(), 2);
    assert_eq!(result["stp_events"][1]["at_ns"], "3200");
    assert_eq!(result["probes"][2]["outcome"]["status"], "reachable");
    assert_eq!(result["probes"][2]["outcome"]["at_ns"], "3700");
}

#[test]
fn flood_preflight_charges_the_ingress_vlan_list_only_once() {
    let mut doc = lab();
    doc["scenario"]["link_actions"] = json!([]);
    let vlans = json!((1..=4094).collect::<Vec<_>>());
    doc["devices"][1]["ports"][2]["mode"]["allowed_vlans"] = vlans.clone();
    doc["devices"][3]["ports"][0]["mode"]["allowed_vlans"] = vlans;
    let result = report(&doc);
    assert!(result["stop_reason"].is_null());
    assert_eq!(result["probes"][0]["outcome"]["status"], "reachable");
}

#[test]
fn campus_stp_budget_covers_peer_pairs_and_the_compact_forwarding_index() {
    let input = include_str!("../examples/campus-lab.json");
    let run = |event_work| {
        let report = run_lab_with_limits(
            input,
            RunLimits {
                event_work,
                ..RunLimits::default()
            },
        )
        .unwrap();
        let reliner::RunReport::Scenario(report) = report else {
            panic!("expected scenario report");
        };
        report
    };
    let below = run(94);
    assert_eq!(
        below.stop_reason,
        Some(reliner::StopReason::ResourceLimit {
            limit: reliner::ResourceLimit::EventWork,
        })
    );
    assert_eq!(below.last_event_ns, 200);
    assert!(below.stp_events.is_empty());

    let exact = run(95);
    assert!(exact.stop_reason.is_none());
    let reference = run(RunLimits::default().event_work);
    assert_eq!(exact.probes, reference.probes);
    assert_eq!(exact.evidence, reference.evidence);
    assert_eq!(exact.stp_events, reference.stp_events);
    assert_eq!(exact.routing_events, reference.routing_events);
    assert_eq!(exact.last_event_id, reference.last_event_id);
}

#[test]
fn unchanged_tree_round_counts_only_the_event_and_keeps_the_full_report() {
    let mut doc = lab();
    doc["scenario"]["warmup"] = json!([]);
    doc["scenario"]["trace_limit"] = json!(0);
    doc["scenario"]["probes"]
        .as_array_mut()
        .unwrap()
        .truncate(1);
    doc["scenario"]["link_actions"][1]["at_ns"] = json!("2800");

    let reference = report(&doc);
    assert_eq!(
        reference["stp_events"][0]["bridges"],
        reference["stp_events"][1]["bridges"]
    );
    assert_eq!(
        reference["stp_events"][0]["links"],
        reference["stp_events"][1]["links"]
    );
    let limited = |essential_facts| {
        serde_json::to_value(
            run_lab_with_limits(
                &doc.to_string(),
                RunLimits {
                    essential_facts,
                    ..RunLimits::default()
                },
            )
            .unwrap(),
        )
        .unwrap()
    };
    let below = limited(16);
    assert_eq!(
        below["stop_reason"],
        json!({"resource_limit":{"limit":"essential_facts"}})
    );
    assert_eq!(below["last_event_ns"], "2800");
    assert_eq!(below["stp_events"].as_array().unwrap().len(), 1);

    let exact = limited(17);
    assert!(exact["stop_reason"].is_null());
    assert_eq!(exact["stp_events"], reference["stp_events"]);
    assert_eq!(exact["evidence"], reference["evidence"]);
    assert_eq!(exact["probes"], reference["probes"]);
    assert_eq!(exact["last_event_id"], reference["last_event_id"]);
}

#[test]
fn stp_work_includes_possible_history_copies_before_the_commit() {
    let mut doc = lab();
    doc["scenario"]["warmup"] = json!([]);
    doc["scenario"]["probes"]
        .as_array_mut()
        .unwrap()
        .truncate(1);
    doc["scenario"]["link_actions"] = json!(
        (0..5)
            .map(|index| json!({
                "link":"s1-s3",
                "at_ns":(2600 + 700 * index).to_string(),
                "state": if index % 2 == 0 { "down" } else { "up" },
            }))
            .collect::<Vec<_>>()
    );
    let run = |event_work| {
        let r = run_lab_with_limits(
            &doc.to_string(),
            RunLimits {
                event_work,
                ..Default::default()
            },
        )
        .unwrap();
        let reliner::RunReport::Scenario(report) = r else {
            panic!("expected scenario")
        };
        report
    };
    let below = run(106);
    assert_eq!(
        below.stop_reason,
        Some(reliner::StopReason::ResourceLimit {
            limit: reliner::ResourceLimit::EventWork,
        })
    );
    assert_eq!(below.last_event_ns, 5400);
    assert_eq!(below.stp_events.len(), 5);
    let exact = run(107);
    assert!(exact.stop_reason.is_none());
    assert_eq!(exact.stp_events.len(), 6);
    assert_eq!(below.stp_events.get(4), exact.stp_events.get(4));
}

#[test]
fn a_parallel_peer_falls_back_to_the_next_eligible_receive_port() {
    let mut doc = lab();
    doc["devices"][1]["ports"]
        .as_array_mut()
        .unwrap()
        .push(json!({
            "id":"s1-parallel","name":"Parallel","stp_cost":30,
            "mode":{"kind":"trunk","allowed_vlans":[10,20]}
        }));
    doc["devices"][3]["ports"]
        .as_array_mut()
        .unwrap()
        .push(json!({
            "id":"s3-parallel","name":"Parallel","stp_cost":2,
            "mode":{"kind":"trunk","allowed_vlans":[10,20]}
        }));
    doc["links"].as_array_mut().unwrap().push(json!({
        "id":"00-parallel","ports":["s1-parallel","s3-parallel"],"delay_ns":"100"
    }));
    let result = report(&doc);
    let events = result["stp_events"].as_array().unwrap();
    for (event, preferred, other) in [
        (&events[0], "s3-s1", "s3-parallel"),
        (&events[1], "s3-parallel", "s3-s1"),
        (&events[2], "s3-s1", "s3-parallel"),
    ] {
        assert_eq!(bridge(event, "sw3")["root_port_id"], preferred);
        assert_eq!(port(event, preferred)["role"], "root");
        assert_ne!(port(event, other)["role"], "root");
    }
    assert_eq!(port(&events[1], "s3-s1")["role"], "disabled");
    assert_eq!(result["probes"][2]["outcome"]["status"], "reachable");
}

#[test]
fn access_only_switch_segments_do_not_occupy_a_dynamic_tree_index() {
    let mut doc = lab();
    doc["scenario"]["warmup"] = json!([]);
    doc["scenario"]["link_actions"] = json!([]);
    doc["devices"][0]["port"]["ipv4_mtu_bytes"] = json!(68);
    doc["devices"][3]["ports"][0]
        .as_object_mut()
        .unwrap()
        .remove("stp_cost");
    doc["links"]
        .as_array_mut()
        .unwrap()
        .retain(|link| link["id"] == "a-s1" || link["id"] == "s3-b");
    let r = run_lab_with_limits(
        &doc.to_string(),
        RunLimits {
            state_entries: 0,
            ..Default::default()
        },
    )
    .unwrap();
    let result = serde_json::to_value(r).unwrap();
    assert!(result["stop_reason"].is_null());
    assert!(
        result["stp_events"][0]["links"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        result["stp_events"][0]["bridges"].as_array().unwrap().len(),
        3
    );
}
