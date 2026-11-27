use std::io::{self, Read, Write};

use reliner::{
    ArchiveError, ArchiveSource, ArchivedRun, EventId, EvidenceId, FrameId, Outcome, ResourceLimit,
    RunLimits, RunReport, RunStatus, StopReason, StpHistory, TraceReport, TraceStatus,
    read_archive, run_lab, run_lab_with_limits, write_archive,
};
use serde_json::{Value, json};

const LABS: [&str; 8] = [
    include_str!("../examples/vlan-lab.json"),
    include_str!("../examples/ipv4-lab.json"),
    include_str!("../examples/failover-lab.json"),
    include_str!("../examples/mtu-lab.json"),
    include_str!("../examples/congestion-lab.json"),
    include_str!("../examples/routing-lab.json"),
    include_str!("../examples/stp-lab.json"),
    include_str!("../examples/campus-lab.json"),
];

#[test]
fn legacy_reports_reject_new_import_semantics() {
    let report = run_lab(LABS[0]).unwrap();
    let original = json!({"archive_version":1,"report":report});
    let mut modified = original.clone();
    modified["report"]["outcome"] = json!({
        "status":"undetermined", "modeled": original["report"]["outcome"],
        "coverage":{"source_component":"1","destination_component":"1"}
    });
    assert!(archive_error(&modified).to_string().contains("未判定"));
    let mut modified = original;
    modified["report"]["evidence"][0]["model_only"] = json!(true);
    assert!(archive_error(&modified).to_string().contains("model_only"));
}

fn source(revision: &str) -> ArchiveSource {
    ArchiveSource {
        run_id: "1".into(),
        document_revision: revision.into(),
        effective_revision: revision.into(),
    }
}

fn archive(report: &RunReport, trace: Option<&TraceReport>) -> Vec<u8> {
    let mut bytes = Vec::new();
    write_archive(report, trace, Some(&source("1")), &mut bytes).unwrap();
    bytes
}

fn reopen(value: &Value) -> Result<ArchivedRun, ArchiveError> {
    read_archive(serde_json::to_vec(value).unwrap().as_slice())
}

fn archive_error(value: &Value) -> ArchiveError {
    match reopen(value) {
        Err(error) => error,
        Ok(_) => panic!("损坏的归档未被拒绝"),
    }
}

#[test]
fn every_existing_report_round_trips_without_changing_its_json_bytes() {
    for input in LABS {
        let report = run_lab(input).unwrap();
        let bytes = serde_json::to_vec(&report).unwrap();
        let restored: RunReport = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(serde_json::to_vec(&restored).unwrap(), bytes);
        assert_eq!(restored, report);
    }
}

#[test]
fn archive_bytes_and_report_facts_survive_reopening_all_existing_examples() {
    for input in LABS {
        let report = run_lab(input).unwrap();
        let bytes = archive(&report, None);
        let restored = read_archive(bytes.as_slice()).unwrap();
        assert_eq!(restored.report, report);
        assert_eq!(restored.source.as_ref().unwrap(), &source("1"));
        let mut rewritten = Vec::new();
        write_archive(
            &restored.report,
            restored.trace.as_ref(),
            restored.source.as_ref(),
            &mut rewritten,
        )
        .unwrap();
        assert_eq!(rewritten, bytes);
    }
}

#[test]
fn decimal_identifiers_reject_numbers_signs_whitespace_and_overflow() {
    for invalid in [
        json!(1),
        json!(null),
        json!(""),
        json!("+1"),
        json!("-1"),
        json!(" 1"),
        json!("1 "),
        json!("1.0"),
        json!("1e2"),
        json!("18446744073709551616"),
    ] {
        assert!(serde_json::from_value::<EvidenceId>(invalid.clone()).is_err());
        assert!(serde_json::from_value::<EventId>(invalid.clone()).is_err());
        assert!(serde_json::from_value::<FrameId>(invalid).is_err());
    }
    assert_eq!(
        serde_json::from_value::<EventId>(json!("18446744073709551615")).unwrap(),
        EventId(u64::MAX)
    );
    let mut report = serde_json::to_value(run_lab(LABS[7]).unwrap()).unwrap();
    report["evidence"][0]["at_ns"] = json!(1700);
    assert!(serde_json::from_value::<RunReport>(report).is_err());
}

#[test]
fn expanded_stp_history_preserves_unchanged_rounds_and_stable_slots() {
    let report = serde_json::to_value(run_lab(LABS[6]).unwrap()).unwrap();
    let mut events = report["stp_events"].as_array().unwrap().clone();
    let mut identical = events[0].clone();
    identical["event_id"] = json!("999999");
    identical["at_ns"] = json!("501");
    events.insert(1, identical);
    let bytes = serde_json::to_vec(&events).unwrap();
    let history: StpHistory = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(history.len(), 4);
    assert_eq!(
        history.get(0).unwrap().bridges,
        history.get(1).unwrap().bridges
    );
    assert_eq!(history.by_event_id(EventId(999999)).unwrap().at_ns, 501);
    assert_eq!(serde_json::to_value(&history).unwrap(), json!(events));

    let mut reordered = events.clone();
    reordered[2]["bridges"].as_array_mut().unwrap().reverse();
    reordered[2]["links"].as_array_mut().unwrap().reverse();
    let stable: StpHistory = serde_json::from_value(json!(reordered)).unwrap();
    assert_eq!(serde_json::to_value(&stable).unwrap(), json!(events));

    events[2]["links"].as_array_mut().unwrap().pop();
    assert!(serde_json::from_value::<StpHistory>(json!(events)).is_err());
}

#[test]
fn separated_trace_and_embedded_trace_have_the_same_pages() {
    let embedded = run_lab(LABS[7]).unwrap();
    let original = read_archive(archive(&embedded, None).as_slice()).unwrap();
    let mut separated = run_lab(LABS[7]).unwrap();
    let RunReport::Scenario(report) = &mut separated else {
        panic!("scenario")
    };
    let trace = TraceReport {
        status: report.trace.status,
        entries: std::mem::take(&mut report.trace.entries),
    };
    assert!(!trace.entries.is_empty());
    let reopened = read_archive(archive(&separated, Some(&trace)).as_slice()).unwrap();
    assert_eq!(reopened.trace_page(0, 128), original.trace_page(0, 128));
    assert_eq!(reopened.summary().probe_counts, embedded.probe_counts());
    assert_eq!(reopened.summary().trace_status, Some(trace.status));
    assert_eq!(
        reopened.evidence_page(0, 128),
        original.evidence_page(0, 128)
    );
    assert_eq!(
        reopened.link_event_page(0, 128),
        original.link_event_page(0, 128)
    );
    assert_eq!(
        reopened.routing_event_page(0, 128),
        original.routing_event_page(0, 128)
    );
    assert_eq!(
        reopened.stp_event_page(0, 128),
        original.stp_event_page(0, 128)
    );
    let event = reopened.stp_event_page(0, 1).entries.pop().unwrap();
    assert_eq!(reopened.stp_event(event.event_id), Some(event));
    assert_eq!(reopened.stp_event(EventId(0)), None);
    assert!(
        reopened
            .stp_event_page(usize::MAX, usize::MAX)
            .entries
            .is_empty()
    );
}

#[test]
fn trace_status_is_preserved_unless_an_explicit_sidecar_overrides_it() {
    let mut input: Value = serde_json::from_str(LABS[7]).unwrap();
    input["scenario"]["trace_limit"] = json!(0);
    let disabled = run_lab(&input.to_string()).unwrap();
    let reopened = read_archive(archive(&disabled, None).as_slice()).unwrap();
    assert_eq!(
        reopened.trace_page(0, 1).status,
        Some(TraceStatus::Disabled)
    );

    let mut missing = run_lab(LABS[7]).unwrap();
    let RunReport::Scenario(report) = &mut missing else {
        panic!("scenario")
    };
    let original_status = report.trace.status;
    report.trace.entries.clear();
    let reopened = read_archive(archive(&missing, None).as_slice()).unwrap();
    assert_eq!(reopened.summary().trace_status, Some(original_status));
    assert!(reopened.trace_page(0, 128).page.entries.is_empty());

    let evicted = TraceReport {
        status: TraceStatus::Evicted,
        entries: Vec::new(),
    };
    let reopened = read_archive(archive(&missing, Some(&evicted)).as_slice()).unwrap();
    assert_eq!(reopened.trace_page(0, 1).status, Some(TraceStatus::Evicted));
}

#[test]
fn zero_trace_budget_is_truncated_and_never_inferred_as_evicted() {
    let report = run_lab_with_limits(
        LABS[7],
        RunLimits {
            trace_entries: 0,
            ..Default::default()
        },
    )
    .unwrap();
    let RunReport::Scenario(scenario) = &report else {
        panic!("scenario")
    };
    assert!(scenario.last_event_id.is_some());
    assert!(scenario.trace.entries.is_empty());
    assert_eq!(scenario.trace.status, TraceStatus::Truncated);
    let reopened = read_archive(archive(&report, None).as_slice()).unwrap();
    assert_eq!(
        reopened.summary().trace_status,
        Some(TraceStatus::Truncated)
    );
    assert_eq!(
        reopened.trace_page(0, 128).status,
        Some(TraceStatus::Truncated)
    );
    assert!(reopened.trace_page(0, 128).page.entries.is_empty());
}

#[test]
fn source_decimal_fields_are_validated_on_both_read_and_write() {
    let report = run_lab(LABS[0]).unwrap();
    let baseline: Value = serde_json::from_slice(&archive(&report, None)).unwrap();
    for field in ["run_id", "document_revision", "effective_revision"] {
        for invalid in [
            "",
            "+1",
            "-1",
            " 1",
            "1 ",
            "1.0",
            "1e2",
            "18446744073709551616",
        ] {
            let mut value = baseline.clone();
            value["source"][field] = json!(invalid);
            let error = archive_error(&value);
            assert!(
                error.to_string().contains(field),
                "来源错误未包含字段 {field}"
            );
            let source: ArchiveSource = serde_json::from_value(value["source"].clone()).unwrap();
            let mut bytes = Vec::new();
            let error = write_archive(&report, None, Some(&source), &mut bytes);
            assert!(
                matches!(error, Err(ArchiveError::InvalidReport(_))),
                "非法来源未被拒绝: {field}"
            );
            assert!(bytes.is_empty());
        }
        let mut value = baseline.clone();
        value["source"][field] = json!(1);
        assert!(reopen(&value).is_err(), "数值来源字段未被拒绝: {field}");
    }
    let valid = ArchiveSource {
        run_id: "18446744073709551615".into(),
        document_revision: "18446744073709551615".into(),
        effective_revision: "0007".into(),
    };
    let mut bytes = Vec::new();
    write_archive(&report, None, Some(&valid), &mut bytes).unwrap();
    assert_eq!(
        read_archive(bytes.as_slice()).unwrap().source.as_ref(),
        Some(&valid)
    );
}

#[test]
fn dropped_frame_parent_is_checked_using_the_recorded_model_version() {
    let mut doc: Value = serde_json::from_str(LABS[1]).unwrap();
    doc["scenario"]["link_actions"] = json!([{"link":"r1-r2","at_ns":"1050","state":"down"}]);
    let report = run_lab(&doc.to_string()).unwrap();
    let mut value: Value = serde_json::from_slice(&archive(&report, None)).unwrap();
    assert_eq!(value["report"]["model_version"], 14);
    let row = value["report"]["evidence"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|row| row["fact"]["kind"] == "link_down")
        .unwrap();
    assert_ne!(row["frame"]["parent_id"], row["frame"]["id"]);
    row["frame"]["parent_id"] = row["frame"]["id"].clone();
    assert!(
        archive_error(&value)
            .to_string()
            .contains("/frame/parent_id")
    );

    // Older models used the incoming frame ID for a synchronous outgoing drop.
    value["report"]["model_version"] = json!(2);
    assert!(reopen(&value).is_ok());
}

#[test]
fn a_timeout_cannot_cite_successful_request_delivery_as_its_cause() {
    let report = run_lab(include_str!("../examples/acl-lab.json")).unwrap();
    let mut value: Value = serde_json::from_slice(&archive(&report, None)).unwrap();
    let probe = &mut value["report"]["probes"][0];
    let delivery = probe["request_evidence_id"].as_str().unwrap().to_owned();
    probe["outcome"]["cause"] = json!(delivery);
    assert!(
        archive_error(&value)
            .to_string()
            .contains("/probes/0/outcome/cause")
    );
}

#[test]
fn missing_cause_and_cross_probe_evidence_references_are_rejected() {
    let report = run_lab(LABS[7]).unwrap();
    let mut value: Value = serde_json::from_slice(&archive(&report, None)).unwrap();
    value["report"]["probes"][0]["outcome"]["cause"] = json!("999999");
    let error = archive_error(&value).to_string();
    assert!(
        error.contains("cause") && error.contains("999999"),
        "{error}"
    );

    let mut value: Value = serde_json::from_slice(&archive(&report, None)).unwrap();
    let evidence = value["report"]["evidence"].as_array().unwrap();
    let other = evidence
        .iter()
        .find(|fact| fact["probe_id"] != "vlan")
        .unwrap()["id"]
        .clone();
    value["report"]["probes"][0]["outcome"]["cause"] = other;
    assert!(reopen(&value).is_err());
}

#[test]
fn missing_stp_routing_and_link_event_targets_are_rejected() {
    for field in ["stp_event_id", "routing_event_id"] {
        let report = run_lab(LABS[7]).unwrap();
        let mut value: Value = serde_json::from_slice(&archive(&report, None)).unwrap();
        let fact = value["report"]["evidence"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|fact| fact.get(field).is_some())
            .unwrap();
        fact[field] = json!("999999");
        let error = archive_error(&value).to_string();
        assert!(error.contains(field) && error.contains("999999"), "{error}");
    }
    let report = run_lab(LABS[2]).unwrap();
    let mut value: Value = serde_json::from_slice(&archive(&report, None)).unwrap();
    let fact = value["report"]["evidence"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|fact| fact["fact"]["kind"] == "in_flight_link_loss")
        .unwrap();
    fact["fact"]["invalidated_by"] = json!("999999");
    assert!(archive_error(&value).to_string().contains("invalidated_by"));
}

#[test]
fn runtime_event_ids_do_not_require_optional_trace_but_learned_routes_require_their_table() {
    let mut input: Value = serde_json::from_str(LABS[7]).unwrap();
    input["scenario"]["trace_limit"] = json!(0);
    let report = run_lab(&input.to_string()).unwrap();
    let reopened = read_archive(archive(&report, None).as_slice()).unwrap();
    assert_eq!(reopened.report, report);

    let report = run_lab(LABS[7]).unwrap();
    let mut value: Value = serde_json::from_slice(&archive(&report, None)).unwrap();
    let entry = value["report"]["trace"]["entries"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|entry| entry["route"]["learned"].is_object())
        .unwrap();
    entry["route"]["learned"]["convergence_event_id"] = json!("999999");
    assert!(
        archive_error(&value)
            .to_string()
            .contains("convergence_event_id")
    );
}

#[test]
fn unrelated_archives_can_keep_the_same_original_run_id() {
    let before = run_lab(LABS[0]).unwrap();
    let mut input: Value = serde_json::from_str(LABS[0]).unwrap();
    input["revision"] = json!("2");
    input["devices"][2]["ports"][0]["mode"]["allowed_vlans"] = json!([10, 20]);
    let after = run_lab(&input.to_string()).unwrap();
    let mut old_bytes = Vec::new();
    let mut new_bytes = Vec::new();
    write_archive(&before, None, Some(&source("1")), &mut old_bytes).unwrap();
    write_archive(&after, None, Some(&source("2")), &mut new_bytes).unwrap();
    let old = read_archive(old_bytes.as_slice()).unwrap();
    let new = read_archive(new_bytes.as_slice()).unwrap();
    assert_eq!(
        old.source.as_ref().unwrap().run_id,
        new.source.as_ref().unwrap().run_id
    );
    assert_eq!(old.summary().probe_counts.timed_out, 1);
    assert_eq!(new.summary().probe_counts.reachable, 1);
    let RunReport::Legacy(report) = &old.report else {
        panic!("legacy")
    };
    assert!(matches!(
        report.outcome,
        Outcome::Timeout { cause: Some(_), .. }
    ));
    assert_eq!(old.source.as_ref().unwrap().effective_revision, "1");
    assert_eq!(new.source.as_ref().unwrap().effective_revision, "2");
    assert_eq!(
        old.evidence_page(0, 128)
            .entries
            .last()
            .unwrap()
            .evidence
            .port
            .device_id,
        "sw2"
    );
}

#[test]
fn submitted_document_and_effective_revisions_remain_distinct_from_native_revision() {
    let mut input: Value = serde_json::from_str(LABS[7]).unwrap();
    input["revision"] = json!("native-revision-A");
    let report = run_lab(&input.to_string()).unwrap();
    let original = ArchiveSource {
        run_id: "18446744073709551615".into(),
        document_revision: "19".into(),
        effective_revision: "7".into(),
    };
    let mut bytes = Vec::new();
    write_archive(&report, None, Some(&original), &mut bytes).unwrap();
    let reopened = read_archive(bytes.as_slice()).unwrap();
    assert_eq!(reopened.source.as_ref(), Some(&original));
    assert_eq!(reopened.summary().revision, "native-revision-A");
    assert_eq!(reopened.report, report);

    let mut bytes = Vec::new();
    write_archive(&report, None, None, &mut bytes).unwrap();
    assert_eq!(read_archive(bytes.as_slice()).unwrap().source, None);
}

#[test]
fn an_evicted_sidecar_retains_the_submitted_source_and_all_essential_evidence() {
    let mut report = run_lab(LABS[7]).unwrap();
    let original = serde_json::to_value(&report).unwrap();
    let RunReport::Scenario(scenario) = &mut report else {
        panic!("scenario")
    };
    scenario.trace.entries.clear();
    let evicted = TraceReport {
        status: TraceStatus::Evicted,
        entries: Vec::new(),
    };
    let submitted = ArchiveSource {
        run_id: "1".into(),
        document_revision: "9".into(),
        effective_revision: "4".into(),
    };
    let mut bytes = Vec::new();
    write_archive(&report, Some(&evicted), Some(&submitted), &mut bytes).unwrap();
    let reopened = read_archive(bytes.as_slice()).unwrap();
    assert_eq!(reopened.source, Some(submitted));
    assert_eq!(reopened.trace.as_ref(), Some(&evicted));
    assert_eq!(reopened.summary().trace_status, Some(TraceStatus::Evicted));
    assert!(reopened.trace_page(0, 128).page.entries.is_empty());
    let restored = serde_json::to_value(&reopened.report).unwrap();
    for field in [
        "probes",
        "evidence",
        "link_events",
        "routing_events",
        "stp_events",
    ] {
        assert_eq!(restored[field], original[field]);
    }
}

#[test]
fn request_evidence_cannot_come_after_the_outcome_that_references_it() {
    let report = run_lab(LABS[7]).unwrap();
    let mut value: Value = serde_json::from_slice(&archive(&report, None)).unwrap();
    let probe = value["report"]["probes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|probe| probe["id"] == "primary")
        .unwrap();
    let request = probe["request_evidence_id"].clone();
    let outcome_ns = probe["outcome"]["at_ns"]
        .as_str()
        .unwrap()
        .parse::<u64>()
        .unwrap();
    let evidence = value["report"]["evidence"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|fact| fact["id"] == request)
        .unwrap();
    evidence["at_ns"] = json!((outcome_ns + 1).to_string());
    assert!(
        archive_error(&value)
            .to_string()
            .contains("request_evidence_id")
    );
}

#[test]
fn control_trace_entries_must_match_the_time_of_the_referenced_event() {
    let report = run_lab(LABS[7]).unwrap();
    for kind in ["link_change", "routing_convergence", "stp_convergence"] {
        let mut value: Value = serde_json::from_slice(&archive(&report, None)).unwrap();
        let entry = value["report"]["trace"]["entries"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .rfind(|entry| entry["event"] == kind)
            .unwrap();
        let at_ns = entry["at_ns"].as_str().unwrap().parse::<u64>().unwrap();
        entry["at_ns"] = json!((at_ns + 1).to_string());
        assert!(
            reopen(&value).is_err(),
            "accepted a later time for the same {kind} event"
        );
    }
}

#[test]
fn timeout_outcomes_must_match_the_submitted_deadline_and_stop_boundary() {
    let legacy = run_lab(LABS[0]).unwrap();
    let mut value: Value = serde_json::from_slice(&archive(&legacy, None)).unwrap();
    value["report"]["outcome"]["at_ns"] = json!("2001");
    assert!(
        reopen(&value).is_err(),
        "accepted a legacy timeout after its deadline"
    );
    let report = run_lab(LABS[7]).unwrap();
    for at in [1, 4351, 35501] {
        let mut value: Value = serde_json::from_slice(&archive(&report, None)).unwrap();
        value["report"]["probes"][0]["outcome"]["at_ns"] = json!(at.to_string());
        assert!(
            reopen(&value).is_err(),
            "accepted impossible timeout at {at}"
        );
    }
    let early = run_lab_with_limits(
        LABS[7],
        RunLimits {
            total_work: 3,
            ..RunLimits::default()
        },
    )
    .unwrap();
    let mut value: Value = serde_json::from_slice(&archive(&early, None)).unwrap();
    assert!(value["report"]["last_event_id"].is_string());
    assert_eq!(value["report"]["last_event_ns"], "0");
    let full: Value = serde_json::to_value(&report).unwrap();
    let mut timeout = full["probes"][0]["outcome"].clone();
    timeout["cause"] = Value::Null;
    value["report"]["probes"][0]["outcome"] = timeout;
    assert!(
        reopen(&value).is_err(),
        "accepted a deadline timeout beyond an early resource stop"
    );
}

#[test]
fn an_incomplete_outcome_requires_the_reported_stop_reason() {
    let report = run_lab_with_limits(
        LABS[7],
        RunLimits {
            pending_events: 1,
            ..Default::default()
        },
    )
    .unwrap();
    let mut value: Value = serde_json::from_slice(&archive(&report, None)).unwrap();
    value["report"]["stop_reason"] = Value::Null;
    assert!(archive_error(&value).to_string().contains("stop_reason"));
}

#[test]
fn archive_version_is_checked_before_decoding_the_report() {
    let error = archive_error(&json!({"archive_version":2,"report":{}}));
    assert!(matches!(error, ArchiveError::UnsupportedVersion(2)));
    let report = run_lab(LABS[0]).unwrap();
    let mut value: Value = serde_json::from_slice(&archive(&report, None)).unwrap();
    value["report"]["report_version"] = json!(99);
    assert!(reopen(&value).is_err());
}

#[test]
fn archive_reader_accepts_exactly_64_mib_and_rejects_one_more_byte() {
    const LIMIT: u64 = 64 * 1024 * 1024;
    let bytes = archive(&run_lab(LABS[0]).unwrap(), None);
    let exact = bytes
        .as_slice()
        .chain(io::repeat(b' ').take(LIMIT - bytes.len() as u64));
    assert!(read_archive(exact).is_ok());
    assert!(matches!(
        read_archive(io::repeat(b' ').take(LIMIT + 1)),
        Err(ArchiveError::TooLarge)
    ));
}

#[test]
fn archive_writer_enforces_the_same_size_limit() {
    let mut report = run_lab(LABS[0]).unwrap();
    let RunReport::Legacy(legacy) = &mut report else {
        panic!("legacy")
    };
    legacy.topology_id = "x".repeat(64 * 1024 * 1024);
    assert!(matches!(
        write_archive(&report, None, None, io::sink()),
        Err(ArchiveError::TooLarge)
    ));
}

#[test]
fn an_early_resource_stop_keeps_unfinished_counts_without_inventing_a_last_event() {
    let report = run_lab_with_limits(
        LABS[7],
        RunLimits {
            pending_events: 1,
            ..Default::default()
        },
    )
    .unwrap();
    let reopened = read_archive(archive(&report, None).as_slice()).unwrap();
    let summary = reopened.summary();
    assert_eq!(summary.status, RunStatus::Incomplete);
    assert_eq!(summary.probe_counts.completed, 0);
    assert_eq!(summary.probe_counts.incomplete, 7);
    assert_eq!(
        summary.stop_reason,
        Some(StopReason::ResourceLimit {
            limit: ResourceLimit::PendingEvents,
        })
    );
    assert_eq!(summary.stop_limit, Some(1));
    assert_eq!(summary.last_event_id, None);
    assert_eq!(summary.last_event_ns, None);
    assert_eq!(summary.trace_status, Some(TraceStatus::Collected));
}

#[test]
fn convergence_trigger_references_and_evidence_positions_are_checked() {
    let report = run_lab(LABS[7]).unwrap();
    for collection in ["routing_events", "stp_events"] {
        let mut value: Value = serde_json::from_slice(&archive(&report, None)).unwrap();
        let event = value["report"][collection]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|event| event["triggered_by_link_event_id"].is_string())
            .unwrap();
        event["triggered_by_link_event_id"] = json!("999999");
        assert!(
            archive_error(&value)
                .to_string()
                .contains("triggered_by_link_event_id")
        );
    }
    let mut value: Value = serde_json::from_slice(&archive(&report, None)).unwrap();
    value["report"]["evidence"][0]["id"] = json!("2");
    assert!(reopen(&value).is_err());
}

#[test]
fn an_unrelated_sidecar_trace_cannot_replace_embedded_facts() {
    let report = run_lab(LABS[7]).unwrap();
    let mut value: Value = serde_json::from_slice(&archive(&report, None)).unwrap();
    value["trace"] = value["report"]["trace"].clone();
    value["trace"]["entries"].as_array_mut().unwrap().pop();
    assert!(archive_error(&value).to_string().contains("不一致"));
}

struct BrokenIo;

impl Read for BrokenIo {
    fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "archive read denied",
        ))
    }
}

impl Write for BrokenIo {
    fn write(&mut self, _: &[u8]) -> io::Result<usize> {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "archive write denied",
        ))
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn archive_io_errors_preserve_the_actual_failure() {
    let error = match read_archive(BrokenIo) {
        Err(error) => error,
        Ok(_) => panic!("读取失败被吞掉"),
    };
    assert!(
        matches!(&error, ArchiveError::Io(error) if error.kind() == io::ErrorKind::PermissionDenied)
    );
    assert!(error.to_string().contains("archive read denied"));
    let error = write_archive(&run_lab(LABS[0]).unwrap(), None, None, BrokenIo).unwrap_err();
    assert!(
        matches!(&error, ArchiveError::Io(error) if error.kind() == io::ErrorKind::PermissionDenied)
    );
    assert!(error.to_string().contains("archive write denied"));
}
