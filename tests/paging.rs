use std::time::{Duration, Instant};

use reliner::{DocumentId, Outcome, RunMode, RunStatus, WaitingFor, Workspace, WorkspaceConfig};
use serde_json::{Value, json};

fn run(workspace: &Workspace, input: String) -> reliner::AcceptedRun {
    let document = workspace.open_document(input).unwrap();
    run_document(workspace, document.id)
}

fn run_document(workspace: &Workspace, id: DocumentId) -> reliner::AcceptedRun {
    let accepted = workspace.submit_run(id, RunMode::Automatic).unwrap();
    let mut state = workspace.run_summary(accepted.id).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !state.status.is_terminal() {
        assert!(Instant::now() < deadline, "run did not finish: {state:?}");
        state = workspace
            .wait_for_update(accepted.id, state.update, Duration::from_secs(1))
            .unwrap();
    }
    assert_eq!(state.status, RunStatus::Finished);
    accepted
}

#[test]
fn stp_headers_and_rows_reconstruct_existing_events() {
    let workspace = Workspace::new(WorkspaceConfig::default()).unwrap();
    let accepted = run(&workspace, include_str!("../examples/stp-lab.json").into());
    let headers = workspace.run_stp_summary_page(accepted.id, 0, 128).unwrap();
    assert!(!headers.entries.is_empty());
    for header in headers.entries {
        let event = workspace
            .run_stp_event(accepted.id, header.event_id)
            .unwrap()
            .unwrap();
        assert_eq!(header.bridge_count, event.bridges.len());
        assert_eq!(header.link_count, event.links.len());
        let mut bridges = Vec::new();
        let mut offset = 0;
        loop {
            let page = workspace
                .run_stp_bridge_page(accepted.id, header.event_id, offset, 1)
                .unwrap()
                .unwrap();
            bridges.extend(page.entries);
            let Some(next) = page.next_offset else { break };
            offset = next;
        }
        assert_eq!(bridges, event.bridges);
        assert_eq!(
            workspace
                .run_stp_link_page(accepted.id, header.event_id, 0, 128)
                .unwrap()
                .unwrap()
                .entries,
            event.links
        );
    }
    assert!(
        workspace
            .run_stp_bridge_page(accepted.id, reliner::EventId(u64::MAX), 0, 1)
            .unwrap()
            .is_none()
    );
}

#[test]
fn comparison_pages_merge_ids_without_losing_added_or_removed_probes() {
    let workspace = Workspace::new(WorkspaceConfig::default()).unwrap();
    let mut input: Value = serde_json::from_str(include_str!("../examples/ipv4-lab.json")).unwrap();
    let before = run(&workspace, input.to_string());
    input["scenario"]["probes"][0]["id"] = json!("new-probe");
    let after = run(&workspace, input.to_string());
    let expected = workspace.compare_runs(before.id, after.id).unwrap();
    let mut entries = Vec::new();
    let mut offset = 0;
    loop {
        let page = workspace
            .compare_run_page(before.id, after.id, offset, 1)
            .unwrap();
        assert_eq!(page.baseline, expected.baseline);
        assert_eq!(page.candidate, expected.candidate);
        entries.extend(page.page.entries);
        let Some(next) = page.page.next_offset else {
            break;
        };
        offset = next;
    }
    assert_eq!(entries, expected.probes);
    assert!(
        workspace
            .compare_run_page(before.id, after.id, usize::MAX, 128)
            .unwrap()
            .page
            .entries
            .is_empty()
    );
}

#[test]
fn restoring_source_gateway_preserves_compared_probe_parameters() {
    let workspace = Workspace::new(WorkspaceConfig::default()).unwrap();
    let mut input: Value = serde_json::from_str(include_str!("../examples/ipv4-lab.json")).unwrap();
    let gateway = input["devices"][0]
        .as_object_mut()
        .unwrap()
        .remove("gateway")
        .unwrap();
    let doc = workspace.open_document(input.to_string()).unwrap();
    let before = run_document(&workspace, doc.id);

    input["devices"][0]["gateway"] = gateway;
    input["revision"] = json!("2");
    let repaired = workspace
        .replace_experiment(doc.id, doc.revision, input.to_string())
        .unwrap();
    let after = run_document(&workspace, repaired.id);

    let page = workspace
        .compare_run_page(before.id, after.id, 0, 2)
        .unwrap();
    assert_eq!(page.page.entries.len(), 2);
    for row in &page.page.entries {
        assert!(matches!(
            row.baseline.as_ref().unwrap().outcome,
            Outcome::Timeout {
                waiting_for: WaitingFor::ProbeDeadline,
                ..
            }
        ));
        assert!(matches!(
            row.candidate.as_ref().unwrap().outcome,
            Outcome::Reachable { .. }
        ));
        assert!(
            row.same_probe_parameters,
            "gateway repair changed probe {} comparison (model {} -> {})",
            row.probe_id, page.baseline.model_version, page.candidate.model_version
        );
    }
    let full = workspace.compare_runs(before.id, after.id).unwrap();
    assert_eq!(page.page.entries, full.probes);
}

#[test]
fn stp_comparison_uses_effective_payload_defaults() {
    let workspace = Workspace::new(WorkspaceConfig::default()).unwrap();
    let mut input: Value = serde_json::from_str(include_str!("../examples/stp-lab.json")).unwrap();
    let before = run(&workspace, input.to_string());
    let changed_probe = input["scenario"]["probes"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    for payload in [56, 57] {
        input["scenario"]["probes"][0]["payload_bytes"] = json!(payload);
        let after = run(&workspace, input.to_string());
        let page = workspace
            .compare_run_page(before.id, after.id, 0, 128)
            .unwrap();
        let full = workspace.compare_runs(before.id, after.id).unwrap();
        assert_eq!(page.page.entries, full.probes);
        assert_eq!(page.page.entries.len(), 4);
        for row in &page.page.entries {
            let effective_payload = if row.probe_id == changed_probe {
                payload
            } else {
                56
            };
            assert_eq!(row.baseline.as_ref().unwrap().payload_bytes, None);
            assert_eq!(
                row.candidate.as_ref().unwrap().payload_bytes,
                Some(effective_payload)
            );
            assert_eq!(
                row.same_probe_parameters,
                effective_payload == 56,
                "{}",
                row.probe_id
            );
        }
    }
}

#[test]
fn import_gap_pages_omit_raw_values_and_reject_invalid_coverage() {
    let workspace = Workspace::new(WorkspaceConfig::default()).unwrap();
    let mut input: Value = serde_json::from_str(include_str!("../examples/ipv4-lab.json")).unwrap();
    input["format_version"] = json!(3);
    input["import_source"] = json!({"format":"test","source_name":"source","gaps":[
        {"id":"policy","source_path":"/policy","scope":{"kind":"all"},"message":"unknown policy","value":"raw policy"}
    ]});
    let accepted = run(&workspace, input.to_string());
    let page = workspace
        .run_import_gap_page(accepted.id, None, 0, 1)
        .unwrap();
    let encoded = serde_json::to_value(&page).unwrap();
    assert!(encoded["entries"][0].get("value").is_none());
    assert_eq!(
        workspace
            .run_import_gap(accepted.id, "policy")
            .unwrap()
            .unwrap()
            .value,
        json!("raw policy")
    );
    assert!(matches!(
        workspace.run_import_gap_page(
            accepted.id,
            Some(reliner::CoverageRef {
                source_component: reliner::ComponentId(999),
                destination_component: reliner::ComponentId(999)
            }),
            0,
            1
        ),
        Err(reliner::WorkspaceError::InvalidCoverageReference)
    ));
}
