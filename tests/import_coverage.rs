use reliner::{RunLimits, run_lab, run_lab_with_limits};
use serde_json::{Value, json};

fn lab() -> Value {
    let mut doc: Value = serde_json::from_str(include_str!("../examples/ipv4-lab.json")).unwrap();
    doc["format_version"] = json!(3);
    doc["import_source"] = json!({
        "format":"containerlab", "source_name":"example.clab.yml",
        "gaps":[{"id":"return-policy","source_path":"/topology/nodes/r2/exec/0",
            "scope":{"kind":"device","device_id":"r2"},
            "value":"unsupported return policy", "message":"回程策略尚未映射"}]
    });
    doc["devices"].as_array_mut().unwrap().extend([
        json!({"kind":"host","id":"pc-c","name":"PC-C","ipv4":"198.18.0.1","prefix_length":24,
            "mac":"02:00:00:00:00:0c","port":{"id":"c-eth0","name":"eth0"}}),
        json!({"kind":"host","id":"pc-d","name":"PC-D","ipv4":"198.18.0.2","prefix_length":24,
            "mac":"02:00:00:00:00:0d","port":{"id":"d-eth0","name":"eth0"}}),
    ]);
    doc["links"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id":"c-d","ports":["c-eth0","d-eth0"],"delay_ns":"100"}));
    doc["scenario"]["probes"]
        .as_array_mut()
        .unwrap()
        .push(json!({
            "id":"independent","source":"pc-c","destination":"pc-d","at_ns":"0","deadline_ns":"2000"
        }));
    doc
}

#[test]
fn imported_return_policy_cannot_confirm_a_modeled_reply_or_pollute_another_component() {
    let result = run_lab(&lab().to_string()).unwrap();
    let value = serde_json::to_value(&result).unwrap();
    assert_eq!(value["report_version"], 8);
    assert_eq!(value["model_version"], 11);
    assert_eq!(value["probes"][0]["outcome"]["status"], "undetermined");
    assert_eq!(
        value["probes"][0]["outcome"]["modeled"]["status"],
        "reachable"
    );
    assert_eq!(value["probes"][2]["outcome"]["status"], "reachable");
    let counts = serde_json::to_value(result.probe_counts()).unwrap();
    assert_eq!(counts["completed"], 3);
    assert_eq!(counts["reachable"], 1);
    assert_eq!(counts["undetermined"], 2);
}

#[test]
fn missing_import_target_is_an_error_and_not_an_unaffected_probe() {
    let mut doc = lab();
    doc["import_source"]["gaps"][0]["scope"]["device_id"] = json!("deleted-router");
    let error = run_lab(&doc.to_string()).unwrap_err();
    assert_eq!(error.code, "unknown_import_target");
}

#[test]
fn an_unknown_probe_keeps_the_real_execution_stop_and_has_no_fabricated_evidence() {
    let result = run_lab_with_limits(
        &lab().to_string(),
        RunLimits {
            pending_events: 1,
            ..Default::default()
        },
    )
    .unwrap();
    let value = serde_json::to_value(&result).unwrap();
    assert_eq!(value["probes"][0]["outcome"]["status"], "undetermined");
    assert_eq!(
        value["probes"][0]["outcome"]["modeled"]["status"],
        "incomplete"
    );
    assert_eq!(value["probes"][0]["outcome"]["modeled"]["at_ns"], "0");
    assert_eq!(value["probes"][2]["outcome"]["status"], "incomplete");
    assert!(value["evidence"].as_array().unwrap().is_empty());
    assert!(result.is_incomplete());
    let counts = serde_json::to_value(result.probe_counts()).unwrap();
    assert_eq!(counts["completed"], 0);
    assert_eq!(counts["incomplete"], 3);
    assert_eq!(counts["undetermined"], 2);
}

#[test]
fn filling_the_last_import_gap_preserves_the_new_model_contract() {
    let mut doc = lab();
    doc["import_source"]["gaps"] = json!([]);
    let result = serde_json::to_value(run_lab(&doc.to_string()).unwrap()).unwrap();
    assert_eq!(result["report_version"], 8);
    assert_eq!(result["model_version"], 11);
    assert_eq!(result["probes"][0]["outcome"]["status"], "reachable");
}

#[test]
fn component_ids_are_stable_when_declarations_are_reordered() {
    let original = lab();
    let before = serde_json::to_value(run_lab(&original.to_string()).unwrap()).unwrap();
    let mut reordered = original;
    reordered["devices"].as_array_mut().unwrap().reverse();
    reordered["links"].as_array_mut().unwrap().reverse();
    let after = serde_json::to_value(run_lab(&reordered.to_string()).unwrap()).unwrap();
    assert_eq!(before["coverage"], after["coverage"]);
    for (before, after) in before["probes"]
        .as_array()
        .unwrap()
        .iter()
        .zip(after["probes"].as_array().unwrap())
    {
        assert_eq!(before["outcome"], after["outcome"]);
    }
}

#[test]
fn standby_and_stp_blocked_links_remain_in_the_import_coverage() {
    for (input, link) in [
        (
            include_str!("../examples/failover-lab.json"),
            "inter-switch-standby",
        ),
        (include_str!("../examples/stp-lab.json"), "s2-s3"),
    ] {
        let mut doc: Value = serde_json::from_str(input).unwrap();
        assert!(
            doc["links"]
                .as_array()
                .unwrap()
                .iter()
                .any(|row| row["id"] == link)
        );
        doc["format_version"] = json!(3);
        doc["import_source"] = json!({"format":"test", "source_name":"blocked-path", "gaps":[
            {"id":"unknown-link-policy", "source_path":"/links/policy", "scope":{"kind":"link", "link_id":link}, "value":"opaque", "message":"unmapped link policy"}
        ]});
        let report = serde_json::to_value(run_lab(&doc.to_string()).unwrap()).unwrap();
        assert!(
            report["probes"]
                .as_array()
                .unwrap()
                .iter()
                .all(|probe| probe["outcome"]["status"] == "undetermined")
        );
        assert!(
            report["evidence"]
                .as_array()
                .unwrap()
                .iter()
                .all(|fact| fact["model_only"] == true)
        );
        assert!(
            report["coverage"]["components"]
                .as_array()
                .unwrap()
                .iter()
                .any(|component| component["links"]
                    .as_array()
                    .unwrap()
                    .contains(&json!(link)))
        );
    }
}

#[test]
fn a_partial_stop_preserves_both_execution_and_import_axes_without_trace() {
    let mut doc: Value = serde_json::from_str(include_str!("../examples/campus-lab.json")).unwrap();
    doc["format_version"] = json!(3);
    doc["scenario"]["trace_limit"] = json!(0);
    doc["import_source"] = json!({"format":"test","source_name":"campus","gaps":[
        {"id":"global-policy","source_path":"/policy","scope":{"kind":"all"},"value":"opaque","message":"unmapped policy"}
    ]});
    let report = run_lab_with_limits(
        &doc.to_string(),
        RunLimits {
            essential_facts: 25,
            ..Default::default()
        },
    )
    .unwrap();
    let counts = report.probe_counts();
    assert!(counts.completed > 0 && counts.incomplete > 0);
    assert_eq!(counts.undetermined, counts.completed + counts.incomplete);
    assert_eq!(
        counts.reachable + counts.timed_out + counts.rejected_before_send,
        0
    );
    let encoded = serde_json::to_value(report).unwrap();
    assert!(
        encoded["probes"]
            .as_array()
            .unwrap()
            .iter()
            .all(|probe| probe["outcome"]["status"] == "undetermined")
    );
}

#[test]
fn save_reopen_and_later_edits_preserve_the_submitted_import_gaps() {
    use reliner::{RunMode, Workspace, WorkspaceConfig};
    use std::{
        fs,
        time::{Duration, Instant, SystemTime, UNIX_EPOCH},
    };

    let workspace = Workspace::new(WorkspaceConfig::default()).unwrap();
    let input = lab();
    let document = workspace.open_document(input.to_string()).unwrap();
    let path = std::env::temp_dir().join(format!(
        "reliner-import-{}-{}.json",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    workspace
        .save_document_at(document.id, document.revision, &path)
        .unwrap();
    workspace
        .close_document_at(document.id, document.revision, false)
        .unwrap();
    let reopened = workspace.open_file(&path).unwrap();
    let accepted = workspace
        .submit_run_at(reopened.id, reopened.revision, RunMode::Automatic)
        .unwrap();
    let mut filled = input;
    filled["import_source"]["gaps"] = json!([]);
    workspace
        .replace_experiment(reopened.id, reopened.revision, filled.to_string())
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut summary = workspace.run_summary(accepted.id).unwrap();
    while !summary.status.is_terminal() {
        assert!(Instant::now() < deadline);
        summary = workspace
            .wait_for_update(accepted.id, summary.update, Duration::from_millis(50))
            .unwrap();
    }
    let report = serde_json::to_value(&*workspace.run_report(accepted.id).unwrap()).unwrap();
    assert_eq!(report["import_source"]["gaps"][0]["id"], "return-policy");
    assert_eq!(report["probes"][0]["outcome"]["status"], "undetermined");
    assert_eq!(summary.origin.document_revision, reopened.revision);
    fs::remove_file(path).unwrap();
}
