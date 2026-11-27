use std::{
    fs,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

use reliner::{
    RunControl, RunMode, RunStatus, Workspace, WorkspaceConfig, WorkspaceError, run_lab_with_limits,
};
use serde_json::{Value, json};

const VLAN: &str = include_str!("../examples/vlan-lab.json");
const IPV4: &str = include_str!("../examples/ipv4-lab.json");

fn workers() -> Workspace {
    Workspace::new(WorkspaceConfig {
        workers: 1,
        resident_runs: 1,
        queue_slots: 1,
        ..WorkspaceConfig::default()
    })
    .unwrap()
}

fn working_vlan() -> String {
    let mut doc: Value = serde_json::from_str(VLAN).unwrap();
    doc["devices"][2]["ports"][0]["mode"]["allowed_vlans"] = json!([10, 20]);
    doc["revision"] = json!("2");
    doc.to_string()
}

fn wait(
    workspace: &Workspace,
    accepted: reliner::AcceptedRun,
    wanted: impl Fn(RunStatus) -> bool,
) -> reliner::RunSummary {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut summary = workspace.run_summary(accepted.id).unwrap();
    while !wanted(summary.status) {
        assert!(
            Instant::now() < deadline,
            "run did not reach expected state: {summary:?}"
        );
        summary = workspace
            .wait_for_update(accepted.id, summary.update, Duration::from_millis(50))
            .unwrap();
    }
    summary
}

#[test]
fn queued_run_keeps_unsaved_revision_when_later_edits_change_the_outcome() {
    let workspace = workers();
    let doc = workspace.open_document(VLAN.into()).unwrap();
    let parking = workspace.open_document(IPV4.into()).unwrap();
    let held = workspace.submit_run(parking.id, RunMode::Paused).unwrap();
    wait(&workspace, held, |status| status == RunStatus::Paused);
    let layout = workspace
        .replace_presentation(doc.id, doc.revision, Some(r#"{"x":10}"#.into()))
        .unwrap();
    assert_eq!(layout.effective_revision, doc.effective_revision);
    let original = workspace.submit_run(doc.id, RunMode::Automatic).unwrap();
    assert_eq!(workspace.stats().queued, 1);
    assert_eq!(
        workspace.run_summary(original.id).unwrap().status,
        RunStatus::Queued
    );
    assert!(matches!(
        workspace.submit_run(doc.id, RunMode::Automatic),
        Err(WorkspaceError::QueueFull)
    ));
    let edited = workspace
        .replace_experiment(doc.id, layout.revision, working_vlan())
        .unwrap();
    assert!(edited.dirty);
    workspace.control_run(held.id, RunControl::Cancel).unwrap();
    wait(&workspace, original, RunStatus::is_terminal);
    let later = workspace.submit_run(doc.id, RunMode::Automatic).unwrap();
    wait(&workspace, later, RunStatus::is_terminal);
    assert_eq!(original.effective_revision, doc.effective_revision);
    assert_eq!(later.effective_revision, edited.effective_revision);
    let old = serde_json::to_value(&*workspace.run_report(original.id).unwrap()).unwrap();
    let new = serde_json::to_value(&*workspace.run_report(later.id).unwrap()).unwrap();
    assert_eq!(old["revision"], "1");
    assert_eq!(old["outcome"]["status"], "timeout");
    assert_eq!(old["evidence"][0]["port"]["port_id"], "sw2-uplink");
    assert_eq!(new["revision"], "2");
    assert_eq!(new["outcome"]["status"], "reachable");
    assert_eq!(workspace.stats().input_bytes, 0);
    assert_eq!(workspace.stats().resident, 0);
}

#[test]
fn paused_step_and_resumed_run_use_the_same_event_order() {
    let workspace = workers();
    let doc = workspace.open_document(IPV4.into()).unwrap();
    let run = workspace.submit_run(doc.id, RunMode::Paused).unwrap();
    let parked = wait(&workspace, run, |status| status == RunStatus::Paused);
    assert!(parked.last_event_id.is_none());
    assert_eq!(workspace.stats().resident, 1);
    assert_eq!(workspace.stats().input_bytes, 0);
    workspace.control_run(run.id, RunControl::Step).unwrap();
    let step = wait(&workspace, run, |status| status == RunStatus::Paused);
    assert_eq!(step.last_event_id, Some(reliner::EventId(1)));
    workspace.control_run(run.id, RunControl::Resume).unwrap();
    wait(&workspace, run, RunStatus::is_terminal);
    assert_eq!(
        *workspace.run_report(run.id).unwrap(),
        run_lab_with_limits(IPV4, WorkspaceConfig::default().run_limits).unwrap()
    );
}

#[test]
fn input_and_history_slots_are_returned_at_their_own_boundaries() {
    let workspace = Workspace::new(WorkspaceConfig {
        workers: 1,
        resident_runs: 1,
        history_runs: 1,
        input_bytes: IPV4.len(),
        ..WorkspaceConfig::default()
    })
    .unwrap();
    let doc = workspace.open_document(IPV4.into()).unwrap();
    let live = workspace.submit_run(doc.id, RunMode::Paused).unwrap();
    wait(&workspace, live, |status| status == RunStatus::Paused);
    let queued = workspace.submit_run(doc.id, RunMode::Automatic).unwrap();
    assert_eq!(workspace.stats().input_bytes, IPV4.len());
    assert!(matches!(
        workspace.submit_run(doc.id, RunMode::Automatic),
        Err(WorkspaceError::InputBudget)
    ));
    workspace
        .control_run(queued.id, RunControl::Cancel)
        .unwrap();
    let cancelled = workspace.run_summary(queued.id).unwrap();
    assert_eq!(cancelled.status, RunStatus::Cancelled);
    assert_eq!(cancelled.probe_counts, None);
    assert_eq!(cancelled.stop_limit, None);
    assert_eq!(cancelled.last_event_id, None);
    assert!(matches!(
        workspace.run_report(queued.id),
        Err(WorkspaceError::ReportUnavailable(_))
    ));
    assert_eq!(workspace.stats().input_bytes, 0);
    let next = workspace.submit_run(doc.id, RunMode::Automatic).unwrap();
    workspace.control_run(live.id, RunControl::Cancel).unwrap();
    wait(&workspace, next, RunStatus::is_terminal);
    assert!(matches!(
        workspace.run_summary(queued.id),
        Err(WorkspaceError::UnknownRun(_))
    ));
    assert_eq!(workspace.stats().retained, 1);
}

struct TemporarySave(std::path::PathBuf);

impl TemporarySave {
    fn new() -> Self {
        static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);
        Self(std::env::current_dir().unwrap().join(format!(
            "reliner-save-test-{}-{}.json",
            std::process::id(),
            TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        )))
    }
}

impl Drop for TemporarySave {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

#[test]
fn competing_workspaces_preserve_the_winner_and_can_save_the_other_draft_as() {
    let file = TemporarySave::new();
    fs::write(&file.0, VLAN).unwrap();
    let workspaces = [workers(), workers()];
    let docs = workspaces
        .each_ref()
        .map(|ws| ws.open_file(&file.0).unwrap());
    let edits = [
        workspaces[0]
            .replace_experiment(docs[0].id, docs[0].revision, working_vlan())
            .unwrap(),
        workspaces[1]
            .replace_presentation(docs[1].id, docs[1].revision, Some(r#"{"x":17}"#.into()))
            .unwrap(),
    ];
    let start = std::sync::Barrier::new(2);
    let save = |i: usize| {
        start.wait();
        workspaces[i].save_document_at(edits[i].id, edits[i].revision, &file.0)
    };
    let results = std::thread::scope(|scope| {
        let first = scope.spawn(|| save(0));
        let second = save(1);
        [first.join().unwrap(), second]
    });
    assert_eq!(
        results.iter().filter(|r| r.is_ok()).count(),
        1,
        "{results:?}"
    );
    let loser = results.iter().position(Result::is_err).unwrap();
    assert!(matches!(
        results[loser],
        Err(WorkspaceError::SaveConflict(_))
    ));
    let winner = 1 - loser;
    let ws = &workspaces[loser];
    let edit = &edits[loser];
    let bytes = fs::read(&file.0).unwrap();
    let disk: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(disk["revision"], if winner == 0 { "2" } else { "1" });
    assert_eq!(
        disk["presentation"]["x"],
        if winner == 1 { json!(17) } else { Value::Null }
    );
    assert_eq!(ws.document(edit.id).unwrap().info, *edit);
    assert!(matches!(
        ws.save_and_rerun_at(edit.id, edit.revision, &file.0, RunMode::Automatic),
        Err(WorkspaceError::SaveConflict(_))
    ));
    assert_eq!(ws.stats().compiles_started, 0);
    let copy = TemporarySave::new();
    fs::write(&copy.0, "existing Save As destination").unwrap();
    ws.save_document(edit.id, &copy.0).unwrap();
    ws.save_document(edit.id, &copy.0).unwrap();
    assert_eq!(fs::read(&file.0).unwrap(), bytes);
    // A same-length edit to the newly associated path must invalidate its saved baseline.
    let mut external = fs::read(&copy.0).unwrap();
    let column = external.iter().position(|&byte| byte == b'v').unwrap();
    external[column] = b'w';
    fs::write(&copy.0, &external).unwrap();
    assert!(matches!(
        ws.save_document(edit.id, &copy.0),
        Err(WorkspaceError::SaveConflict(_))
    ));
    assert_eq!(fs::read(&copy.0).unwrap(), external);
    fs::remove_file(&copy.0).unwrap();
    assert!(matches!(
        ws.save_document(edit.id, &copy.0),
        Err(WorkspaceError::SaveConflict(_))
    ));
    assert!(!copy.0.exists());
}

#[test]
#[cfg(windows)]
fn case_only_rename_uses_the_original_content_baseline() {
    let file = TemporarySave::new();
    let renamed = TemporarySave(file.0.with_extension("JSON"));
    fs::write(&file.0, VLAN).unwrap();
    let ws = workers();
    let doc = ws.open_file(&file.0).unwrap();
    let edit = ws
        .replace_presentation(doc.id, doc.revision, Some(r#"{"x":17}"#.into()))
        .unwrap();
    fs::rename(&file.0, &renamed.0).unwrap();
    fs::write(&renamed.0, working_vlan()).unwrap();
    assert!(matches!(
        ws.save_document_at(doc.id, edit.revision, &renamed.0),
        Err(WorkspaceError::SaveConflict(_))
    ));
    assert_eq!(fs::read_to_string(&renamed.0).unwrap(), working_vlan());
    assert_eq!(ws.document(doc.id).unwrap().info, edit);
    fs::write(&renamed.0, VLAN).unwrap();
    assert_eq!(ws.save_document(doc.id, &renamed.0).unwrap(), edit.revision);
}

#[cfg(unix)]
#[test]
fn unix_save_preserves_private_modes_and_rejects_read_only_destinations() {
    use std::os::unix::fs::PermissionsExt;
    let file = TemporarySave::new();
    fs::write(&file.0, VLAN).unwrap();
    let ws = workers();
    let doc = ws.open_file(&file.0).unwrap();
    for mode in [0o600, 0o640] {
        fs::set_permissions(&file.0, fs::Permissions::from_mode(mode)).unwrap();
        ws.save_document(doc.id, &file.0).unwrap();
        assert_eq!(
            fs::metadata(&file.0).unwrap().permissions().mode() & 0o7777,
            mode
        );
    }
    fs::set_permissions(&file.0, fs::Permissions::from_mode(0o400)).unwrap();
    let edit = ws
        .replace_experiment(doc.id, doc.revision, working_vlan())
        .unwrap();
    let result = ws.save_and_rerun_at(doc.id, edit.revision, &file.0, RunMode::Automatic);
    assert!(
        matches!(result, Err(WorkspaceError::Io(ref e)) if e.kind() == std::io::ErrorKind::PermissionDenied)
    );
    assert_eq!(ws.document(doc.id).unwrap().info, edit);
    assert_eq!(fs::read_to_string(&file.0).unwrap(), VLAN);
    assert_eq!(
        fs::metadata(&file.0).unwrap().permissions().mode() & 0o7777,
        0o400
    );

    let other = workers();
    let draft = other.open_document(IPV4.into()).unwrap();
    assert!(matches!(other.save_document(draft.id, &file.0),
        Err(WorkspaceError::Io(ref e)) if e.kind() == std::io::ErrorKind::PermissionDenied));
    assert_eq!(fs::read_to_string(&file.0).unwrap(), VLAN);
}

#[test]
fn redirected_open_path_cannot_overwrite_an_unrelated_file() {
    let file = TemporarySave::new();
    let unrelated = TemporarySave::new();
    fs::write(&file.0, VLAN).unwrap();
    fs::write(&unrelated.0, IPV4).unwrap();
    let ws = workers();
    let doc = ws.open_file(&file.0).unwrap();
    fs::remove_file(&file.0).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&unrelated.0, &file.0).unwrap();
    #[cfg(windows)]
    if let Err(e) = std::os::windows::fs::symlink_file(&unrelated.0, &file.0) {
        if e.raw_os_error() == Some(1314) {
            eprintln!("file symlink creation requires Windows Developer Mode or privilege");
            return;
        }
        panic!("{e}");
    }
    assert!(matches!(
        ws.save_document(doc.id, &file.0),
        Err(WorkspaceError::SaveConflict(_))
    ));
    assert_eq!(fs::read_to_string(&unrelated.0).unwrap(), IPV4);
}

#[test]
fn preview_and_open_agree_at_the_saved_layout_byte_limit() {
    let experiment = serde_json::from_str::<Value>(VLAN).unwrap().to_string();
    let layout = r#"{"x":1}"#;
    let workspace = Workspace::new(WorkspaceConfig {
        bytes_per_document: experiment.len() + layout.len(),
        ..WorkspaceConfig::default()
    })
    .unwrap();
    let file = TemporarySave::new();
    let doc = workspace.open_document(experiment).unwrap();
    workspace
        .replace_presentation(doc.id, doc.revision, Some(layout.into()))
        .unwrap();
    workspace.save_document(doc.id, &file.0).unwrap();
    workspace.close_document(doc.id, false).unwrap();
    let saved = fs::read_to_string(&file.0).unwrap();
    assert_eq!(workspace.preview_file(&file.0).unwrap().text, saved);
    let reopened = workspace.open_file(&file.0).unwrap();
    workspace.close_document(reopened.id, false).unwrap();
    fs::write(&file.0, saved.replace(layout, r#"{"x":10}"#)).unwrap();
    assert!(matches!(
        workspace.preview_file(&file.0),
        Err(WorkspaceError::DocumentTooLarge)
    ));
    assert!(matches!(
        workspace.open_file(&file.0),
        Err(WorkspaceError::DocumentTooLarge)
    ));
}

#[test]
fn paused_initial_limit_releases_the_only_resident_slot() {
    let mut config = WorkspaceConfig {
        workers: 1,
        resident_runs: 1,
        ..WorkspaceConfig::default()
    };
    config.run_limits.pending_events = 1;
    let ws = Workspace::new(config).unwrap();
    let doc = ws.open_document(VLAN.into()).unwrap();
    let first = ws.submit_run(doc.id, RunMode::Paused).unwrap();
    let second = ws.submit_run(doc.id, RunMode::Automatic).unwrap();
    let summary = wait(&ws, first, |s| s == RunStatus::Paused || s.is_terminal());
    assert_eq!(summary.status, RunStatus::Incomplete);
    assert_eq!(summary.stop_limit, Some(1));
    assert_eq!(
        summary.stop_reason,
        Some(reliner::StopReason::ResourceLimit {
            limit: reliner::ResourceLimit::PendingEvents
        })
    );
    assert!(ws.run_report(first.id).unwrap().is_incomplete());
    assert_eq!(
        wait(&ws, second, RunStatus::is_terminal).status,
        RunStatus::Incomplete
    );
    assert_eq!(ws.stats().resident, 0);
}

#[test]
fn saved_revision_reruns_from_its_own_input_and_saving_succeeds_when_queue_is_full() {
    let saved_file = TemporarySave::new();
    let path = &saved_file.0;
    let workspace = workers();
    let doc = workspace.open_document(VLAN.into()).unwrap();
    let first = workspace
        .replace_experiment(doc.id, doc.revision, working_vlan())
        .unwrap();
    let first = workspace
        .replace_presentation(doc.id, first.revision, Some(r#"{"x":10}"#.into()))
        .unwrap();
    let blocking = workspace.submit_run(doc.id, RunMode::Paused).unwrap();
    wait(&workspace, blocking, |status| status == RunStatus::Paused);
    let saved = workspace
        .save_and_rerun(doc.id, path, RunMode::Automatic)
        .unwrap();
    let first_run = saved.rerun.unwrap();
    assert_eq!(saved.revision, first.revision);
    let disk: Value = serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
    assert_eq!(disk["presentation"]["x"], 10);
    assert_eq!(disk["revision"], "2");
    let next = workspace
        .replace_experiment(doc.id, first.revision, VLAN.into())
        .unwrap();
    let saved_again = workspace
        .save_and_rerun(doc.id, path, RunMode::Automatic)
        .unwrap();
    assert_eq!(saved_again.revision, next.revision);
    assert!(matches!(saved_again.rerun, Err(WorkspaceError::QueueFull)));
    assert!(!workspace.document(doc.id).unwrap().info.dirty);
    assert!(matches!(
        workspace.open_file(path),
        Err(WorkspaceError::PathInUse(_))
    ));
    let disk: Value = serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
    assert_eq!(disk["revision"], "1");
    workspace
        .control_run(blocking.id, RunControl::Cancel)
        .unwrap();
    wait(&workspace, first_run, RunStatus::is_terminal);
    let old = serde_json::to_value(&*workspace.run_report(first_run.id).unwrap()).unwrap();
    assert_eq!(old["revision"], "2");
    assert_eq!(old["outcome"]["status"], "reachable");
}

#[test]
fn trace_eviction_keeps_the_report_and_evidence_pages() {
    use reliner::TraceStatus;
    let workspace = Workspace::new(WorkspaceConfig {
        history_trace_entries: 16,
        ..WorkspaceConfig::default()
    })
    .unwrap();
    let mut input: Value = serde_json::from_str(IPV4).unwrap();
    input["scenario"]["trace_limit"] = json!(16);
    let doc = workspace.open_document(input.to_string()).unwrap();
    let first = workspace.submit_run(doc.id, RunMode::Automatic).unwrap();
    wait(&workspace, first, RunStatus::is_terminal);
    let first_counts = workspace
        .run_summary(first.id)
        .unwrap()
        .probe_counts
        .unwrap();
    let first_page = workspace.run_trace_page(first.id, 0, 3).unwrap();
    assert_eq!(first_page.status, Some(TraceStatus::Truncated));
    assert_eq!(first_page.page.entries.len(), 3);
    assert_eq!(first_page.page.next_offset, Some(3));
    let later = workspace.submit_run(doc.id, RunMode::Automatic).unwrap();
    wait(&workspace, later, RunStatus::is_terminal);
    let evicted = workspace.run_trace_page(first.id, 0, 3).unwrap();
    assert_eq!(evicted.status, Some(TraceStatus::Evicted));
    assert!(evicted.page.entries.is_empty());
    assert_eq!(
        workspace.run_summary(first.id).unwrap().probe_counts,
        Some(first_counts)
    );
    let page = workspace.run_evidence_page(first.id, 0, 1).unwrap();
    assert_eq!(page.entries[0].probe_id.as_deref(), Some("first"));
    let rest = workspace
        .run_evidence_page(first.id, page.next_offset.unwrap(), 500)
        .unwrap();
    assert_eq!(rest.entries.len(), 3);
    assert_eq!(rest.next_offset, None);
}

#[test]
fn comparison_links_each_outcome_to_its_own_revision_and_evidence() {
    let workspace = workers();
    let doc = workspace.open_document(VLAN.into()).unwrap();
    let before = workspace.submit_run(doc.id, RunMode::Automatic).unwrap();
    wait(&workspace, before, RunStatus::is_terminal);
    workspace
        .replace_experiment(doc.id, doc.revision, working_vlan())
        .unwrap();
    let after = workspace.submit_run(doc.id, RunMode::Automatic).unwrap();
    wait(&workspace, after, RunStatus::is_terminal);
    let changed = workspace.compare_runs(before.id, after.id).unwrap();
    assert_eq!(changed.baseline.run, before);
    assert_eq!(changed.candidate.run, after);
    assert_eq!(changed.baseline.revision, "1");
    assert_eq!(changed.candidate.revision, "2");
    let pair = &changed.probes[0];
    assert!(pair.same_probe_parameters);
    let reliner::Outcome::Timeout {
        cause: Some(cause), ..
    } = pair.baseline.as_ref().unwrap().outcome
    else {
        panic!("missing original cause");
    };
    let reliner::Outcome::Reachable { evidence_id, .. } = pair.candidate.as_ref().unwrap().outcome
    else {
        panic!("missing corrected result");
    };
    let old = workspace.run_evidence_page(before.id, 0, 128).unwrap();
    let new = workspace.run_evidence_page(after.id, 0, 128).unwrap();
    assert_eq!(
        old.entries
            .iter()
            .find(|fact| fact.evidence.id == cause)
            .unwrap()
            .evidence
            .port
            .port_id,
        "sw2-uplink"
    );
    assert_eq!(
        new.entries
            .iter()
            .find(|fact| fact.evidence.id == evidence_id)
            .unwrap()
            .evidence
            .port
            .port_id,
        "pc-a-eth0"
    );
}

#[test]
fn cancellation_after_a_completed_probe_keeps_its_result() {
    let workspace = workers();
    let doc = workspace.open_document(IPV4.into()).unwrap();
    let accepted = workspace.submit_run(doc.id, RunMode::Paused).unwrap();
    let mut summary = wait(&workspace, accepted, |status| status == RunStatus::Paused);
    while summary.last_event_ns < 1600 {
        workspace
            .control_run(accepted.id, RunControl::Step)
            .unwrap();
        summary = wait(&workspace, accepted, |status| status == RunStatus::Paused);
    }
    workspace
        .control_run(accepted.id, RunControl::Cancel)
        .unwrap();
    let summary = wait(&workspace, accepted, RunStatus::is_terminal);
    assert_eq!(summary.status, RunStatus::Cancelled);
    let summary = serde_json::to_value(summary).unwrap();
    assert_eq!(
        summary["probe_counts"],
        json!({"completed":1,"reachable":1,"timed_out":0,"rejected_before_send":0,"incomplete":1})
    );
    assert!(summary.get("stop_limit").is_none());
    let result = workspace.run_report(accepted.id).unwrap();
    let reliner::RunReport::Scenario(result) = result.as_ref() else {
        panic!("expected scenario report");
    };
    assert!(matches!(
        result.probes[0].outcome,
        reliner::Outcome::Reachable { at_ns: 1600, .. }
    ));
    assert_eq!(
        result.probes[1].outcome,
        reliner::Outcome::Incomplete {
            at_ns: 1600,
            reason: reliner::StopReason::Cancelled
        }
    );
    assert_eq!(result.evidence.len(), 2);
}

#[test]
fn stepped_failover_keeps_its_evidence_when_the_vlan_is_fixed_and_rerun() {
    let mut input: Value =
        serde_json::from_str(include_str!("../examples/failover-lab.json")).unwrap();
    input["scenario"]["link_actions"]
        .as_array_mut()
        .unwrap()
        .extend([
            json!({"link":"inter-switch-standby", "at_ns":"1900", "state":"down"}),
            json!({"link":"inter-switch-standby", "at_ns":"2200", "state":"up"}),
        ]);
    let workspace = workers();
    let doc = workspace.open_document(input.to_string()).unwrap();
    let before = workspace.submit_run(doc.id, RunMode::Paused).unwrap();
    let mut paused = wait(&workspace, before, |status| status == RunStatus::Paused);
    while paused.last_event_ns < 2200 {
        workspace.control_run(before.id, RunControl::Step).unwrap();
        paused = wait(&workspace, before, |status| status == RunStatus::Paused);
    }
    assert_eq!(paused.last_event_ns, 2200);
    workspace
        .control_run(before.id, RunControl::Resume)
        .unwrap();
    wait(&workspace, before, RunStatus::is_terminal);
    let first_page = workspace.run_link_event_page(before.id, 0, 1).unwrap();
    let next = workspace
        .run_link_event_page(before.id, first_page.next_offset.unwrap(), 1)
        .unwrap();
    let failover = &next.entries[0];
    assert_eq!(paused.last_event_id, Some(failover.event_id));
    assert_eq!(
        failover.simultaneous.as_ref().unwrap().state,
        reliner::LinkStatus::Up
    );
    assert_eq!(
        failover
            .protection
            .as_ref()
            .unwrap()
            .active_link_id
            .as_deref(),
        Some("inter-switch-standby")
    );

    input["devices"][2]["ports"][2]["mode"]["allowed_vlans"] = json!([10, 20]);
    input["revision"] = json!("2");
    let edited = workspace
        .replace_experiment(doc.id, doc.revision, input.to_string())
        .unwrap();
    let file = TemporarySave::new();
    let saved = workspace
        .save_and_rerun(doc.id, &file.0, RunMode::Automatic)
        .unwrap();
    let after = saved.rerun.unwrap();
    assert_eq!(after.effective_revision, edited.effective_revision);
    wait(&workspace, after, RunStatus::is_terminal);
    let disk = fs::read_to_string(&file.0).unwrap();
    let from_disk = run_lab_with_limits(&disk, WorkspaceConfig::default().run_limits).unwrap();
    assert_eq!(*workspace.run_report(after.id).unwrap(), from_disk);
    let after_event = workspace.run_link_event_page(after.id, 1, 1).unwrap();
    assert_eq!(&after_event.entries[0], failover);

    let comparison = workspace.compare_runs(before.id, after.id).unwrap();
    assert_eq!(comparison.baseline.revision, "1");
    assert_eq!(comparison.candidate.revision, "2");
    let pair = comparison
        .probes
        .iter()
        .find(|probe| probe.probe_id == "on-standby")
        .unwrap();
    assert!(pair.same_probe_parameters);
    let reliner::Outcome::Timeout {
        cause: Some(cause), ..
    } = pair.baseline.as_ref().unwrap().outcome
    else {
        panic!("the original VLAN drop is missing");
    };
    assert!(matches!(
        pair.candidate.as_ref().unwrap().outcome,
        reliner::Outcome::Reachable { at_ns: 3300, .. }
    ));
    let old = workspace.run_evidence_page(before.id, 0, 128).unwrap();
    let original = &old
        .entries
        .iter()
        .find(|item| item.evidence.id == cause)
        .unwrap()
        .evidence;
    assert_eq!(original.port.port_id, "sw2-standby");
    assert_eq!(
        original.protected_link.as_ref().unwrap().selected_by,
        Some(failover.event_id)
    );
}

#[test]
fn mtu_repair_saves_and_compares_the_same_probe_with_its_original_evidence() {
    let workspace = workers();
    let mut input: Value = serde_json::from_str(include_str!("../examples/mtu-lab.json")).unwrap();
    let doc = workspace.open_document(input.to_string()).unwrap();
    let before = workspace.submit_run(doc.id, RunMode::Automatic).unwrap();
    wait(&workspace, before, RunStatus::is_terminal);

    input["revision"] = json!("2");
    input["devices"][1]["ports"][1]["ipv4_mtu_bytes"] = json!(1500);
    let edited = workspace
        .replace_experiment(doc.id, doc.revision, input.to_string())
        .unwrap();
    let file = TemporarySave::new();
    let saved = workspace
        .save_and_rerun(doc.id, &file.0, RunMode::Automatic)
        .unwrap();
    let after = saved.rerun.unwrap();
    assert_eq!(after.effective_revision, edited.effective_revision);
    wait(&workspace, after, RunStatus::is_terminal);
    let disk = fs::read_to_string(&file.0).unwrap();
    let from_disk = run_lab_with_limits(&disk, WorkspaceConfig::default().run_limits).unwrap();
    assert_eq!(*workspace.run_report(after.id).unwrap(), from_disk);

    let fixed = workspace.compare_runs(before.id, after.id).unwrap();
    assert_eq!(fixed.baseline.revision, "1");
    assert_eq!(fixed.candidate.revision, "2");
    let pair = fixed
        .probes
        .iter()
        .find(|p| p.probe_id == "over-mtu")
        .unwrap();
    assert!(pair.same_probe_parameters);
    let reliner::Outcome::Timeout {
        cause: Some(cause), ..
    } = pair.baseline.as_ref().unwrap().outcome
    else {
        panic!("the original MTU drop is missing");
    };
    assert!(matches!(
        pair.candidate.as_ref().unwrap().outcome,
        reliner::Outcome::Reachable { at_ns: 3600, .. }
    ));
    let original = workspace
        .run_evidence_page(before.id, (cause.0 - 1) as usize, 1)
        .unwrap();
    let evidence = &original.entries[0];
    assert_eq!(evidence.evidence.id, cause);
    assert_eq!(evidence.probe_id.as_deref(), Some("over-mtu"));
    assert_eq!(evidence.evidence.port.port_id, "r1-transit");
    assert_eq!(
        serde_json::to_value(&evidence.evidence.fact).unwrap()["mtu_bytes"],
        1200
    );

    input["revision"] = json!("3");
    input["scenario"]["probes"][2]["payload_bytes"] = json!(1172);
    workspace
        .replace_experiment(doc.id, edited.revision, input.to_string())
        .unwrap();
    let resized = workspace.submit_run(doc.id, RunMode::Automatic).unwrap();
    wait(&workspace, resized, RunStatus::is_terminal);
    let resized = workspace.compare_runs(after.id, resized.id).unwrap();
    let pair = resized
        .probes
        .iter()
        .find(|p| p.probe_id == "over-mtu")
        .unwrap();
    assert!(!pair.same_probe_parameters);
    let pair = serde_json::to_value(pair).unwrap();
    assert_eq!(pair["baseline"]["payload_bytes"], 1173);
    assert_eq!(pair["candidate"]["payload_bytes"], 1172);
}

#[test]
fn congestion_repair_saves_and_compares_the_same_probe_with_its_drop() {
    let workspace = workers();
    let mut input: Value =
        serde_json::from_str(include_str!("../examples/congestion-lab.json")).unwrap();
    let doc = workspace.open_document(input.to_string()).unwrap();
    let before = workspace.submit_run(doc.id, RunMode::Automatic).unwrap();
    wait(&workspace, before, RunStatus::is_terminal);

    input["revision"] = json!("2");
    input["links"][1]["transmission"]["queue_frames"] = json!(2);
    let edited = workspace
        .replace_experiment(doc.id, doc.revision, input.to_string())
        .unwrap();
    let file = TemporarySave::new();
    let after = workspace
        .save_and_rerun(doc.id, &file.0, RunMode::Automatic)
        .unwrap()
        .rerun
        .unwrap();
    assert_eq!(after.effective_revision, edited.effective_revision);
    wait(&workspace, after, RunStatus::is_terminal);
    let disk = fs::read_to_string(&file.0).unwrap();
    let from_disk = run_lab_with_limits(&disk, WorkspaceConfig::default().run_limits).unwrap();
    assert_eq!(*workspace.run_report(after.id).unwrap(), from_disk);

    let compared = workspace.compare_runs(before.id, after.id).unwrap();
    assert_eq!(compared.baseline.revision, "1");
    assert_eq!(compared.candidate.revision, "2");
    let pair = compared
        .probes
        .iter()
        .find(|p| p.probe_id == "burst-third")
        .unwrap();
    assert!(pair.same_probe_parameters);
    let reliner::Outcome::Timeout {
        cause: Some(cause), ..
    } = pair.baseline.as_ref().unwrap().outcome
    else {
        panic!("the original queue drop is missing");
    };
    assert!(matches!(
        pair.candidate.as_ref().unwrap().outcome,
        reliner::Outcome::Reachable { at_ns: 8864, .. }
    ));
    let page = workspace
        .run_evidence_page(before.id, (cause.0 - 1) as usize, 1)
        .unwrap();
    let original = &page.entries[0];
    assert_eq!(original.probe_id.as_deref(), Some("burst-third"));
    assert_eq!(original.evidence.id, cause);
    assert_eq!(original.evidence.port.port_id, "r1-transit");
    assert!(matches!(
        original.evidence.fact,
        reliner::EvidenceFact::TransmitQueueFull {
            queue_frames: 1,
            ..
        }
    ));
}

#[test]
fn routing_repair_keeps_the_return_drop_and_its_convergence_event() {
    let workspace = workers();
    let mut input: Value =
        serde_json::from_str(include_str!("../examples/routing-lab.json")).unwrap();
    let doc = workspace.open_document(input.to_string()).unwrap();
    let before = workspace.submit_run(doc.id, RunMode::Automatic).unwrap();
    wait(&workspace, before, RunStatus::is_terminal);

    input["revision"] = json!("2");
    input["devices"][1]["link_state"]["advertise"] = json!(["r1-lan"]);
    workspace
        .replace_experiment(doc.id, doc.revision, input.to_string())
        .unwrap();
    let file = TemporarySave::new();
    let after = workspace
        .save_and_rerun(doc.id, &file.0, RunMode::Automatic)
        .unwrap()
        .rerun
        .unwrap();
    wait(&workspace, after, RunStatus::is_terminal);
    let disk = fs::read_to_string(&file.0).unwrap();
    let from_disk = run_lab_with_limits(&disk, WorkspaceConfig::default().run_limits).unwrap();
    let mut from_disk = from_disk;
    if let reliner::RunReport::Scenario(report) = &mut from_disk {
        report.trace.entries.clear();
    }
    assert_eq!(*workspace.run_report(after.id).unwrap(), from_disk);

    let comparison = workspace.compare_runs(before.id, after.id).unwrap();
    assert_eq!(comparison.baseline.revision, "1");
    assert_eq!(comparison.candidate.revision, "2");
    let pair = comparison
        .probes
        .iter()
        .find(|p| p.probe_id == "primary")
        .unwrap();
    assert!(pair.same_probe_parameters);
    let baseline = pair.baseline.as_ref().unwrap();
    let reliner::Outcome::Timeout {
        cause: Some(id), ..
    } = baseline.outcome
    else {
        panic!("missing route evidence");
    };
    assert!(matches!(
        pair.candidate.as_ref().unwrap().outcome,
        reliner::Outcome::Reachable { at_ns: 1600, .. }
    ));
    let page = workspace
        .run_evidence_page(before.id, (id.0 - 1) as usize, 1)
        .unwrap();
    let loss = &page.entries[0].evidence;
    assert_eq!(loss.port.port_id, "r3-lan");
    let reliner::EvidenceFact::DynamicRouteMiss {
        convergence_event_id: Some(convergence),
        request_evidence_id,
        ..
    } = loss.fact
    else {
        panic!("missing referenced convergence");
    };
    assert_eq!(request_evidence_id, baseline.request_evidence_id);
    let events = workspace.run_routing_event_page(before.id, 0, 128).unwrap();
    let installed = events
        .entries
        .iter()
        .find(|entry| entry.event_id == convergence)
        .unwrap();
    assert_eq!(installed.router_id, "r3");
    assert_eq!(installed.at_ns, 500);
}

#[test]
fn stp_cost_repair_reopens_the_same_probe_and_preserves_the_fault_tree() {
    let workspace = workers();
    let mut input: Value = serde_json::from_str(include_str!("../examples/stp-lab.json")).unwrap();
    input["scenario"]["trace_limit"] = json!(0);
    let doc = workspace.open_document(input.to_string()).unwrap();
    let before = workspace.submit_run(doc.id, RunMode::Automatic).unwrap();
    wait(&workspace, before, RunStatus::is_terminal);

    input["revision"] = json!("2");
    input["devices"][3]["ports"][0]["stp_cost"] = json!(10);
    workspace
        .replace_experiment(doc.id, doc.revision, input.to_string())
        .unwrap();
    let file = TemporarySave::new();
    let after = workspace
        .save_and_rerun(doc.id, &file.0, RunMode::Automatic)
        .unwrap()
        .rerun
        .unwrap();
    wait(&workspace, after, RunStatus::is_terminal);
    let disk = fs::read_to_string(&file.0).unwrap();
    let from_disk = run_lab_with_limits(&disk, WorkspaceConfig::default().run_limits).unwrap();
    assert_eq!(*workspace.run_report(after.id).unwrap(), from_disk);

    let compared = workspace.compare_runs(before.id, after.id).unwrap();
    assert_eq!(compared.baseline.revision, "1");
    assert_eq!(compared.candidate.revision, "2");
    let pair = compared
        .probes
        .iter()
        .find(|p| p.probe_id == "initial-tree")
        .unwrap();
    assert!(pair.same_probe_parameters);
    let reliner::Outcome::Timeout {
        cause: Some(id), ..
    } = pair.baseline.as_ref().unwrap().outcome
    else {
        panic!("the original VLAN loss is missing");
    };
    assert!(matches!(
        pair.candidate.as_ref().unwrap().outcome,
        reliner::Outcome::Reachable { at_ns: 2300, .. }
    ));
    let evidence = workspace
        .run_evidence_page(before.id, (id.0 - 1) as usize, 1)
        .unwrap();
    let loss = &evidence.entries[0].evidence;
    assert_eq!(loss.port.port_id, "s3-s1");
    let tree = workspace.run_stp_event_page(before.id, 0, 128).unwrap();
    let original = tree
        .entries
        .iter()
        .find(|entry| Some(entry.event_id) == loss.stp_event_id)
        .unwrap();
    let sw3 = original
        .bridges
        .iter()
        .find(|bridge| bridge.bridge_id == "sw3")
        .unwrap();
    assert_eq!(sw3.root_port_id.as_deref(), Some("s3-s1"));
}

#[test]
fn campus_faults_survive_trace_limits_and_keep_the_route_decision() {
    let input = include_str!("../examples/campus-lab.json");
    let doc: Value = serde_json::from_str(input).unwrap();
    let mut baseline = None;
    for trace_limit in [0, 1, 1024] {
        let mut doc = doc.clone();
        doc["scenario"]["trace_limit"] = json!(trace_limit);
        let report = serde_json::to_value(reliner::run_lab(&doc.to_string()).unwrap()).unwrap();
        let evidence = report["evidence"].as_array().unwrap();
        let probe = |id: &str| {
            report["probes"]
                .as_array()
                .unwrap()
                .iter()
                .find(|p| p["id"] == id)
                .unwrap()
        };
        let get = |id: &Value| evidence.iter().find(|e| e["id"] == *id).unwrap();
        let vlan = get(&probe("vlan")["outcome"]["cause"]);
        assert_eq!(vlan["port"]["port_id"], "s3-s1");
        assert_eq!(vlan["fact"]["reason"]["vlan"], 10);
        assert_eq!(vlan["stp_event_id"], report["stp_events"][0]["event_id"]);
        let old = get(&probe("old-route")["outcome"]["cause"]);
        assert_eq!(old["port"]["port_id"], "r1-r3");
        assert_eq!(
            old["fact"]["down_event_id"],
            report["link_events"][1]["event_id"]
        );
        let burst = probe("burst-reply");
        let drop = get(&burst["outcome"]["cause"]);
        assert_eq!(drop["port"]["port_id"], "r2-r1");
        assert_eq!(drop["frame"]["kind"], "echo_reply");
        assert_eq!(drop["fact"]["kind"], "transmit_queue_full");
        assert_eq!(drop["at_ns"], "19228");
        assert_eq!(get(&burst["request_evidence_id"])["at_ns"], "19028");
        for (loss, router, install) in [(old, "r1", "2500"), (drop, "r2", "11500")] {
            let decision = report["routing_events"]
                .as_array()
                .unwrap()
                .iter()
                .find(|e| e["event_id"] == loss["routing_event_id"])
                .unwrap();
            assert_eq!(decision["router_id"], router);
            assert_eq!(decision["at_ns"], install);
        }
        assert!(probe("short-deadline")["outcome"]["cause"].is_null());
        let comparable = (report["probes"].clone(), report["evidence"].clone());
        if let Some(previous) = baseline {
            assert_eq!(comparable, previous);
        }
        baseline = Some(comparable);
    }
}

#[test]
fn campus_repairs_keep_prior_runs_and_save_the_same_probes() {
    let input = include_str!("../examples/campus-lab.json");
    let mut draft: Value = serde_json::from_str(input).unwrap();
    draft["scenario"]["trace_limit"] = json!(0);
    let workspace = workers();
    let mut doc = workspace.open_document(draft.to_string()).unwrap();
    let original = workspace.submit_run(doc.id, RunMode::Automatic).unwrap();
    let mut previous = original;
    let file = TemporarySave::new();
    wait(&workspace, original, RunStatus::is_terminal);
    for (revision, repaired, at_ns) in [
        ("2", "vlan", 4300),
        ("3", "old-route", 13032),
        ("4", "burst-reply", 20544),
    ] {
        draft["revision"] = json!(revision);
        match repaired {
            "vlan" => draft["devices"][3]["ports"][0]["mode"]["allowed_vlans"] = json!([10, 20]),
            "old-route" => draft["scenario"]["routing_convergence_ns"] = json!("1000"),
            "burst-reply" => draft["links"][6]["transmission"]["queue_frames"] = json!(1),
            _ => unreachable!(),
        }
        doc = workspace
            .replace_experiment(doc.id, doc.revision, draft.to_string())
            .unwrap();
        let saved = workspace
            .save_and_rerun(doc.id, &file.0, RunMode::Automatic)
            .unwrap();
        let new = saved.rerun.unwrap();
        assert_eq!(new.effective_revision, doc.effective_revision);
        assert_eq!(
            wait(&workspace, new, RunStatus::is_terminal).status,
            RunStatus::Finished
        );
        let saved_input = fs::read_to_string(&file.0).unwrap();
        let standalone =
            run_lab_with_limits(&saved_input, WorkspaceConfig::default().run_limits).unwrap();
        assert_eq!(*workspace.run_report(new.id).unwrap(), standalone);
        let diff = workspace.compare_runs(previous.id, new.id).unwrap();
        for probe in &diff.probes {
            assert!(probe.same_probe_parameters);
            let before = probe.baseline.as_ref().unwrap().outcome;
            let after = probe.candidate.as_ref().unwrap().outcome;
            let status = |outcome| matches!(outcome, reliner::Outcome::Reachable { .. });
            let changed =
                probe.probe_id == repaired || (repaired == "vlan" && probe.probe_id == "restored");
            assert_eq!(
                status(before) != status(after),
                changed,
                "{}",
                probe.probe_id
            );
            if probe.probe_id == repaired {
                assert!(matches!(
                    before,
                    reliner::Outcome::Timeout { cause: Some(_), .. }
                ));
                assert!(
                    matches!(after, reliner::Outcome::Reachable { at_ns: t, .. } if t == at_ns)
                );
            }
        }
        previous = new;
    }
    let compared = workspace.compare_runs(original.id, previous.id).unwrap();
    let burst = compared
        .probes
        .iter()
        .find(|p| p.probe_id == "burst-reply")
        .unwrap();
    let reliner::Outcome::Timeout {
        cause: Some(cause), ..
    } = burst.baseline.as_ref().unwrap().outcome
    else {
        panic!("original queue evidence is missing");
    };
    let page = workspace
        .run_evidence_page(original.id, (cause.0 - 1) as usize, 1)
        .unwrap();
    let fact = &page.entries[0].evidence;
    assert_eq!(fact.port.port_id, "r2-r1");
    let event = fact.routing_event_id.unwrap();
    let control = workspace
        .run_routing_event_page(original.id, 0, 128)
        .unwrap();
    assert!(
        control
            .entries
            .iter()
            .any(|route| route.event_id == event && route.at_ns == 11500)
    );
}

#[test]
fn campus_cli_names_the_reply_egress_before_the_route_detail() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_reliner"))
        .arg("run")
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/examples/campus-lab.json"
        ))
        .output()
        .unwrap();
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    let burst = text
        .split_once("burst-reply")
        .unwrap()
        .1
        .split_once("restored")
        .unwrap()
        .0;
    let received = burst
        .find("PC-B 在 19028 ns 收到 ICMP Echo Request")
        .unwrap();
    let drop = burst
        .find("ICMP Echo Reply 在 R2/Via R1 出口丢弃（19228 ns）")
        .unwrap();
    let context = burst.find("R2 在 11500 ns 安装的动态路由表").unwrap();
    assert!(received < drop && drop < context);
    assert!(burst.contains("没有等待位置"));
    assert!(text.contains("R1 在 2500 ns 安装的动态路由表"));
}

#[test]
fn campus_one_nanosecond_moves_congestion_from_reply_to_request() {
    let mut doc: Value = serde_json::from_str(include_str!("../examples/campus-lab.json")).unwrap();
    doc["scenario"]["probes"][5]["at_ns"] = json!("17815");
    let reliner::RunReport::Scenario(report) = reliner::run_lab(&doc.to_string()).unwrap() else {
        panic!("expected scenario report");
    };
    let probe = &report.probes[5];
    assert!(probe.request_evidence_id.is_none());
    let reliner::Outcome::Timeout {
        cause: Some(id), ..
    } = probe.outcome
    else {
        panic!("expected request queue loss");
    };
    let drop = report.evidence(id).unwrap();
    assert_eq!(drop.at_ns, 18215);
    assert_eq!(drop.port.port_id, "r1-r2");
    assert_eq!(drop.frame.kind, reliner::FrameKind::EchoRequest);
    assert!(matches!(
        drop.fact,
        reliner::EvidenceFact::TransmitQueueFull { .. }
    ));
}

#[test]
fn campus_tree_commit_invalidates_the_original_reply_at_the_same_time() {
    let mut doc: Value = serde_json::from_str(include_str!("../examples/campus-lab.json")).unwrap();
    doc["scenario"]["link_actions"][3]["at_ns"] = json!("19332");
    let reliner::RunReport::Scenario(report) = reliner::run_lab(&doc.to_string()).unwrap() else {
        panic!("expected scenario report");
    };
    let first = &report.probes[4];
    let reliner::Outcome::Timeout {
        cause: Some(id), ..
    } = first.outcome
    else {
        panic!("expected STP in-flight loss");
    };
    let loss = report.evidence(id).unwrap();
    assert_eq!(loss.frame.kind, reliner::FrameKind::EchoReply);
    assert_eq!(loss.port.port_id, "s2-s3");
    assert_eq!(loss.at_ns, 19832);
    assert!(
        matches!(loss.fact, reliner::EvidenceFact::InFlightStpLoss { invalidated_by, .. }
        if invalidated_by == report.stp_events.get(2).unwrap().event_id)
    );
}

#[test]
fn campus_workspace_stops_before_a_tree_it_cannot_record() {
    let input = include_str!("../examples/campus-lab.json");
    let mut config = WorkspaceConfig::default();
    config.run_limits.essential_facts = 25;
    let expected = run_lab_with_limits(input, config.run_limits).unwrap();
    let reliner::RunReport::Scenario(report) = &expected else {
        panic!("expected scenario report");
    };
    assert_eq!(
        report.stop_reason,
        Some(reliner::StopReason::ResourceLimit {
            limit: reliner::ResourceLimit::EssentialFacts,
        })
    );
    assert_eq!(report.last_event_ns, 4500);
    assert_eq!(report.last_event_id, Some(report.link_events[0].event_id));
    assert_eq!(report.stp_events.len(), 1);
    assert!(matches!(
        report.probes[0].outcome,
        reliner::Outcome::Timeout { cause: Some(_), .. }
    ));
    assert!(
        report.probes[1..]
            .iter()
            .all(|p| matches!(p.outcome, reliner::Outcome::Incomplete { .. }))
    );

    let workspace = Workspace::new(config).unwrap();
    let doc = workspace.open_document(input.into()).unwrap();
    let run = workspace.submit_run(doc.id, RunMode::Automatic).unwrap();
    let done = wait(&workspace, run, RunStatus::is_terminal);
    assert_eq!(done.status, RunStatus::Incomplete);
    assert_eq!(done.last_event_id, report.last_event_id);
    let summary = serde_json::to_value(&done).unwrap();
    assert_eq!(
        summary["probe_counts"],
        json!({"completed":1,"reachable":0,"timed_out":1,"rejected_before_send":0,"incomplete":6})
    );
    assert_eq!(summary["stop_limit"], 25);
    let observed = workspace.run_report(run.id).unwrap();
    let reliner::RunReport::Scenario(observed) = observed.as_ref() else {
        panic!("expected scenario report");
    };
    assert_eq!(observed.probes, report.probes);
    assert_eq!(observed.evidence, report.evidence);
    assert_eq!(observed.stp_events, report.stp_events);
}

#[test]
fn campus_cold_arp_keeps_the_route_that_triggered_resolution() {
    let mut doc: Value = serde_json::from_str(include_str!("../examples/campus-lab.json")).unwrap();
    doc["scenario"]["trace_limit"] = json!(0);
    doc["devices"][3]["ports"][0]["mode"]["allowed_vlans"] = json!([10, 20]);
    doc["scenario"]["warmup"]
        .as_array_mut()
        .unwrap()
        .retain(|warmup| warmup["port"] != "r1-r2");
    for (index, at) in [(4, "29376"), (5, "29576")] {
        doc["scenario"]["probes"][index]["at_ns"] = json!(at);
        doc["scenario"]["probes"][index]["deadline_ns"] = json!("34900");
    }
    for (down, probe, kind, port) in [
        (None, 5, reliner::FrameKind::EchoRequest, "r1-r2"),
        (Some("31000"), 4, reliner::FrameKind::ArpReply, "r1-r2"),
        (Some("31916"), 4, reliner::FrameKind::EchoRequest, "r2-r1"),
    ] {
        let mut draft = doc.clone();
        if let Some(at) = down {
            draft["scenario"]["link_actions"]
                .as_array_mut()
                .unwrap()
                .push(json!({"link":"r1-r2","at_ns":at,"state":"down"}));
        }
        let reliner::RunReport::Scenario(report) = reliner::run_lab(&draft.to_string()).unwrap()
        else {
            panic!("expected scenario report");
        };
        let reliner::Outcome::Timeout {
            cause: Some(id), ..
        } = report.probes[probe].outcome
        else {
            panic!("expected a blocking cause");
        };
        let loss = report.evidence(id).unwrap();
        assert_eq!(loss.frame.kind, kind);
        assert_eq!(loss.port.port_id, port);
        if down.is_some() {
            assert!(matches!(
                loss.fact,
                reliner::EvidenceFact::InFlightLinkLoss { .. }
            ));
        } else {
            assert!(matches!(
                loss.fact,
                reliner::EvidenceFact::TransmitQueueFull { .. }
            ));
        }
        let selected = report
            .routing_events
            .iter()
            .find(|event| Some(event.event_id) == loss.routing_event_id)
            .unwrap();
        assert_eq!(selected.router_id, "r1");
        assert_eq!(selected.at_ns, 11500);
        assert!(
            report
                .routing_events
                .iter()
                .any(|event| event.router_id == "r1" && event.at_ns == 31000)
        );
    }
}

#[test]
fn stp_pages_reconstruct_old_rounds_after_identical_and_changed_publications() {
    let mut doc: Value = serde_json::from_str(include_str!("../examples/stp-lab.json")).unwrap();
    doc["scenario"]["trace_limit"] = json!(0);
    doc["scenario"]["link_actions"][1]["at_ns"] = json!("2800");
    doc["scenario"]["link_actions"]
        .as_array_mut()
        .unwrap()
        .push(json!({
            "link":"s1-s3", "state":"down", "at_ns":"5500"
        }));
    doc["scenario"]["probes"][1]["at_ns"] = json!("2800");
    doc["scenario"]["probes"][2]["at_ns"] = json!("3100");
    let input = doc.to_string();
    let run = |essential_facts| {
        let r = run_lab_with_limits(
            &input,
            reliner::RunLimits {
                essential_facts,
                ..Default::default()
            },
        )
        .unwrap();
        let reliner::RunReport::Scenario(report) = r else {
            panic!("expected scenario")
        };
        report
    };
    let below = run(27);
    let exact = run(28);
    assert_eq!(below.last_event_ns, 5500);
    assert_eq!(below.stp_events.len(), 2);
    assert_eq!(exact.stp_events.len(), 3);
    assert_eq!(exact.stp_events.get(2).unwrap().at_ns, 6000);
    assert_eq!(below.stp_events.get(1), exact.stp_events.get(1));

    let workspace = workers();
    let opened = workspace.open_document(input).unwrap();
    let accepted = workspace.submit_run(opened.id, RunMode::Automatic).unwrap();
    assert_eq!(
        wait(&workspace, accepted, RunStatus::is_terminal).status,
        RunStatus::Finished
    );
    for (index, time, expected_port) in [
        (2, 6000, "s3-s2"),
        (1, 3200, "s3-s1"),
        (0, 500, "s3-s1"),
        (1, 3200, "s3-s1"),
    ] {
        let page = workspace.run_stp_event_page(accepted.id, index, 1).unwrap();
        let restored = &page.entries[0];
        assert_eq!(restored, &exact.stp_events.get(index).unwrap());
        assert_eq!(
            workspace
                .run_stp_event(accepted.id, restored.event_id)
                .unwrap()
                .as_ref(),
            Some(restored)
        );
        assert_eq!(restored.at_ns, time);
        let sw3 = restored
            .bridges
            .iter()
            .find(|bridge| bridge.bridge_id == "sw3")
            .unwrap();
        assert_eq!(sw3.root_port_id.as_deref(), Some(expected_port));
        assert_eq!(page.next_offset, (index < 2).then_some(index + 1));
    }
    assert!(
        workspace
            .run_stp_event_page(accepted.id, 3, 1)
            .unwrap()
            .entries
            .is_empty()
    );
    assert!(
        workspace
            .run_stp_event(accepted.id, reliner::EventId(0))
            .unwrap()
            .is_none()
    );
    let report = workspace.run_report(accepted.id).unwrap();
    let reliner::RunReport::Scenario(report) = report.as_ref() else {
        panic!("expected scenario")
    };
    let reliner::Outcome::Timeout {
        cause: Some(id), ..
    } = report.probes[2].outcome
    else {
        panic!("expected VLAN discard after republishing the same tree");
    };
    assert_eq!(
        report.evidence(id).unwrap().stp_event_id,
        Some(report.stp_events.get(1).unwrap().event_id)
    );
}

#[test]
fn paused_campus_publishes_the_tree_only_at_the_control_event() {
    let input = include_str!("../examples/campus-lab.json");
    let workspace = workers();
    let doc = workspace.open_document(input.into()).unwrap();
    let before_tree_change = || {
        let run = workspace.submit_run(doc.id, RunMode::Paused).unwrap();
        let mut summary = wait(&workspace, run, |status| status == RunStatus::Paused);
        while summary.last_event_ns < 4500 {
            workspace.control_run(run.id, RunControl::Step).unwrap();
            summary = wait(&workspace, run, |status| status == RunStatus::Paused);
        }
        assert_eq!(summary.last_event_ns, 4500);
        run
    };
    let cancelled = before_tree_change();
    workspace
        .control_run(cancelled.id, RunControl::Cancel)
        .unwrap();
    let stopped = wait(&workspace, cancelled, RunStatus::is_terminal);
    assert_eq!(stopped.status, RunStatus::Cancelled);
    assert_eq!(stopped.last_event_ns, 4500);
    let old_tree = workspace.run_stp_event_page(cancelled.id, 0, 3).unwrap();
    assert_eq!(
        old_tree
            .entries
            .iter()
            .map(|event| event.at_ns)
            .collect::<Vec<_>>(),
        [500]
    );

    let resumed = before_tree_change();
    workspace.control_run(resumed.id, RunControl::Step).unwrap();
    let published = wait(&workspace, resumed, |status| status == RunStatus::Paused);
    assert_eq!(published.last_event_ns, 5000);
    workspace
        .control_run(resumed.id, RunControl::Resume)
        .unwrap();
    let done = wait(&workspace, resumed, RunStatus::is_terminal);
    assert_eq!(done.status, RunStatus::Finished);
    let reference = run_lab_with_limits(input, WorkspaceConfig::default().run_limits).unwrap();
    let reliner::RunReport::Scenario(reference) = reference else {
        panic!("expected scenario")
    };
    let r = workspace.run_report(resumed.id).unwrap();
    let reliner::RunReport::Scenario(actual) = r.as_ref() else {
        panic!("expected scenario")
    };
    assert_eq!(actual.stp_events, reference.stp_events);
    assert_eq!(actual.evidence, reference.evidence);
    assert_eq!(actual.probes, reference.probes);
    assert_eq!(actual.last_event_id, reference.last_event_id);
}

#[test]
fn a_paused_routing_round_steps_one_router_then_keeps_the_same_budget_stop() {
    let mut doc: Value =
        serde_json::from_str(include_str!("../examples/routing-lab.json")).unwrap();
    doc["scenario"]["warmup"] = json!([]);
    doc["scenario"]["probes"].as_array_mut().unwrap().remove(0);
    let input = doc.to_string();
    let config = WorkspaceConfig {
        batch_work: 1,
        run_limits: reliner::RunLimits {
            event_work: 34,
            total_work: 68,
            ..WorkspaceConfig::default().run_limits
        },
        ..WorkspaceConfig::default()
    };
    let mut reference = run_lab_with_limits(&input, config.run_limits).unwrap();
    let reliner::RunReport::Scenario(expected) = &mut reference else {
        unreachable!("the fixture uses format 2")
    };
    assert_eq!(expected.routing_events.len(), 2);

    let workspace = Workspace::new(config).unwrap();
    let doc = workspace.open_document(input).unwrap();
    let run = workspace.submit_run(doc.id, RunMode::Paused).unwrap();
    wait(&workspace, run, |status| status == RunStatus::Paused);
    workspace.control_run(run.id, RunControl::Step).unwrap();
    let first = wait(&workspace, run, |status| status == RunStatus::Paused);
    assert_eq!(
        first.last_event_id,
        Some(expected.routing_events[0].event_id)
    );
    assert_eq!(first.last_event_ns, 500);

    workspace.control_run(run.id, RunControl::Resume).unwrap();
    let stopped = wait(&workspace, run, RunStatus::is_terminal);
    assert_eq!(stopped.last_event_ns, first.last_event_ns);
    assert_eq!(
        stopped.last_event_id,
        Some(expected.routing_events[1].event_id)
    );
    let trace = workspace.run_trace_page(run.id, 0, 3).unwrap();
    assert_eq!(trace.page.entries, expected.trace.entries);
    expected.trace.entries.clear();
    assert_eq!(*workspace.run_report(run.id).unwrap(), reference);
}

#[test]
fn saved_native_formats_keep_their_own_semantics_after_reopening() {
    for (input, version) in [(VLAN, 1), (IPV4, 2)] {
        let saved = TemporarySave::new();
        {
            let workspace = workers();
            let doc = workspace.open_document(input.into()).unwrap();
            let layout = r#"{"viewport":{"x":10},"custom":{"label":"保留"}}"#;
            let doc = workspace
                .replace_presentation(doc.id, doc.revision, Some(layout.into()))
                .unwrap();
            workspace.save_document(doc.id, &saved.0).unwrap();
        }
        let text = fs::read_to_string(&saved.0).unwrap();
        let raw: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(raw["format_version"], version);
        assert_eq!(raw["presentation"]["custom"]["label"], "保留");
        let workspace = workers();
        let doc = workspace.open_file(&saved.0).unwrap();
        assert_eq!(doc.format_version, version);
        let checked = workspace.validate_document(doc.id, doc.revision).unwrap();
        assert!(matches!(
            checked.result,
            reliner::ValidationResult::Valid { .. }
        ));
        let run = workspace.submit_run(doc.id, RunMode::Automatic).unwrap();
        wait(&workspace, run, RunStatus::is_terminal);
        let mut reference =
            run_lab_with_limits(&text, WorkspaceConfig::default().run_limits).unwrap();
        if let reliner::RunReport::Scenario(report) = &mut reference {
            let trace = workspace.run_trace_page(run.id, 0, 128).unwrap();
            assert_eq!(trace.page.entries, report.trace.entries);
            report.trace.entries.clear();
        }
        assert_eq!(*workspace.run_report(run.id).unwrap(), reference);
    }
}

#[test]
fn archive_export_keeps_the_submitted_revision_after_later_document_edits() {
    let workspace = workers();
    let mut input: Value = serde_json::from_str(IPV4).unwrap();
    input["scenario"]["trace_limit"] = json!(128);
    let doc = workspace.open_document(input.to_string()).unwrap();
    let submitted = workspace
        .replace_presentation(doc.id, doc.revision, Some("{}".into()))
        .unwrap();
    let run = workspace.submit_run(doc.id, RunMode::Automatic).unwrap();
    wait(&workspace, run, RunStatus::is_terminal);
    let changed = workspace
        .replace_presentation(doc.id, submitted.revision, Some(r#"{"x":11}"#.into()))
        .unwrap();
    assert_ne!(changed.revision, submitted.revision);
    let mut bytes = Vec::new();
    workspace.write_run_archive(run.id, &mut bytes).unwrap();
    let saved = reliner::read_archive(bytes.as_slice()).unwrap();
    let source = saved.source.as_ref().unwrap();
    assert_eq!(source.run_id, run.id.0.to_string());
    assert_eq!(source.document_revision, submitted.revision.0.to_string());
    assert_eq!(
        source.effective_revision,
        run.effective_revision.0.to_string()
    );
    assert!(!saved.trace_page(0, 128).page.entries.is_empty());
    drop(workspace);
    let next_session = workers();
    let next_doc = next_session.open_document(VLAN.into()).unwrap();
    let next_run = next_session
        .submit_run(next_doc.id, RunMode::Automatic)
        .unwrap();
    wait(&next_session, next_run, RunStatus::is_terminal);
    assert_eq!(next_run.id, run.id);
    assert_eq!(
        reliner::read_archive(bytes.as_slice())
            .unwrap()
            .summary()
            .topology_id,
        "ipv4-lab"
    );
}

#[test]
fn an_archive_keeps_facts_when_workspace_trace_has_been_evicted() {
    let workspace = Workspace::new(WorkspaceConfig {
        history_trace_entries: 1,
        ..WorkspaceConfig::default()
    })
    .unwrap();
    let mut input: Value = serde_json::from_str(IPV4).unwrap();
    input["scenario"]["trace_limit"] = json!(128);
    let doc = workspace.open_document(input.to_string()).unwrap();
    let run = workspace.submit_run(doc.id, RunMode::Automatic).unwrap();
    let stopped = wait(&workspace, run, RunStatus::is_terminal);
    assert_eq!(stopped.trace_status, Some(reliner::TraceStatus::Evicted));
    let mut bytes = Vec::new();
    workspace.write_run_archive(run.id, &mut bytes).unwrap();
    let saved = reliner::read_archive(bytes.as_slice()).unwrap();
    assert_eq!(
        saved.trace_page(0, 128).status,
        Some(reliner::TraceStatus::Evicted)
    );
    assert!(!saved.evidence_page(0, 128).entries.is_empty());
    assert_eq!(saved.summary().probe_counts.completed, 2);
}

#[test]
fn cli_archives_reopen_in_a_new_process_and_never_overwrite_an_existing_file() {
    let saved = TemporarySave::new();
    let binary = env!("CARGO_BIN_EXE_reliner");
    let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/stp-lab.json");
    let archive = std::process::Command::new(binary)
        .arg("archive")
        .arg(&source)
        .arg(&saved.0)
        .output()
        .unwrap();
    assert!(
        archive.status.success(),
        "{}",
        String::from_utf8_lossy(&archive.stderr)
    );
    let original = fs::read(&saved.0).unwrap();
    let opened = std::process::Command::new(binary)
        .arg("report")
        .arg(&saved.0)
        .arg("--json")
        .output()
        .unwrap();
    assert!(
        opened.status.success(),
        "{}",
        String::from_utf8_lossy(&opened.stderr)
    );
    let value: Value = serde_json::from_slice(&opened.stdout).unwrap();
    assert_eq!(value["archive_version"], 1);
    assert_eq!(value["report"]["topology_id"], "stp-lab");
    assert_eq!(value["report"]["stp_events"].as_array().unwrap().len(), 3);
    let repeated = std::process::Command::new(binary)
        .arg("archive")
        .arg(&source)
        .arg(&saved.0)
        .output()
        .unwrap();
    assert!(!repeated.status.success());
    assert_eq!(fs::read(&saved.0).unwrap(), original);
    let recorded = std::process::Command::new(binary)
        .arg("report")
        .arg(&saved.0)
        .output()
        .unwrap();
    let direct = std::process::Command::new(binary)
        .arg("run")
        .arg(&source)
        .output()
        .unwrap();
    assert!(recorded.status.success());
    assert_eq!(recorded.stdout, direct.stdout);
}

#[test]
fn stale_desktop_commands_cannot_submit_save_or_close_a_newer_revision() {
    let workspace = workers();
    let doc = workspace.open_document(IPV4.into()).unwrap();
    let current = workspace
        .replace_presentation(doc.id, doc.revision, Some("{}".into()))
        .unwrap();
    let file = TemporarySave::new();
    assert!(
        matches!(workspace.submit_run_at(doc.id, doc.revision, RunMode::Automatic), Err(WorkspaceError::StaleEdit { actual }) if actual == current.revision)
    );
    assert!(matches!(
        workspace.save_document_at(doc.id, doc.revision, &file.0),
        Err(WorkspaceError::StaleEdit { .. })
    ));
    assert!(matches!(
        workspace.save_and_rerun_at(doc.id, doc.revision, &file.0, RunMode::Automatic),
        Err(WorkspaceError::StaleEdit { .. })
    ));
    assert!(matches!(
        workspace.close_document_at(doc.id, doc.revision, true),
        Err(WorkspaceError::StaleEdit { .. })
    ));
    assert!(!file.0.exists());
    assert_eq!(workspace.document(doc.id).unwrap().info, current);
    assert_eq!(workspace.stats().queued, 0);
    assert_eq!(workspace.stats().compiles_started, 0);
}

#[test]
fn run_origin_remains_available_when_the_document_is_closed() {
    let workspace = workers();
    let doc = workspace.open_document(IPV4.into()).unwrap();
    let submitted = workspace
        .replace_presentation(doc.id, doc.revision, Some("{}".into()))
        .unwrap();
    let run = workspace
        .submit_run_at(doc.id, submitted.revision, RunMode::Paused)
        .unwrap();
    wait(&workspace, run, |status| status == RunStatus::Paused);
    workspace
        .close_document_at(doc.id, submitted.revision, true)
        .unwrap();
    let summary = workspace.run_summary(run.id).unwrap();
    assert_eq!(summary.origin.document_revision, submitted.revision);
    assert_eq!(summary.origin.effective_revision, doc.effective_revision);
    assert_eq!(summary.origin.topology_id, "ipv4-lab");
    assert_eq!(summary.origin.native_revision, "1");
    workspace.control_run(run.id, RunControl::Cancel).unwrap();
    wait(&workspace, run, RunStatus::is_terminal);
}
