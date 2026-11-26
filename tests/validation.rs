use std::{
    io::Write,
    process::{Command, Stdio},
};

use reliner::{
    RunMode, RunStatus, ValidationResult, Workspace, WorkspaceConfig, WorkspaceError, validate_lab,
};
use serde_json::{Value, json};

#[test]
fn future_native_format_can_be_viewed_without_being_loaded_or_rewritten() {
    let input = "\u{feff}{\"format_version\":37,\"future\":{\"config\":true}}\n".to_owned();
    let preview = reliner::preview_native(input.clone());
    assert_eq!(preview.text, input);
    assert_eq!(
        preview.format,
        reliner::NativeFormat::Unsupported { format_version: 37 }
    );
    assert_eq!(
        validate_lab(&preview.text).unwrap_err().code,
        "unsupported_version"
    );
    let invalid = reliner::preview_native("{ broken".into());
    assert!(matches!(
        invalid.format,
        reliner::NativeFormat::Invalid { .. }
    ));
    assert_eq!(invalid.text, "{ broken");
}

#[test]
fn decode_errors_keep_the_original_probe_identity() {
    let mut input: Value = serde_json::from_str(include_str!("../examples/stp-lab.json")).unwrap();
    input["scenario"]["probes"][0]["ttl"] = json!("bad");
    let error = validate_lab(&input.to_string()).unwrap_err();
    assert_eq!(error.location.path, "/scenario/probes/0/ttl");
    assert_eq!(error.location.probe_id.as_deref(), Some("initial-tree"));
}

#[test]
fn legacy_effective_settings_are_readonly_model_values() {
    let settings = reliner::effective_settings(include_str!("../examples/vlan-lab.json")).unwrap();
    assert_eq!(settings.len(), 2);
    for setting in &settings {
        assert_eq!(setting.location.path, "/probe");
        assert_eq!(setting.location.probe_id, None);
        assert_eq!(setting.source, reliner::SettingSource::ModelDefault);
        assert!(!setting.configurable);
    }
    assert_eq!(settings[0].key, "ttl");
    assert_eq!(settings[1].key, "payload_bytes");
}

#[test]
fn effective_configuration_pages_are_revision_bound() {
    let workspace = Workspace::new(WorkspaceConfig::default()).unwrap();
    let doc = workspace
        .open_document(include_str!("../examples/stp-lab.json").into())
        .unwrap();
    let first = workspace
        .effective_settings_page(doc.id, doc.revision, 0, 1)
        .unwrap();
    assert_eq!(first.revision, doc.revision);
    assert_eq!(first.page.entries.len(), 1);
    let next = workspace
        .effective_settings_page(doc.id, doc.revision, first.page.next_offset.unwrap(), 128)
        .unwrap();
    assert!(!next.page.entries.is_empty());
    assert_eq!(workspace.stats().compiles_started, 0);
}

fn validate(input: &str) -> std::process::Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_reliner"))
        .args(["validate", "-", "--json"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

#[test]
fn cli_validation_checks_a_document_without_creating_a_run() {
    let output = validate(include_str!("../examples/campus-lab.json"));
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["status"], "valid");
    assert_eq!(result["summary"]["topology_id"], "campus-lab");
    assert_eq!(result["summary"]["devices"], 8);
    assert!(result.get("probes").is_none());
}

#[test]
fn invalid_vlan_points_to_its_original_port_and_field() {
    let mut input: Value = serde_json::from_str(include_str!("../examples/vlan-lab.json")).unwrap();
    input["devices"][1]["ports"][0]["mode"]["vlan"] = json!(4095);
    let output = validate(&input.to_string());
    assert_eq!(output.status.code(), Some(1));
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["status"], "invalid");
    assert_eq!(result["error"]["code"], "vlan_out_of_range");
    assert_eq!(
        result["error"]["location"]["path"],
        "/devices/1/ports/0/mode/vlan"
    );
    assert_eq!(result["error"]["location"]["device_id"], "sw1");
    assert_eq!(result["error"]["location"]["port_id"], "sw1-access");
}

#[test]
fn semantic_errors_keep_source_positions_after_port_sorting() {
    let mut input: Value = serde_json::from_str(include_str!("../examples/vlan-lab.json")).unwrap();
    input["devices"][2]["ports"][0]["mode"]["allowed_vlans"] = json!([20, 0, 10]);
    let error = validate_lab(&input.to_string()).unwrap_err();
    assert_eq!(error.code, "vlan_out_of_range");
    assert_eq!(error.location.path, "/devices/2/ports/0/mode/allowed_vlans");
    assert_eq!(error.location.port_id.as_deref(), Some("sw2-uplink"));

    input["devices"][2]["ports"][0]["mode"]["allowed_vlans"] = json!([20, 10]);
    input["devices"][3]["prefix_length"] = json!(33);
    let error = validate_lab(&input.to_string()).unwrap_err();
    assert_eq!(error.code, "invalid_ipv4");
    assert_eq!(error.location.path, "/devices/3/prefix_length");
    assert_eq!(error.location.port_id.as_deref(), Some("pc-b-eth0"));
}

#[test]
fn duplicate_and_unknown_references_identify_the_offending_input() {
    let original: Value = serde_json::from_str(include_str!("../examples/vlan-lab.json")).unwrap();
    let mut input = original.clone();
    input["devices"][2]["ports"][1]["id"] = input["devices"][1]["ports"][0]["id"].clone();
    let error = validate_lab(&input.to_string()).unwrap_err();
    assert_eq!(error.code, "duplicate_id");
    assert_eq!(error.location.path, "/devices/2/ports/1/id");
    assert_eq!(error.location.device_id.as_deref(), Some("sw2"));

    input = original;
    input["links"][1]["ports"][1] = json!("missing-port");
    let error = validate_lab(&input.to_string()).unwrap_err();
    assert_eq!(error.code, "unknown_reference");
    assert_eq!(error.location.path, "/links/1/ports/1");
    assert_eq!(error.location.link_id.as_deref(), Some("sw1-sw2"));
    assert_eq!(error.location.port_id, None);
}

#[test]
fn an_invalid_duplicate_port_never_points_to_an_earlier_device() {
    let mut input: Value = serde_json::from_str(include_str!("../examples/vlan-lab.json")).unwrap();
    input["devices"][2]["ports"][0]["id"] = json!("sw1-access");
    input["devices"][2]["ports"][0]["mode"]["allowed_vlans"] = json!([0]);
    let error = validate_lab(&input.to_string()).unwrap_err();
    assert_eq!(error.code, "vlan_out_of_range");
    assert_eq!(error.location.path, "/devices/2/ports/0/mode/allowed_vlans");
    assert_eq!(error.location.device_id.as_deref(), Some("sw2"));
}

#[test]
fn json_errors_retain_real_parser_locations_and_reject_trailing_content() {
    let input = include_str!("../examples/vlan-lab.json");
    assert_eq!(
        validate_lab(&format!("\u{feff}{input}")).unwrap().devices,
        4
    );
    let error = validate_lab(&format!("{input} true")).unwrap_err();
    assert_eq!(error.code, "invalid_json");
    assert!(error.line.is_some());
    let mut malformed: Value = serde_json::from_str(input).unwrap();
    malformed["devices"][2]["ports"][0]["mode"]["unexpected_vlan"] = json!(10);
    let error = validate_lab(&malformed.to_string()).unwrap_err();
    assert_eq!(error.code, "invalid_value");
    // Internally tagged Serde enums report the containing device, not an invented leaf.
    assert_eq!(error.location.path, "/devices/2");
    assert_eq!(error.location.device_id.as_deref(), Some("sw2"));
    malformed["format_version"] = json!(4);
    let error = validate_lab(&malformed.to_string()).unwrap_err();
    assert_eq!(error.code, "unsupported_version");
    assert_eq!(error.location.path, "/format_version");
}

#[test]
fn workspace_validation_and_compile_failure_share_the_same_diagnostic() {
    let mut input: Value = serde_json::from_str(include_str!("../examples/ipv4-lab.json")).unwrap();
    input["devices"][1]["ports"][0]["mac"] = json!("bad-mac");
    let workspace = Workspace::new(WorkspaceConfig::default()).unwrap();
    let doc = workspace.open_document(input.to_string()).unwrap();
    let checked = workspace.validate_document(doc.id, doc.revision).unwrap();
    assert_eq!(checked.revision, doc.revision);
    assert_eq!(checked.effective_revision, doc.effective_revision);
    let ValidationResult::Invalid { error } = checked.result else {
        panic!("invalid MAC")
    };
    assert_eq!(workspace.stats().compiles_started, 0);
    assert_eq!(error.code, "invalid_mac");
    assert_eq!(error.location.path, "/devices/1/ports/0/mac");
    let run = workspace.submit_run(doc.id, RunMode::Automatic).unwrap();
    let mut summary = workspace.run_summary(run.id).unwrap();
    while !summary.status.is_terminal() {
        summary = workspace
            .wait_for_update(run.id, summary.update, std::time::Duration::from_secs(1))
            .unwrap();
    }
    assert_eq!(summary.status, RunStatus::Failed);
    assert_eq!(summary.error, Some(error.to_string()));
    assert_eq!(summary.input_error, Some(error));
    assert_eq!(summary.probe_counts, None);

    let edited = workspace
        .replace_presentation(doc.id, doc.revision, Some("{}".into()))
        .unwrap();
    assert!(
        matches!(workspace.validate_document(doc.id, doc.revision), Err(WorkspaceError::StaleEdit { actual }) if actual == edited.revision)
    );
}

#[test]
fn workspace_only_label_limit_does_not_change_direct_experiment_semantics() {
    let mut input: Value = serde_json::from_str(include_str!("../examples/vlan-lab.json")).unwrap();
    input["devices"][0]["name"] = json!("x".repeat(257));
    assert!(validate_lab(&input.to_string()).is_ok());
    let workspace = Workspace::new(WorkspaceConfig::default()).unwrap();
    let doc = workspace.open_document(input.to_string()).unwrap();
    let checked = workspace.validate_document(doc.id, doc.revision).unwrap();
    let ValidationResult::Invalid { error } = checked.result else {
        panic!("label limit")
    };
    assert_eq!(error.code, "label_too_long");
    assert_eq!(error.location.path, "/devices/0/name");
}

#[test]
fn effective_settings_distinguish_model_defaults_from_explicit_values() {
    let mut input: Value = serde_json::from_str(include_str!("../examples/stp-lab.json")).unwrap();
    input["scenario"]["probes"][0]
        .as_object_mut()
        .unwrap()
        .remove("ttl");
    let values =
        serde_json::to_value(reliner::effective_settings(&input.to_string()).unwrap()).unwrap();
    let settings = values.as_array().unwrap();
    let find = |path: &str| {
        settings
            .iter()
            .find(|setting| setting["location"]["path"] == path)
            .unwrap()
    };
    assert_eq!(find("/scenario/probes/0/ttl")["value"], 64);
    assert_eq!(find("/scenario/probes/0/ttl")["source"], "model_default");
    assert_eq!(find("/devices/1/stp_priority")["source"], "explicit");
    input["scenario"]["probes"][0]["ttl"] = json!(64);
    let values =
        serde_json::to_value(reliner::effective_settings(&input.to_string()).unwrap()).unwrap();
    let explicit = values
        .as_array()
        .unwrap()
        .iter()
        .find(|setting| setting["location"]["path"] == "/scenario/probes/0/ttl")
        .unwrap();
    assert_eq!(explicit["source"], "explicit");
    assert_eq!(explicit["value"], 64);
}
