use reliner::{RunLimits, run_lab, run_lab_with_limits};
use serde_json::{Value, json};

const LAB: &str = include_str!("../examples/failover-lab.json");
const PRIMARY: &str = "inter-switch-primary";
const STANDBY: &str = "inter-switch-standby";

fn lab() -> Value {
    serde_json::from_str(LAB).unwrap()
}

fn report(doc: &Value) -> Value {
    serde_json::to_value(run_lab(&doc.to_string()).unwrap()).unwrap()
}

fn cause(report: &Value, probe: usize) -> &Value {
    let id = &report["probes"][probe]["outcome"]["cause"];
    report["evidence"]
        .as_array()
        .unwrap()
        .iter()
        .find(|evidence| &evidence["id"] == id)
        .expect("the failed probe has a concrete cause")
}

fn repair_standby(doc: &mut Value) {
    doc["devices"][2]["ports"][2]["mode"]["allowed_vlans"] = json!([10, 20]);
}

#[test]
fn warm_traffic_switches_to_the_backup_and_reports_its_actual_vlan_rule() {
    let mut doc = lab();
    let failed = report(&doc);
    assert_eq!(failed["probes"][0]["outcome"]["at_ns"], "1600");
    assert_eq!(failed["probes"][0]["outcome"]["status"], "reachable");
    assert_eq!(cause(&failed, 1)["fact"]["kind"], "in_flight_link_loss");
    assert_eq!(cause(&failed, 1)["at_ns"], "2200");
    assert_eq!(cause(&failed, 1)["fact"]["link_id"], PRIMARY);
    assert_eq!(
        cause(&failed, 1)["fact"]["invalidated_by"],
        failed["link_events"][0]["event_id"]
    );
    let vlan_drop = cause(&failed, 2);
    assert_eq!(vlan_drop["frame"]["kind"], "echo_request");
    assert_eq!(vlan_drop["at_ns"], "2800");
    assert_eq!(vlan_drop["port"]["port_id"], "sw2-standby");
    assert_eq!(vlan_drop["fact"]["reason"]["allowed_vlans"], json!([20]));
    assert_eq!(vlan_drop["fact"]["stage"], "ingress");
    assert_eq!(vlan_drop["protected_link"]["link_id"], STANDBY);
    let failover = &failed["link_events"][0];
    assert_eq!(
        vlan_drop["protected_link"]["selected_by"],
        failover["event_id"]
    );
    assert_eq!(failover["protection"]["previous_active_link_id"], PRIMARY);
    assert_eq!(failover["protection"]["active_link_id"], STANDBY);
    assert_eq!(
        failed["link_events"][1]["protection"]["active_link_id"],
        PRIMARY
    );
    assert_eq!(failed["probes"][3]["outcome"]["status"], "reachable");
    assert_eq!(failed["probes"][3]["outcome"]["at_ns"], "4300");
    assert_eq!(failed["trace"]["status"], "disabled");

    repair_standby(&mut doc);
    let fixed = report(&doc);
    assert_eq!(fixed["probes"][2]["outcome"]["status"], "reachable");
    assert_eq!(fixed["probes"][2]["outcome"]["at_ns"], "3300");
    assert_eq!(fixed["probes"][1]["outcome"]["status"], "timeout");
    assert_eq!(fixed["probes"][3]["outcome"]["at_ns"], "4300");

    doc["scenario"]["trace_limit"] = json!(1);
    let traced = report(&doc);
    assert_eq!(traced["evidence"], fixed["evidence"]);
    assert_eq!(traced["link_events"], fixed["link_events"]);
    assert_eq!(traced["probes"], fixed["probes"]);
}

#[test]
fn an_undeclared_parallel_link_still_forms_an_invalid_loop() {
    let mut doc = lab();
    doc["links"][2]
        .as_object_mut()
        .unwrap()
        .remove("standby_for");
    assert!(
        run_lab(&doc.to_string())
            .unwrap_err()
            .to_string()
            .contains("环路")
    );
}

#[test]
fn a_blocked_standby_port_cannot_be_blamed_while_primary_is_active() {
    let mut doc = lab();
    doc["scenario"]["warmup"] = json!([]);
    doc["devices"][1]["ports"][2]["mode"]["allowed_vlans"] = json!([20]);
    doc["devices"][2]["ports"][1]["mode"]["allowed_vlans"] = json!([20]);
    let result = report(&doc);
    let first = cause(&result, 0);
    assert_eq!(first["port"]["port_id"], "sw2-primary");
    assert_eq!(first["frame"]["kind"], "arp_request");
    assert_eq!(first["fact"]["stage"], "ingress");
    assert_eq!(first["protected_link"]["link_id"], PRIMARY);
    assert!(first["protected_link"]["selected_by"].is_null());
    let third = cause(&result, 2);
    assert_eq!(third["port"]["port_id"], "sw1-standby");
    assert_eq!(third["fact"]["stage"], "egress");
}

#[test]
fn an_old_backup_frame_stays_invalid_after_the_backup_is_selected_again() {
    let mut doc = lab();
    repair_standby(&mut doc);
    doc["scenario"]["link_actions"] = json!([
        {"link": PRIMARY, "at_ns": "2200", "state": "down"},
        {"link": PRIMARY, "at_ns": "2700", "state": "up"},
        {"link": PRIMARY, "at_ns": "2750", "state": "down"},
        {"link": PRIMARY, "at_ns": "3650", "state": "up"}
    ]);
    doc["scenario"]["probes"]
        .as_array_mut()
        .unwrap()
        .push(json!({
            "id": "backup-again", "source": "pc-a", "destination": "pc-b",
            "at_ns": "2801", "deadline_ns": "3650"
        }));
    let result = report(&doc);
    let lost = cause(&result, 2);
    assert_eq!(lost["at_ns"], "2800");
    assert_eq!(lost["fact"]["link_id"], STANDBY);
    assert_eq!(lost["fact"]["kind"], "in_flight_protection_loss");
    assert_eq!(
        lost["fact"]["invalidated_by"],
        result["link_events"][1]["event_id"]
    );
    assert_eq!(lost["fact"]["invalidated_at_ns"], "2700");
    assert!(result["probes"][2]["request_evidence_id"].is_null());
    assert_eq!(result["probes"][4]["outcome"]["status"], "reachable");
    assert_eq!(result["probes"][4]["outcome"]["at_ns"], "3601");
}

#[test]
fn an_unavailable_pair_names_both_physical_outages() {
    let mut doc = lab();
    doc["scenario"]["link_actions"] = json!([
        {"link": STANDBY, "at_ns": "2150", "state": "down"},
        {"link": PRIMARY, "at_ns": "2200", "state": "down"}
    ]);
    let result = report(&doc);
    assert_eq!(
        result["link_events"][0]["protection"]["active_link_id"],
        PRIMARY
    );
    assert!(result["link_events"][1]["protection"]["active_link_id"].is_null());
    let blocked = cause(&result, 2);
    assert_eq!(blocked["at_ns"], "2600");
    assert_eq!(blocked["fact"]["kind"], "protection_unavailable");
    assert_eq!(blocked["fact"]["primary_link_id"], PRIMARY);
    assert_eq!(blocked["fact"]["standby_link_id"], STANDBY);
    assert_eq!(
        blocked["fact"]["primary_down_event_id"],
        result["link_events"][1]["event_id"]
    );
    assert_eq!(
        blocked["fact"]["standby_down_event_id"],
        result["link_events"][0]["event_id"]
    );
}

#[test]
fn same_time_members_share_a_commit_and_a_resource_boundary() {
    let mut doc = lab();
    doc["scenario"]["link_actions"] = json!([
        {"link": STANDBY, "at_ns": "1", "state": "down"},
        {"link": PRIMARY, "at_ns": "10", "state": "down"},
        {"link": STANDBY, "at_ns": "10", "state": "up"}
    ]);
    let result = report(&doc);
    assert_eq!(result["link_events"].as_array().unwrap().len(), 2);
    let change = &result["link_events"][1];
    assert_eq!(change["link_id"], PRIMARY);
    assert_eq!(change["state"], "down");
    assert_eq!(change["simultaneous"]["link_id"], STANDBY);
    assert_eq!(change["simultaneous"]["state"], "up");
    assert_eq!(change["protection"]["previous_active_link_id"], PRIMARY);
    assert_eq!(change["protection"]["active_link_id"], STANDBY);
    doc["scenario"]["link_actions"]
        .as_array_mut()
        .unwrap()
        .swap(1, 2);
    assert_eq!(report(&doc), result);

    doc["scenario"]["warmup"] = json!([]);
    let limits = RunLimits {
        essential_facts: 2,
        ..RunLimits::default()
    };
    let stopped =
        serde_json::to_value(run_lab_with_limits(&doc.to_string(), limits).unwrap()).unwrap();
    assert_eq!(
        stopped["stop_reason"],
        json!({"resource_limit": {"limit": "essential_facts"}})
    );
    assert_eq!(stopped["last_event_ns"], "1");
    assert_eq!(
        stopped["link_events"].as_array().unwrap(),
        &[result["link_events"][0].clone()]
    );
    assert_eq!(stopped["probes"][0]["outcome"]["status"], "incomplete");
}

#[test]
fn standby_declarations_validate_endpoints_and_disjoint_membership() {
    let mut doc = lab();
    doc["links"][2]["standby_for"] = json!("missing-link");
    assert!(
        run_lab(&doc.to_string())
            .unwrap_err()
            .to_string()
            .contains("missing-link")
    );
    doc["links"][2]["standby_for"] = json!("a-sw1");
    assert!(
        run_lab(&doc.to_string())
            .unwrap_err()
            .to_string()
            .contains("同一对交换机")
    );
    doc["links"][2]["standby_for"] = json!(STANDBY);
    assert!(
        run_lab(&doc.to_string())
            .unwrap_err()
            .to_string()
            .contains("只属于一组")
    );
    doc["links"][2]["standby_for"] = json!(PRIMARY);
    doc["links"][1]["standby_for"] = json!(STANDBY);
    assert!(
        run_lab(&doc.to_string())
            .unwrap_err()
            .to_string()
            .contains("只属于一组")
    );
}

#[test]
fn an_inactive_members_vlan_list_does_not_charge_the_active_flood() {
    let mut doc = lab();
    repair_standby(&mut doc);
    doc["devices"][1]["ports"][2]["mode"]["allowed_vlans"] = json!((1..=3000).collect::<Vec<_>>());
    doc["scenario"]["warmup"] = json!([]);
    doc["scenario"]["link_actions"] = json!([
        {"link": STANDBY, "at_ns": "1", "state": "down"},
        {"link": PRIMARY, "at_ns": "10", "state": "down"},
        {"link": STANDBY, "at_ns": "10", "state": "up"}
    ]);
    doc["scenario"]["probes"] = json!([{
        "id": "cold-reverse", "source": "pc-b", "destination": "pc-a",
        "at_ns": "1000", "deadline_ns": "4000"
    }]);
    for primary_vlans in [2183, 4094] {
        doc["devices"][1]["ports"][1]["mode"]["allowed_vlans"] =
            json!((1..=primary_vlans).collect::<Vec<_>>());
        let result = report(&doc);
        assert_eq!(result["probes"][0]["outcome"]["status"], "reachable");
        assert_eq!(result["probes"][0]["outcome"]["at_ns"], "2600");
    }
}

#[test]
fn protected_reports_keep_the_initial_roles_even_without_link_events() {
    let mut doc = lab();
    doc["scenario"]["link_actions"] = json!([]);
    let protected = report(&doc);
    assert_eq!(protected["report_version"], 3);
    assert_eq!(protected["model_version"], 3);
    assert_eq!(protected["link_events"], json!([]));
    assert_eq!(
        protected["settings"]["protected_links"],
        json!([{
            "primary_link_id": PRIMARY, "standby_link_id": STANDBY
        }])
    );

    doc["links"].as_array_mut().unwrap().remove(2);
    let original_format = report(&doc);
    assert_eq!(original_format["report_version"], 2);
    assert_eq!(original_format["model_version"], 2);
    assert!(original_format["settings"].get("protected_links").is_none());
}
