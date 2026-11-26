use reliner::{ImportOptions, RunReport, import_gns3, preview_gns3, run_lab};
use serde_json::{Value, json};

const A: &str = "11111111-1111-4111-8111-111111111111";
const B: &str = "22222222-2222-4222-8222-222222222222";
const LINK: &str = "33333333-3333-4333-8333-333333333333";
const PROJECT: &str = include_str!("../examples/import-gns3.gns3");
const OPTIONS: &str = include_str!("../examples/import-gns3-models.json");

fn project() -> Value {
    serde_json::from_str(PROJECT).unwrap()
}

fn options() -> ImportOptions {
    serde_json::from_str(OPTIONS).unwrap()
}

#[test]
fn source_id_ports_and_vpcs_configuration_remain_explicit() {
    let input = project().to_string();
    let preview = preview_gns3(&input).unwrap();
    assert_eq!(preview.source.format, "gns3");
    assert_eq!(preview.links[0].id, LINK);
    assert_eq!(preview.links[0].endpoints[0].id, format!("{A}:a0p0"));
    assert_eq!(preview.source.gaps.len(), 2);
    let native = import_gns3(&input, &options()).unwrap();
    let converted: Value = serde_json::from_str(&native).unwrap();
    assert_eq!(converted["devices"][0]["name"], "PC1");
    let report = run_lab(&native).unwrap();
    let RunReport::Scenario(report) = report else {
        panic!("scenario expected")
    };
    let outcome = serde_json::to_value(report.probes[0].outcome).unwrap();
    assert_eq!(outcome["status"], "undetermined");
    assert_eq!(outcome["modeled"]["status"], "reachable");
}

#[test]
fn external_connectivity_and_unmapped_link_state_are_not_silently_ignored() {
    let mut input = project();
    input["topology"]["nodes"][0]["node_type"] = json!("cloud");
    input["topology"]["links"][0]["link_type"] = json!("ethernet");
    input["topology"]["links"][0]["suspend"] = json!(true);
    let result = serde_json::to_value(preview_gns3(&input.to_string()).unwrap()).unwrap();
    let gaps = result["source"]["gaps"].as_array().unwrap();
    assert!(
        gaps.iter()
            .any(|gap| gap["source_path"] == "/topology/nodes/0/node_type"
                && gap["scope"]["kind"] == "all")
    );
    assert!(
        gaps.iter()
            .any(|gap| gap["source_path"] == "/topology/links/0/suspend"
                && gap["scope"]["link_id"] == LINK)
    );
}

#[test]
fn refuses_unverified_revisions_and_broken_endpoint_references() {
    let mut input = project();
    input["revision"] = json!(10);
    assert!(
        preview_gns3(&input.to_string())
            .unwrap_err()
            .to_string()
            .contains("/revision")
    );
    input["revision"] = json!(9);
    input["topology"]["links"][0]["nodes"][0]["node_id"] =
        json!("55555555-5555-4555-8555-555555555555");
    assert!(
        preview_gns3(&input.to_string())
            .unwrap_err()
            .to_string()
            .contains("/topology/links/0/nodes/0/node_id")
    );
}

#[test]
fn serial_links_preserve_their_endpoints_but_are_not_simulated_as_ethernet() {
    let mut input = project();
    input["topology"]["links"][0]["link_type"] = json!("serial");
    let preview = preview_gns3(&input.to_string()).unwrap();
    assert!(preview.links.is_empty());
    assert!(preview.nodes.iter().all(|node| node.ports.len() == 1));
    assert_eq!(preview.omitted_links[0].link.id, LINK);
    assert_eq!(preview.omitted_links[0].link.endpoints[1].node, B);
    assert_eq!(preview.omitted_links[0].reason, "unsupported_media");
    assert!(
        preview
            .source
            .gaps
            .iter()
            .any(|gap| gap.source_path == "/topology/links/0"
                && gap.scope == reliner::ImportScope::All
                && gap.value["link_type"] == "serial")
    );
}

#[test]
fn preview_rejects_same_node_links_and_malformed_known_fields() {
    for (path, value, location) in [
        (
            "/topology/links/0/nodes/1/node_id",
            json!(A),
            "/topology/links/0/nodes/1/node_id",
        ),
        ("/topology/drawings", json!("broken"), "/topology/drawings"),
        ("/topology/drawings", json!([42]), "/topology/drawings/0"),
        ("/topology/computes", json!(null), "/topology/computes"),
        (
            "/topology/computes",
            json!([{"compute_id":"local"}]),
            "/topology/computes/0/name",
        ),
        (
            "/topology/nodes/0/compute_id",
            json!(5),
            "/topology/nodes/0/compute_id",
        ),
        (
            "/topology/nodes/0/properties",
            json!(null),
            "/topology/nodes/0/properties",
        ),
    ] {
        let mut input = project();
        *input.pointer_mut(path).unwrap() = value;
        assert!(
            preview_gns3(&input.to_string())
                .unwrap_err()
                .to_string()
                .contains(location)
        );
    }
}

#[test]
fn unevidenced_media_and_inert_saved_defaults_are_distinct() {
    let mut input = project();
    input["variables"] = json!([]);
    input["topology"]["links"][0]["suspend"] = json!(false);
    input["topology"]["links"][0]["filters"] = json!({});
    let preview = preview_gns3(&input.to_string()).unwrap();
    assert_eq!(preview.links.len(), 1);
    assert_eq!(preview.source.gaps.len(), 2);
    for node in input["topology"]["nodes"].as_array_mut().unwrap() {
        node["node_type"] = json!("dynamips");
    }
    let preview = preview_gns3(&input.to_string()).unwrap();
    assert!(preview.links.is_empty());
    assert!(preview.source.gaps.iter().any(
        |gap| gap.source_path == "/topology/links/0" && gap.scope == reliner::ImportScope::All
    ));
    input["topology"]["links"][0]["link_type"] = json!("ethernet");
    assert_eq!(preview_gns3(&input.to_string()).unwrap().links.len(), 1);
}

#[test]
fn desktop_session_can_preview_and_open_gns3_without_writing_a_file() {
    let mut backend = reliner::bridge::Backend::new(Default::default()).unwrap();
    let hello: Value = serde_json::from_str(
        &backend
            .handle_json(r#"{"protocol_version":1,"request_id":"1","command":{"type":"hello"}}"#),
    )
    .unwrap();
    let mut call = |command| {
        let request = json!({"protocol_version":1,"request_id":"2","session_id":hello["session_id"],"command":command});
        serde_json::from_str::<Value>(&backend.handle_json(&request.to_string())).unwrap()
    };
    let source = project().to_string();
    let preview = call(json!({"type":"import.gns3.preview","project":source}));
    assert_eq!(preview["data"]["source"]["format"], "gns3");
    let error = call(json!({"type":"import.gns3.open","project":"{","options":options()}));
    assert_eq!(error["error"]["details"]["import_error"]["kind"], "json");
    let opened = call(json!({"type":"import.gns3.open","project":source,"options":options()}));
    assert_eq!(opened["data"]["id"], "1");
    assert_eq!(opened["data"]["format_version"], 3);
    assert!(opened["data"]["path"].is_null());
}

#[test]
fn cli_lists_gns3_structure() {
    use std::{
        io::Write,
        process::{Command, Stdio},
    };
    let mut child = Command::new(env!("CARGO_BIN_EXE_reliner"))
        .args(["import", "preview", "gns3", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(project().to_string().as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let preview: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(preview["source"]["format"], "gns3");
}
