use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant},
};

use reliner::{
    WorkspaceConfig, WorkspaceError,
    bridge::{Backend, BridgeError, DecimalU64, ErrorCode, Request, Response, ResponseBody},
};
use serde_json::{Value, json};

const VLAN: &str = include_str!("../examples/vlan-lab.json");
const IPV4: &str = include_str!("../examples/ipv4-lab.json");
const CAMPUS: &str = include_str!("../examples/campus-lab.json");
const STP: &str = include_str!("../examples/stp-lab.json");
const IMPORT_YAML: &str = include_str!("../examples/import-containerlab.yml");
const IMPORT_OPTIONS: &str = include_str!("../examples/import-models.json");

struct Client {
    backend: Backend,
    session: Option<String>,
    next_request: u64,
}

impl Client {
    fn new() -> Self {
        Self::with_config(WorkspaceConfig {
            workers: 1,
            resident_runs: 1,
            queue_slots: 1,
            ..WorkspaceConfig::default()
        })
    }

    fn with_config(config: WorkspaceConfig) -> Self {
        Self {
            backend: Backend::new(config).unwrap(),
            session: None,
            next_request: 0,
        }
    }

    fn call(&mut self, command: Value) -> Value {
        self.next_request += 1;
        let id = self.next_request.to_string();
        let request = json!({
            "protocol_version": 1,
            "request_id": id,
            "session_id": self.session,
            "command": command,
        });
        let encoded = self.backend.handle_json(&request.to_string());
        assert!(!encoded.contains('\n'));
        let response: Value = serde_json::from_str(&encoded).unwrap();
        assert_eq!(response["request_id"], id);
        response
    }

    fn hello(&mut self) -> Value {
        let response = self.call(json!({"type": "hello"}));
        assert_eq!(response["status"], "ok", "{response}");
        let session = response["session_id"].as_str().unwrap().to_owned();
        assert!(!session.is_empty());
        assert!(session.bytes().all(|byte| byte.is_ascii_digit()));
        self.session = Some(session);
        response["data"].clone()
    }

    fn ok(&mut self, command: Value) -> Value {
        let response = self.call(command);
        assert_eq!(response["status"], "ok", "{response}");
        response["data"].clone()
    }

    fn open(&mut self, input: &str) -> Value {
        self.ok(json!({"type": "document.open", "input": input}))
    }

    fn submit(&mut self, doc: &Value, mode: &str) -> Value {
        self.ok(json!({
            "type": "run.submit",
            "document_id": doc["id"],
            "expected_revision": doc["revision"],
            "mode": mode,
        }))
    }

    fn wait(&mut self, run: &Value, statuses: &[&str]) -> Value {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let result = self.ok(json!({"type": "run.summary", "run_id": run["run"]["id"]}));
            let status = result["summary"]["status"].as_str().unwrap();
            if statuses.contains(&status) {
                return result;
            }
            assert!(Instant::now() < deadline, "run did not settle: {result}");
            thread::sleep(Duration::from_millis(1));
        }
    }

    fn entries(&mut self, mut command: Value) -> Vec<Value> {
        let mut entries = Vec::new();
        let mut offset = 0;
        loop {
            command["offset"] = json!(offset);
            let page = self.ok(command.clone());
            entries.extend(page["entries"].as_array().unwrap().iter().cloned());
            match page["next_offset"].as_u64() {
                Some(next) => {
                    assert!(next > offset, "pagination did not advance: {page}");
                    offset = next;
                }
                None => return entries,
            }
        }
    }
}

struct TemporaryFile(PathBuf);

impl TemporaryFile {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        Self(std::env::temp_dir().join(format!(
            "reliner-bridge-test-{}-{}.json",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        )))
    }
}

impl Drop for TemporaryFile {
    fn drop(&mut self) {
        match fs::remove_file(&self.0) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => panic!("cannot remove test file {}: {error}", self.0.display()),
        }
    }
}

#[test]
fn handshake_is_required_and_repeating_it_preserves_the_workspace() {
    let mut client = Client::new();
    let denied = client.call(json!({"type": "document.open", "input": VLAN}));
    assert_eq!(denied["error"]["code"], "handshake_required");
    let hello = client.hello();
    assert_eq!(hello["protocol_version"], 1);
    assert_eq!(hello["backend_api_version"], 2);
    assert_eq!(hello["crate_version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(hello["native_formats"], json!([1, 2, 3]));
    assert_eq!(hello["report_versions"], json!([1, 2, 3, 4, 5, 6, 7, 8, 9]));
    assert_eq!(hello["max_page_size"], 128);
    let doc = client.open(VLAN);
    assert_eq!(doc["id"], "1");
    let session = client.session.clone();
    client.hello();
    assert_eq!(client.session, session);
    let info = client.ok(json!({"type": "document.info", "document_id": doc["id"]}));
    assert_eq!(info["id"], doc["id"]);
    assert!(info.get("experiment").is_none());
}

#[test]
fn import_preview_reports_structure_and_missing_options_without_opening_a_document() {
    let mut client = Client::new();
    let hello = client.hello();
    let commands = hello["commands"].as_array().unwrap();
    assert!(commands.iter().any(|name| name == "import.preview"));
    assert!(commands.iter().any(|name| name == "import.open"));

    let preview = client.ok(json!({"type": "import.preview", "yaml": IMPORT_YAML}));
    assert_eq!(preview["source"]["source_name"], "import-static-ipv4");
    assert_eq!(preview["nodes"].as_array().unwrap().len(), 4);
    assert_eq!(preview["links"].as_array().unwrap().len(), 3);
    let missing = preview["missing"].as_array().unwrap();
    assert!(missing.iter().any(|item| item["path"] == "/models/r2"));
    assert!(missing.iter().any(|item| item["path"] == "/scenario"));
    assert_eq!(client.open(VLAN)["id"], "1");
}

#[test]
fn ios_preview_keeps_line_numbers_and_reports_unresolved_list_edits() {
    let mut client = Client::new();
    assert!(
        client.hello()["commands"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v == "import.ios.preview")
    );
    let preview = client.ok(json!({"type":"import.ios.preview","config":
        "!\r\ninterface GigabitEthernet1/0/1\r\n switchport mode trunk\r\n switchport trunk allowed vlan 10-12\r\n switchport trunk allowed vlan add 20\r\n!"}));
    assert_eq!(preview["interface"], "GigabitEthernet1/0/1");
    assert_eq!(preview["mode"], json!({"kind":"trunk"}));
    assert_eq!(preview["field_lines"], json!({"kind":3}));
    let gaps = preview["gaps"].as_array().unwrap();
    assert!(
        gaps.iter()
            .any(|g| g["line"] == 5 && g["field"] == "allowed_vlans")
    );
    assert!(
        gaps.iter()
            .any(|g| g["line"].is_null() && g["field"] == "native_vlan")
    );

    for (config, line) in [
        ("interface Gi0/1\n switchport trunk allowed vlan 22-20", 2),
        ("interface Gi0/1\n switchport access vlan 4095", 2),
        ("interface range Gi0/1 - 3", 1),
        ("interface Vlan10\n switchport mode trunk", 1),
        ("interface Gi0/1\n!\ninterface Gi0/2", 3),
        ("interface Gi0/1\n!\nswitchport trunk native vlan 20", 3),
        ("interface Gi0/1\nend\nswitchport mode access", 3),
    ] {
        let response = client.call(json!({"type":"import.ios.preview","config":config}));
        assert_eq!(response["error"]["code"], "import_error");
        assert_eq!(
            response["error"]["details"]["import_error"]["path"],
            format!("/ios_config/lines/{line}")
        );
    }
}

#[test]
fn whole_ios_config_opens_a_workspace_document_with_mapped_switch_ports() {
    let mut client = Client::new();
    let config = include_str!("../examples/ios-switch.cfg");
    let hello = client.hello();
    assert!(
        hello["commands"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v == "import.ios.device.preview")
    );
    client.ok(json!({"type":"import.ios.device.preview","config":config}));
    let mut options: Value =
        serde_json::from_str(include_str!("../examples/import-ios-models.json")).unwrap();
    options["models"]["r1"]["ios_device_config"] = json!(config);
    let doc = client.ok(json!({"type":"import.open","yaml":IMPORT_YAML,"options":options}));
    let data = client.ok(json!({"type":"document.get","document_id":doc["id"]}));
    let topology: Value = serde_json::from_str(data["experiment"].as_str().unwrap()).unwrap();
    let sw = topology["devices"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["id"] == "r1")
        .unwrap();
    let ports = sw["ports"].as_array().unwrap();
    assert!(
        ports
            .iter()
            .any(|p| p["id"] == "r1:eth2" && p["mode"]["native_vlan"] == 10)
    );
    assert!(
        ports
            .iter()
            .any(|p| p["id"] == "r1:eth1" && p["mode"]["vlan"] == 10)
    );
    let run = client.submit(&doc, "automatic");
    client.wait(&run, &["finished"]);
    let result =
        client.ok(json!({"type":"run.probes","run_id":run["run"]["id"],"offset":0,"count":1}));
    assert_eq!(result["entries"][0]["outcome"]["status"], "undetermined");
    assert_eq!(
        result["entries"][0]["outcome"]["modeled"]["status"],
        "timeout"
    );
}

#[test]
fn import_failures_preserve_structured_diagnostics_and_do_not_open_documents() {
    let mut client = Client::with_config(WorkspaceConfig {
        max_documents: 1,
        ..WorkspaceConfig::default()
    });
    client.hello();
    let mut options: Value = serde_json::from_str(IMPORT_OPTIONS).unwrap();
    for command in [
        json!({"type": "import.preview", "yaml": "topology: [\n"}),
        json!({"type": "import.open", "yaml": "topology: [\n", "options": options}),
    ] {
        let failed = client.call(command);
        assert_eq!(failed["error"]["code"], "import_error");
        let diagnostic = &failed["error"]["details"]["import_error"];
        assert_eq!(diagnostic["kind"], "yaml");
        assert!(diagnostic["line"].is_number());
        assert!(diagnostic["column"].is_number());
    }

    let mut missing = options.clone();
    missing["models"].as_object_mut().unwrap().remove("r2");
    let failed =
        client.call(json!({"type": "import.open", "yaml": IMPORT_YAML, "options": missing}));
    assert_eq!(failed["error"]["code"], "import_error");
    let diagnostic = &failed["error"]["details"]["import_error"];
    assert_eq!(diagnostic["kind"], "mapping");
    assert_eq!(diagnostic["path"], "/models/r2");

    options["scenario"]["probes"][0]["source"] = json!("missing-host");
    let failed =
        client.call(json!({"type": "import.open", "yaml": IMPORT_YAML, "options": options}));
    assert_eq!(failed["error"]["code"], "import_error");
    let diagnostic = &failed["error"]["details"]["import_error"];
    assert_eq!(diagnostic["kind"], "validation");
    assert_eq!(
        diagnostic["error"]["location"]["path"],
        "/scenario/probes/0/source"
    );
    assert_eq!(client.open(VLAN)["id"], "1");
}

#[test]
fn import_open_uses_explicit_options_and_creates_an_unsaved_workspace_document() {
    let mut client = Client::new();
    client.hello();
    let options: Value = serde_json::from_str(IMPORT_OPTIONS).unwrap();
    let opened = client.ok(json!({"type": "import.open", "yaml": IMPORT_YAML, "options": options}));
    assert_eq!(opened["id"], "1");
    assert_eq!(opened["topology_id"], options["id"]);
    assert_eq!(opened["native_revision"], options["revision"]);
    assert_eq!(opened["format_version"], 3);
    assert_eq!(opened["dirty"], true);
    assert!(opened["path"].is_null());
    assert!(opened["saved_revision"].is_null());

    let checked = client.ok(
        json!({"type": "document.validate", "document_id": opened["id"],
        "expected_revision": opened["revision"]}),
    );
    assert_eq!(checked["status"], "valid");
    assert_eq!(checked["summary"]["devices"], 4);
    let stored = client.ok(json!({"type": "document.get", "document_id": opened["id"]}));
    let native: Value = serde_json::from_str(stored["experiment"].as_str().unwrap()).unwrap();
    assert_eq!(native["import_source"]["source_name"], "import-static-ipv4");
    assert_eq!(native["links"][0]["delay_ns"], options["link_delay_ns"]);
}

#[test]
fn version_and_session_checks_precede_command_decoding_and_mutation() {
    let mut first = Client::new();
    let mut second = Client::new();
    first.hello();
    second.hello();
    assert_ne!(first.session, second.session);
    let old_doc = first.open(VLAN);
    let current_doc = second.open(IPV4);
    assert_eq!(old_doc["id"], current_doc["id"]);
    let mut envelope = json!({
        "protocol_version": 99,
        "request_id": "9007199254740993",
        "session_id": second.session,
        "command": {"type": "unknown.command"},
    });
    let response: Value =
        serde_json::from_str(&second.backend.handle_json(&envelope.to_string())).unwrap();
    assert_eq!(response["error"]["code"], "unsupported_protocol");
    assert_eq!(response["request_id"], "9007199254740993");
    envelope["protocol_version"] = json!(1);
    envelope["session_id"] = json!(first.session);
    let response: Value =
        serde_json::from_str(&second.backend.handle_json(&envelope.to_string())).unwrap();
    assert_eq!(response["error"]["code"], "session_mismatch");
    envelope["command"] = json!({
        "type": "document.close", "document_id": old_doc["id"],
        "expected_revision": "1", "discard_unsaved": true,
    });
    let response: Value =
        serde_json::from_str(&second.backend.handle_json(&envelope.to_string())).unwrap();
    assert_eq!(response["error"]["code"], "session_mismatch");
    let info = second.ok(json!({"type": "document.info", "document_id": current_doc["id"]}));
    assert_eq!(info["topology_id"], "ipv4-lab");
    second.session = None;
    let denied = second.call(json!({"type": "document.info", "document_id": "1"}));
    assert_eq!(denied["error"]["code"], "session_mismatch");
}

#[test]
fn decimal_identifiers_are_strict_and_large_values_remain_exact() {
    let mut client = Client::new();
    client.hello();
    for invalid in [
        json!(1),
        json!("-1"),
        json!("+1"),
        json!("1.0"),
        json!(""),
        json!("18446744073709551616"),
    ] {
        let denied = client.call(json!({"type": "document.info", "document_id": invalid}));
        assert_eq!(denied["error"]["code"], "invalid_request", "{denied}");
    }
    for id in ["9007199254740993", "18446744073709551615"] {
        let denied = client.call(json!({"type": "document.info", "document_id": id}));
        assert_eq!(denied["error"]["code"], "unknown_document");
        assert_eq!(denied["error"]["details"]["document_id"], id);
    }
    let request: Request = serde_json::from_value(json!({
        "protocol_version": 1, "request_id": "18446744073709551615",
        "session_id": client.session,
        "command": {"type": "run.summary", "run_id": "9007199254740993"},
    }))
    .unwrap();
    assert_eq!(request.request_id, DecimalU64(u64::MAX));
    let response = client.backend.handle(request);
    let encoded = serde_json::to_string(&response).unwrap();
    let decoded: Response = serde_json::from_str(&encoded).unwrap();
    assert_eq!(decoded.request_id, Some(DecimalU64(u64::MAX)));
    assert!(
        matches!(decoded.body, ResponseBody::Error { error } if error.code == ErrorCode::UnknownRun)
    );
}

#[test]
fn malformed_unknown_and_extra_fields_do_not_consume_document_slots() {
    let mut client = Client::new();
    let malformed: Value = serde_json::from_str(&client.backend.handle_json("{ broken")).unwrap();
    assert_eq!(malformed["error"]["code"], "invalid_request");
    assert!(malformed["request_id"].is_null());
    client.hello();
    for command in [
        json!({"type": "run.report", "run_id": "1"}),
        json!({"type": "run.compare", "baseline": "1", "candidate": "2"}),
        json!({"type": "document.open", "input": VLAN, "surprise": true}),
        json!({"type": "document.open"}),
        json!({"type": "document.open", "input": VLAN, "path": "unread.json"}),
    ] {
        let response = client.call(command);
        assert_eq!(response["error"]["code"], "invalid_request", "{response}");
    }
    assert_eq!(client.open(VLAN)["id"], "1");
}

#[test]
fn edits_and_explicit_document_reads_keep_layout_and_simulation_revisions_distinct() {
    let mut client = Client::new();
    client.hello();
    let doc = client.open(VLAN);
    let layout = client.ok(json!({
        "type": "document.replace_presentation", "document_id": doc["id"],
        "expected_revision": "1", "presentation": "{\"x\":12}",
    }));
    assert_eq!(layout["revision"], "2");
    assert_eq!(layout["effective_revision"], "1");
    let stale = client.call(json!({
        "type": "document.replace_experiment", "document_id": doc["id"],
        "expected_revision": "1", "input": VLAN,
    }));
    assert_eq!(stale["error"]["code"], "stale_revision");
    assert_eq!(stale["error"]["details"]["expected"], "1");
    assert_eq!(stale["error"]["details"]["actual"], "2");
    let mut changed: Value = serde_json::from_str(VLAN).unwrap();
    changed["revision"] = json!("native-2");
    let edited = client.ok(json!({
        "type": "document.replace_experiment", "document_id": doc["id"],
        "expected_revision": "2", "input": changed.to_string(),
    }));
    assert_eq!(edited["revision"], "3");
    assert_eq!(edited["effective_revision"], "2");
    let read = client.ok(json!({"type": "document.get", "document_id": doc["id"]}));
    assert_eq!(read["info"]["native_revision"], "native-2");
    assert_eq!(read["presentation"], "{\"x\":12}");
    assert_eq!(
        serde_json::from_str::<Value>(read["experiment"].as_str().unwrap()).unwrap()["revision"],
        "native-2"
    );
}

#[test]
fn validation_returns_original_diagnostics_and_settings_are_revision_bound() {
    let mut client = Client::new();
    client.hello();
    let doc = client.open(IPV4);
    let valid = client.ok(
        json!({"type": "document.validate", "document_id": doc["id"], "expected_revision": "1"}),
    );
    assert_eq!(valid["status"], "valid");
    let settings = client.ok(json!({
        "type": "document.effective_settings", "document_id": doc["id"],
        "expected_revision": "1", "offset": 0, "count": 1,
    }));
    assert_eq!(settings["entries"].as_array().unwrap().len(), 1);
    assert_eq!(settings["next_offset"], 1);
    let mut invalid: Value = serde_json::from_str(IPV4).unwrap();
    invalid["scenario"]["probes"][0]["source"] = json!("missing-host");
    client.ok(json!({
        "type": "document.replace_experiment", "document_id": doc["id"],
        "expected_revision": "1", "input": invalid.to_string(),
    }));
    let diagnosed = client.ok(
        json!({"type": "document.validate", "document_id": doc["id"], "expected_revision": "2"}),
    );
    assert_eq!(diagnosed["status"], "invalid");
    assert_eq!(diagnosed["error"]["location"]["probe_id"], "first");
    let stale = client.call(json!({
        "type": "document.effective_settings", "document_id": doc["id"],
        "expected_revision": "1", "offset": 0, "count": 1,
    }));
    assert_eq!(stale["error"]["code"], "stale_revision");
    let failed = client.call(json!({
        "type": "document.effective_settings", "document_id": doc["id"],
        "expected_revision": "2", "offset": 0, "count": 1,
    }));
    assert_eq!(failed["error"]["code"], "input_error");
    assert_eq!(
        failed["error"]["details"]["input_error"]["location"]["probe_id"],
        "first"
    );
}

#[test]
fn all_page_commands_reject_zero_and_oversized_pages_before_dispatch() {
    let mut client = Client::new();
    client.hello();
    for command in [
        "run.probes",
        "run.evidence",
        "run.links",
        "run.routes",
        "run.stp",
        "run.stp_bridges",
        "run.stp_links",
        "run.compare",
        "run.import_gaps",
        "run.trace",
        "document.effective_settings",
    ] {
        for count in [0, 129] {
            let mut request = json!({"type": command, "offset": 0, "count": count});
            if command == "document.effective_settings" {
                request["document_id"] = json!("1");
                request["expected_revision"] = json!("1");
            } else if command == "run.compare" {
                request["baseline"] = json!("1");
                request["candidate"] = json!("2");
            } else {
                request["run_id"] = json!("1");
                if command == "run.stp_bridges" || command == "run.stp_links" {
                    request["event_id"] = json!("1");
                }
            }
            let response = client.call(request);
            assert_eq!(response["error"]["code"], "invalid_page", "{response}");
        }
    }
}

#[test]
fn run_pages_keep_the_submitted_revision_after_edits_and_document_close() {
    let mut client = Client::new();
    client.hello();
    let doc = client.open(IPV4);
    let run = client.submit(&doc, "automatic");
    let done = client.wait(&run, &["finished"]);
    assert_eq!(done["origin"]["document_revision"], "1");
    assert_eq!(done["origin"], run["origin"]);
    assert_eq!(done["origin"]["topology_id"], "ipv4-lab");
    assert_eq!(done["origin"]["native_revision"], "1");
    assert_eq!(done["summary"]["origin"]["document_id"], doc["id"]);
    assert_eq!(done["summary"]["origin"]["document_revision"], "1");
    assert_eq!(done["summary"]["origin"]["effective_revision"], "1");
    assert_eq!(done["summary"]["origin"]["topology_id"], "ipv4-lab");
    assert_eq!(done["summary"]["origin"]["native_revision"], "1");
    assert!(done["summary"].get("evidence").is_none());
    assert!(done["summary"]["last_event_ns"].is_string());
    assert!(done["summary"]["update"].is_string());
    client.ok(json!({
        "type": "document.replace_presentation", "document_id": doc["id"],
        "expected_revision": "1", "presentation": "{}",
    }));
    let mut changed: Value = serde_json::from_str(IPV4).unwrap();
    changed["revision"] = json!("later-native");
    client.ok(json!({
        "type": "document.replace_experiment", "document_id": doc["id"],
        "expected_revision": "2", "input": changed.to_string(),
    }));
    client.ok(json!({
        "type": "document.close", "document_id": doc["id"],
        "expected_revision": "3", "discard_unsaved": true,
    }));
    let closed = client.ok(json!({"type": "run.summary", "run_id": run["run"]["id"]}));
    assert_eq!(closed["origin"], done["origin"]);
    assert_eq!(closed["summary"]["origin"], done["summary"]["origin"]);
    let mut ids = Vec::new();
    let mut offset = 0;
    loop {
        let page = client.ok(
            json!({"type": "run.probes", "run_id": run["run"]["id"], "offset": offset, "count": 1}),
        );
        assert_eq!(page["origin"]["document_id"], doc["id"]);
        assert_eq!(page["origin"]["document_revision"], "1");
        assert_eq!(page["origin"]["effective_revision"], "1");
        assert_eq!(page["origin"], run["origin"]);
        assert!(page.get("evidence").is_none());
        ids.push(page["entries"][0]["id"].as_str().unwrap().to_owned());
        match page["next_offset"].as_u64() {
            Some(next) => offset = next,
            None => break,
        }
    }
    assert_eq!(ids, ["first", "second"]);
    for command in [
        "run.evidence",
        "run.links",
        "run.routes",
        "run.stp",
        "run.trace",
    ] {
        let page = client
            .ok(json!({"type": command, "run_id": run["run"]["id"], "offset": 0, "count": 128}));
        assert_eq!(page["origin"]["document_revision"], "1");
        assert_eq!(page["origin"], run["origin"]);
        assert!(page["entries"].as_array().unwrap().len() <= 128);
        if command == "run.trace" {
            assert_eq!(page["status"], "disabled");
        }
    }
    let beyond = client
        .ok(json!({"type": "run.probes", "run_id": run["run"]["id"], "offset": 999, "count": 128}));
    assert_eq!(beyond["entries"], json!([]));
    assert!(beyond["next_offset"].is_null());
}

#[test]
fn queued_origin_survives_edits_and_close_before_compilation() {
    let mut client = Client::new();
    client.hello();
    let parking_doc = client.open(IPV4);
    let parking = client.submit(&parking_doc, "paused");
    client.wait(&parking, &["paused"]);
    let mut input: Value = serde_json::from_str(IPV4).unwrap();
    input["revision"] = json!("submitted-native");
    let doc = client.open(&input.to_string());
    let edited = client.ok(json!({
        "type": "document.replace_presentation", "document_id": doc["id"],
        "expected_revision": "1", "presentation": "{}",
    }));
    let run = client.submit(&edited, "automatic");
    let queued = client.ok(json!({"type": "run.summary", "run_id": run["run"]["id"]}));
    assert_eq!(queued["summary"]["status"], "queued");
    assert_eq!(queued["origin"], run["origin"]);
    assert_eq!(queued["origin"]["document_revision"], "2");
    assert_eq!(queued["origin"]["effective_revision"], "1");
    assert_eq!(queued["origin"]["native_revision"], "submitted-native");
    input["revision"] = json!("later-native");
    client.ok(json!({
        "type": "document.replace_experiment", "document_id": doc["id"],
        "expected_revision": "2", "input": input.to_string(),
    }));
    client.ok(json!({
        "type": "document.close", "document_id": doc["id"],
        "expected_revision": "3", "discard_unsaved": true,
    }));
    client.ok(json!({"type": "run.control", "run_id": parking["run"]["id"], "control": "cancel"}));
    let finished = client.wait(&run, &["finished"]);
    assert_eq!(finished["origin"], run["origin"]);
    assert_eq!(finished["summary"]["origin"], queued["summary"]["origin"]);
}

#[test]
fn legacy_probe_results_use_the_same_page_shape_as_scenario_results() {
    let mut client = Client::new();
    client.hello();
    let doc = client.open(VLAN);
    let run = client.submit(&doc, "automatic");
    client.wait(&run, &["finished"]);
    let page = client
        .ok(json!({"type": "run.probes", "run_id": run["run"]["id"], "offset": 0, "count": 1}));
    assert_eq!(page["entries"].as_array().unwrap().len(), 1);
    assert_eq!(page["entries"][0]["id"], "1");
    assert_eq!(page["entries"][0]["outcome"]["status"], "timeout");
    assert_eq!(page["entries"][0]["at_ns"], "0");
    assert!(page["next_offset"].is_null());
}

#[test]
fn error_codes_follow_rust_variants_instead_of_the_message_text() {
    let document = BridgeError::from(WorkspaceError::InvalidDocument("实验待办已满。".into()));
    assert_eq!(document.code, ErrorCode::InvalidDocument);
    let queue = BridgeError::from(WorkspaceError::QueueFull);
    assert_eq!(queue.code, ErrorCode::QueueFull);
    let mut client = Client::with_config(WorkspaceConfig {
        max_documents: 1,
        ..WorkspaceConfig::default()
    });
    client.hello();
    client.open(VLAN);
    let full = client.call(json!({"type": "document.open", "input": IPV4}));
    assert_eq!(full["error"]["code"], "document_slots");
    let malformed = client.call(json!({"type": "document.open", "input": "{ broken"}));
    assert_eq!(malformed["error"]["code"], "json_error");
    assert!(malformed["error"]["details"]["line"].is_number());
    assert!(matches!(
        Backend::new(WorkspaceConfig {
            workers: 0,
            ..WorkspaceConfig::default()
        }),
        Err(WorkspaceError::InvalidConfig)
    ));
}

#[test]
fn pausing_a_queued_run_is_applied_before_its_first_event() {
    let mut client = Client::new();
    client.hello();
    let doc = client.open(IPV4);
    let held = client.submit(&doc, "paused");
    client.wait(&held, &["paused"]);
    let queued = client.submit(&doc, "automatic");
    client.ok(json!({"type": "run.control", "run_id": queued["run"]["id"], "control": "pause"}));
    client.ok(json!({"type": "run.control", "run_id": held["run"]["id"], "control": "cancel"}));
    let paused = client.wait(&queued, &["paused"]);
    assert!(paused["summary"]["last_event_id"].is_null());
    client.ok(json!({"type": "run.control", "run_id": queued["run"]["id"], "control": "cancel"}));
    client.wait(&queued, &["cancelled"]);
}

#[test]
fn pause_step_resume_and_cancel_have_separate_acknowledgements_and_final_states() {
    let mut client = Client::new();
    client.hello();
    let doc = client.open(IPV4);
    let run = client.submit(&doc, "paused");
    client.wait(&run, &["paused"]);
    let unavailable = client
        .call(json!({"type": "run.probes", "run_id": run["run"]["id"], "offset": 0, "count": 1}));
    assert_eq!(unavailable["error"]["code"], "report_unavailable");
    let ack =
        client.ok(json!({"type": "run.control", "run_id": run["run"]["id"], "control": "step"}));
    assert_eq!(ack["accepted"], true);
    let stepped = client.wait(&run, &["paused"]);
    assert_eq!(stepped["summary"]["last_event_id"], "1");
    client.ok(json!({"type": "run.control", "run_id": run["run"]["id"], "control": "resume"}));
    client.wait(&run, &["finished"]);
    let invalid = client
        .call(json!({"type": "run.control", "run_id": run["run"]["id"], "control": "cancel"}));
    assert_eq!(invalid["error"]["code"], "invalid_control");
    assert_eq!(invalid["error"]["details"]["control"], "cancel");
    let parking = client.submit(&doc, "paused");
    client.wait(&parking, &["paused"]);
    let queued = client.submit(&doc, "automatic");
    let ack = client
        .ok(json!({"type": "run.control", "run_id": queued["run"]["id"], "control": "cancel"}));
    assert_eq!(ack["accepted"], true);
    client.wait(&queued, &["cancelled"]);
    let replacement = client.submit(&doc, "automatic");
    client.ok(json!({"type": "run.control", "run_id": parking["run"]["id"], "control": "cancel"}));
    client.wait(&parking, &["cancelled"]);
    client.wait(&replacement, &["finished"]);
}

#[test]
fn stale_submit_save_rerun_and_close_do_not_act_on_a_later_revision() {
    let destination = TemporaryFile::new();
    let mut client = Client::new();
    client.hello();
    let doc = client.open(VLAN);
    client.ok(json!({"type": "document.replace_presentation", "document_id": doc["id"], "expected_revision": "1", "presentation": "{}"}));
    for command in [
        "run.submit",
        "document.save",
        "document.save_and_rerun",
        "document.close",
    ] {
        let mut request =
            json!({"type": command, "document_id": doc["id"], "expected_revision": "1"});
        if command == "document.save" || command == "document.save_and_rerun" {
            request["path"] = json!(destination.0);
        }
        if command == "document.close" {
            request["discard_unsaved"] = json!(true);
        }
        let response = client.call(request);
        assert_eq!(response["error"]["code"], "stale_revision", "{response}");
        assert_eq!(response["error"]["details"]["actual"], "2");
        assert!(!destination.0.exists());
    }
    let info = client.ok(json!({"type": "document.info", "document_id": doc["id"]}));
    assert_eq!(info["revision"], "2");
    assert!(info["dirty"].as_bool().unwrap());
    assert_eq!(client.submit(&info, "paused")["run"]["id"], "1");
}

#[test]
fn saving_succeeds_even_when_rerun_is_rejected_and_io_failure_does_not_submit() {
    let destination = TemporaryFile::new();
    let missing_parent = destination
        .0
        .with_extension("missing-parent")
        .join("lab.json");
    let mut client = Client::new();
    client.hello();
    let doc = client.open(VLAN);
    let parking = client.submit(&doc, "paused");
    client.wait(&parking, &["paused"]);
    let queued = client.submit(&doc, "automatic");
    let result = client.ok(json!({
        "type": "document.save_and_rerun", "document_id": doc["id"],
        "expected_revision": "1", "path": destination.0,
    }));
    assert_eq!(result["saved_revision"], "1");
    assert_eq!(result["rerun"]["status"], "rejected");
    assert_eq!(result["rerun"]["error"]["code"], "queue_full");
    assert_eq!(
        serde_json::from_str::<Value>(&fs::read_to_string(&destination.0).unwrap()).unwrap()["id"],
        "vlan-lab"
    );
    let info = client.ok(json!({"type": "document.info", "document_id": doc["id"]}));
    assert_eq!(info["dirty"], false);
    let failed = client.call(json!({"type": "document.save_and_rerun", "document_id": doc["id"], "expected_revision": "1", "path": missing_parent}));
    assert_eq!(failed["error"]["code"], "io_error");
    assert_eq!(failed["error"]["details"]["io_kind"], "not_found");
    client.ok(json!({"type": "run.control", "run_id": queued["run"]["id"], "control": "cancel"}));
    let next = client.submit(&doc, "automatic");
    assert_eq!(next["run"]["id"], "3");
    client.ok(json!({"type": "run.control", "run_id": parking["run"]["id"], "control": "cancel"}));
    client.wait(&next, &["finished"]);
}

#[test]
fn competing_jsonl_processes_report_a_disk_conflict() {
    use std::{
        io::{BufRead, BufReader, Write},
        process::{Command, Stdio},
        sync::Barrier,
    };

    let file = TemporaryFile::new();
    fs::write(&file.0, VLAN).unwrap();
    let path = &file.0;
    let clients = [17, 19].map(|x| {
        let mut child = Command::new(env!("CARGO_BIN_EXE_reliner"))
            .args(["workspace", "--jsonl"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut input = child.stdin.take().unwrap();
        let mut output = BufReader::new(child.stdout.take().unwrap());
        let mut session = Value::Null;
        let mut call = move |cmd: Value| {
            writeln!(
                input,
                "{}",
                json!({"protocol_version":1,"request_id":"1","session_id":session,"command":cmd})
            )
            .unwrap();
            let mut line = String::new();
            output.read_line(&mut line).unwrap();
            let response: Value = serde_json::from_str(&line).unwrap();
            session = response["session_id"].clone();
            response
        };
        assert_eq!(call(json!({"type":"hello"}))["status"], "ok");
        let opened = call(json!({"type":"document.open","path":file.0}));
        assert_eq!(opened["status"], "ok", "{opened}");
        let id = opened["data"]["id"].clone();
        let edited = call(
            json!({"type":"document.replace_presentation","document_id":id,
            "expected_revision":"1","presentation":format!(r#"{{"x":{x}}}"#)}),
        );
        assert_eq!(edited["status"], "ok", "{edited}");
        move |start: &Barrier| {
            start.wait();
            let response = call(json!({"type":"document.save","document_id":id,
                "expected_revision":"2","path":path}));
            drop(call);
            assert!(child.wait().unwrap().success());
            response
        }
    });
    let start = Barrier::new(2);
    let [a, b] = clients;
    let results = thread::scope(|scope| {
        let first = scope.spawn(|| a(&start));
        let second = b(&start);
        [first.join().unwrap(), second]
    });
    assert_eq!(
        results.iter().filter(|r| r["status"] == "ok").count(),
        1,
        "{results:?}"
    );
    let winner = results.iter().position(|r| r["status"] == "ok").unwrap();
    let error = &results[1 - winner]["error"];
    assert_eq!(error["code"], "save_conflict", "{error}");
    assert_eq!(
        error["details"]["path"],
        json!(fs::canonicalize(&file.0).unwrap())
    );
    let disk: Value = serde_json::from_slice(&fs::read(&file.0).unwrap()).unwrap();
    assert_eq!(disk["presentation"]["x"], [17, 19][winner]);
}

#[test]
fn saved_rerun_and_reopened_file_keep_the_same_saved_input() {
    let destination = TemporaryFile::new();
    let mut client = Client::new();
    client.hello();
    let doc = client.open(IPV4);
    let result = client.ok(json!({"type": "document.save_and_rerun", "document_id": doc["id"], "expected_revision": "1", "path": destination.0}));
    assert_eq!(result["rerun"]["status"], "accepted");
    assert_eq!(result["rerun"]["origin"]["document_revision"], "1");
    assert_eq!(result["rerun"]["origin"]["topology_id"], "ipv4-lab");
    assert_eq!(result["rerun"]["origin"]["native_revision"], "1");
    let run = json!({"run": result["rerun"]["run"]});
    let finished = client.wait(&run, &["finished"]);
    assert_eq!(finished["origin"], result["rerun"]["origin"]);
    client
        .ok(json!({"type": "document.close", "document_id": doc["id"], "expected_revision": "1"}));
    let reopened = client.ok(json!({"type": "document.open", "path": destination.0}));
    assert_eq!(reopened["dirty"], false);
    let saved = client.ok(json!({"type": "document.save", "document_id": reopened["id"], "expected_revision": "1", "path": destination.0}));
    assert_eq!(saved["saved_revision"], "1");
}

#[test]
fn dirty_close_and_future_format_preview_keep_the_original_content() {
    let file = TemporaryFile::new();
    let text = "\u{feff}{\"format_version\":37,\"future\":true}\n";
    fs::write(&file.0, text).unwrap();
    let mut client = Client::new();
    client.hello();
    let preview = client.ok(json!({"type": "document.preview_file", "path": file.0}));
    assert_eq!(preview["text"], text);
    assert_eq!(preview["format"]["status"], "unsupported");
    let doc = client.open(VLAN);
    let denied = client.call(
        json!({"type": "document.close", "document_id": doc["id"], "expected_revision": "1"}),
    );
    assert_eq!(denied["error"]["code"], "unsaved_document");
    client.ok(json!({"type": "document.close", "document_id": doc["id"], "expected_revision": "1", "discard_unsaved": true}));
    assert_eq!(fs::read_to_string(&file.0).unwrap(), text);
}

#[test]
fn archive_export_creates_once_preserves_existing_files_and_removes_failed_outputs() {
    let archive = TemporaryFile::new();
    let failed_output = TemporaryFile::new();
    let mut client = Client::new();
    client.hello();
    let doc = client.open(IPV4);
    let run = client.submit(&doc, "automatic");
    client.wait(&run, &["finished"]);
    let exported =
        client.ok(json!({"type": "run.archive_to", "run_id": run["run"]["id"], "path": archive.0}));
    assert_eq!(exported["origin"]["document_revision"], "1");
    let bytes = fs::read(&archive.0).unwrap();
    let archived = reliner::read_archive(bytes.as_slice()).unwrap();
    assert_eq!(archived.source.unwrap().document_revision, "1");
    let duplicate = client
        .call(json!({"type": "run.archive_to", "run_id": run["run"]["id"], "path": archive.0}));
    assert_eq!(duplicate["error"]["code"], "io_error");
    assert_eq!(duplicate["error"]["details"]["io_kind"], "already_exists");
    assert_eq!(fs::read(&archive.0).unwrap(), bytes);
    let held = client.submit(&doc, "paused");
    client.wait(&held, &["paused"]);
    let duplicate = client
        .call(json!({"type": "run.archive_to", "run_id": held["run"]["id"], "path": archive.0}));
    assert_eq!(duplicate["error"]["code"], "io_error");
    assert_eq!(duplicate["error"]["details"]["io_kind"], "already_exists");
    assert_eq!(fs::read(&archive.0).unwrap(), bytes);
    let unavailable = client.call(
        json!({"type": "run.archive_to", "run_id": held["run"]["id"], "path": failed_output.0}),
    );
    assert_eq!(unavailable["error"]["code"], "report_unavailable");
    assert!(!failed_output.0.exists());
}

#[test]
fn comparison_checks_both_run_ids_before_report_availability() {
    let mut client = Client::with_config(WorkspaceConfig {
        history_runs: 1,
        ..WorkspaceConfig::default()
    });
    client.hello();
    let doc = client.open(IPV4);
    let old = client.submit(&doc, "automatic");
    client.wait(&old, &["finished"]);
    let new = client.submit(&doc, "automatic");
    client.wait(&new, &["finished"]);
    let held = client.submit(&doc, "paused");
    client.wait(&held, &["paused"]);
    let missing = client.call(json!({
        "type": "run.compare", "baseline": held["run"]["id"], "candidate": old["run"]["id"],
        "offset": 0, "count": 1
    }));
    assert_eq!(missing["error"]["code"], "unknown_run");
    assert_eq!(missing["error"]["details"]["run_id"], old["run"]["id"]);
    let unavailable = client.call(json!({
        "type": "run.compare", "baseline": held["run"]["id"], "candidate": new["run"]["id"],
        "offset": 0, "count": 1
    }));
    assert_eq!(unavailable["error"]["code"], "report_unavailable");
}

#[test]
fn history_and_trace_eviction_are_not_reported_as_successful_empty_results() {
    let mut client = Client::with_config(WorkspaceConfig {
        history_runs: 1,
        history_trace_entries: 0,
        ..WorkspaceConfig::default()
    });
    client.hello();
    let mut input: Value = serde_json::from_str(IPV4).unwrap();
    input["scenario"]["trace_limit"] = json!(8);
    let doc = client.open(&input.to_string());
    let first = client.submit(&doc, "automatic");
    client.wait(&first, &["finished"]);
    let trace = client
        .ok(json!({"type": "run.trace", "run_id": first["run"]["id"], "offset": 0, "count": 1}));
    assert_eq!(trace["status"], "evicted");
    assert_eq!(trace["entries"], json!([]));
    let evidence = client
        .ok(json!({"type": "run.evidence", "run_id": first["run"]["id"], "offset": 0, "count": 1}));
    assert_eq!(evidence["entries"].as_array().unwrap().len(), 1);
    let second = client.submit(&doc, "automatic");
    client.wait(&second, &["finished"]);
    let gone = client
        .call(json!({"type": "run.probes", "run_id": first["run"]["id"], "offset": 0, "count": 1}));
    assert_eq!(gone["error"]["code"], "unknown_run");
}

#[test]
fn campus_link_route_and_stp_pages_contain_the_referenced_events() {
    let mut client = Client::new();
    client.hello();
    let doc = client.open(CAMPUS);
    let run = client.submit(&doc, "automatic");
    client.wait(&run, &["finished"]);
    for command in ["run.links", "run.routes", "run.stp"] {
        let page = client
            .ok(json!({"type": command, "run_id": run["run"]["id"], "offset": 0, "count": 1}));
        assert_eq!(
            page["entries"].as_array().unwrap().len(),
            1,
            "{command}: {page}"
        );
        assert!(page["entries"][0]["event_id"].is_string());
        assert!(page["entries"][0]["at_ns"].is_string());
        if command == "run.stp" {
            assert!(page["entries"][0].get("bridges").is_none());
            assert!(page["entries"][0].get("links").is_none());
            assert!(page["entries"][0]["bridge_count"].is_number());
            assert!(page["entries"][0]["link_count"].is_number());
        }
    }
}

#[test]
fn new_page_identifiers_require_decimal_strings() {
    let mut client = Client::new();
    client.hello();
    for command in [
        json!({"type": "run.stp_bridges", "run_id": "1", "event_id": 1, "offset": 0, "count": 1}),
        json!({"type": "run.stp_links", "run_id": "1", "event_id": 1, "offset": 0, "count": 1}),
        json!({"type": "run.compare", "baseline": 1, "candidate": "2", "offset": 0, "count": 1}),
        json!({"type": "run.import_gaps", "run_id": "1", "offset": 0, "count": 1,
            "reference": {"source_component": 1, "destination_component": "1"}}),
    ] {
        let result = client.call(command);
        assert_eq!(result["error"]["code"], "invalid_request", "{result}");
    }
}

#[test]
fn comparison_pages_keep_both_origins_and_align_added_removed_and_changed_probes() {
    let mut client = Client::new();
    client.hello();
    let doc = client.open(IPV4);
    let baseline = client.submit(&doc, "automatic");
    client.wait(&baseline, &["finished"]);
    let mut input: Value = serde_json::from_str(IPV4).unwrap();
    input["revision"] = json!("candidate-native");
    let mut changed = input["scenario"]["probes"][1].clone();
    changed["ttl"] = json!(32);
    let mut added = input["scenario"]["probes"][0].clone();
    added["id"] = json!("third");
    input["scenario"]["probes"] = json!([changed, added]);
    let edited = client.ok(json!({
        "type": "document.replace_experiment", "document_id": doc["id"],
        "expected_revision": "1", "input": input.to_string(),
    }));
    let candidate = client.submit(&edited, "automatic");
    client.wait(&candidate, &["finished"]);
    client.ok(json!({"type": "document.close", "document_id": doc["id"],
        "expected_revision": "2", "discard_unsaved": true}));
    let request = json!({"type": "run.compare", "baseline": baseline["run"]["id"],
        "candidate": candidate["run"]["id"], "offset": 0, "count": 1});
    let first = client.ok(request.clone());
    assert_eq!(first["baseline_origin"], baseline["origin"]);
    assert_eq!(first["candidate_origin"], candidate["origin"]);
    assert_eq!(first["baseline"]["revision"], "1");
    assert_eq!(first["candidate"]["revision"], "candidate-native");
    assert!(first.get("probes").is_none());
    let rows = client.entries(request);
    let ids: Vec<_> = rows
        .iter()
        .map(|row| row["probe_id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["first", "second", "third"]);
    assert!(rows[0]["candidate"].is_null());
    assert_eq!(rows[0]["baseline"]["outcome"]["status"], "reachable");
    assert_eq!(rows[1]["baseline"]["ttl"], 64);
    assert_eq!(rows[1]["candidate"]["ttl"], 32);
    assert_eq!(rows[1]["same_probe_parameters"], false);
    assert!(rows[2]["baseline"].is_null());
    assert!(rows[2]["candidate"]["request_evidence_id"].is_string());
    let beyond = client.ok(
        json!({"type": "run.compare", "baseline": baseline["run"]["id"],
        "candidate": candidate["run"]["id"], "offset": 999, "count": 128}),
    );
    assert_eq!(beyond["entries"], json!([]));
    assert!(beyond["next_offset"].is_null());
    let same = client.ok(
        json!({"type": "run.compare", "baseline": baseline["run"]["id"],
        "candidate": baseline["run"]["id"], "offset": 0, "count": 128}),
    );
    assert_eq!(same["entries"].as_array().unwrap().len(), 2);
    assert!(
        same["entries"]
            .as_array()
            .unwrap()
            .iter()
            .all(|row| row["same_probe_parameters"] == true)
    );
    let unknown = client.call(
        json!({"type": "run.compare", "baseline": "18446744073709551615",
        "candidate": candidate["run"]["id"], "offset": 0, "count": 1}),
    );
    assert_eq!(unknown["error"]["code"], "unknown_run");
}

#[test]
fn stp_summary_and_row_pages_follow_old_evidence_after_a_repair_and_close() {
    let mut client = Client::new();
    client.hello();
    let doc = client.open(STP);
    let original = client.submit(&doc, "automatic");
    client.wait(&original, &["finished"]);
    let summaries = client
        .ok(json!({"type": "run.stp", "run_id": original["run"]["id"], "offset": 0, "count": 128}));
    let header = &summaries["entries"][0];
    assert_eq!(header["bridge_count"], 3);
    assert_eq!(header["link_count"], 3);
    assert!(header.get("bridges").is_none());
    assert!(header.get("links").is_none());
    let event_id = header["event_id"].clone();
    let evidence = client
        .entries(json!({"type": "run.evidence", "run_id": original["run"]["id"], "count": 128}));
    assert!(
        evidence
            .iter()
            .any(|entry| entry["stp_event_id"] == event_id)
    );
    let mut input: Value = serde_json::from_str(STP).unwrap();
    input["revision"] = json!("repaired-native");
    input["devices"][3]["ports"][0]["stp_cost"] = json!(100);
    let edited = client.ok(
        json!({"type": "document.replace_experiment", "document_id": doc["id"],
        "expected_revision": "1", "input": input.to_string()}),
    );
    let repaired = client.submit(&edited, "automatic");
    client.wait(&repaired, &["finished"]);
    client.ok(json!({"type": "document.close", "document_id": doc["id"], "expected_revision": "2", "discard_unsaved": true}));
    let old_bridges = client.entries(
        json!({"type": "run.stp_bridges", "run_id": original["run"]["id"],
        "event_id": event_id, "count": 1}),
    );
    assert_eq!(old_bridges.len(), 3);
    let sw3 = old_bridges
        .iter()
        .find(|row| row["bridge_id"] == "sw3")
        .unwrap();
    assert_eq!(sw3["root_port_id"], "s3-s1");
    let old_links = client.entries(
        json!({"type": "run.stp_links", "run_id": original["run"]["id"],
        "event_id": event_id, "count": 1}),
    );
    assert_eq!(old_links.len(), 3);
    let old_port = old_links
        .iter()
        .flat_map(|row| row["ports"].as_array().unwrap())
        .find(|port| port["port_id"] == "s3-s1")
        .unwrap();
    assert_eq!(old_port["path_cost"], 1);
    assert_eq!(old_port["role"], "root");
    let new_header = client
        .ok(json!({"type": "run.stp", "run_id": repaired["run"]["id"], "offset": 0, "count": 1}));
    let new_links = client.entries(
        json!({"type": "run.stp_links", "run_id": repaired["run"]["id"],
        "event_id": new_header["entries"][0]["event_id"], "count": 128}),
    );
    let new_port = new_links
        .iter()
        .flat_map(|row| row["ports"].as_array().unwrap())
        .find(|port| port["port_id"] == "s3-s1")
        .unwrap();
    assert_eq!(new_port["path_cost"], 100);
    assert_eq!(new_port["role"], "alternate");
    for command in ["run.stp_bridges", "run.stp_links"] {
        let missing = client.call(json!({"type": command, "run_id": original["run"]["id"],
            "event_id": "18446744073709551615", "offset": 0, "count": 1}));
        assert_eq!(missing["error"]["code"], "unknown_stp_event");
        assert_eq!(
            missing["error"]["details"]["event_id"],
            "18446744073709551615"
        );
        let empty = client.ok(json!({"type": command, "run_id": original["run"]["id"],
            "event_id": event_id, "offset": 999, "count": 128}));
        assert_eq!(empty["origin"], original["origin"]);
        assert_eq!(empty["event_id"], event_id);
        assert_eq!(empty["entries"], json!([]));
        assert!(empty["next_offset"].is_null());
    }
}

#[test]
fn import_gap_pages_filter_old_coverage_without_shipping_raw_values() {
    let mut client = Client::new();
    client.hello();
    let raw = json!({"raw_config": "x".repeat(16_384)});
    let mut input: Value = serde_json::from_str(IPV4).unwrap();
    input["format_version"] = json!(3);
    input["import_source"] = json!({"format": "containerlab", "source_name": "gaps.clab.yml", "gaps": [
        {"id": "return-policy", "source_path": "/topology/nodes/r2/exec/0", "scope": {"kind": "device", "device_id": "r2"},
            "value": raw, "message": "原回程策略尚未映射"},
        {"id": "global-policy", "source_path": "/policy", "scope": {"kind": "all"},
            "value": "global raw", "message": "全局缺项"},
        {"id": "isolated-policy", "source_path": "/topology/nodes/pc-c", "scope": {"kind": "device", "device_id": "pc-c"},
            "value": "isolated raw", "message": "另一组件缺项"}
    ]});
    input["devices"].as_array_mut().unwrap().push(json!({
        "kind": "host", "id": "pc-c", "name": "PC-C", "ipv4": "198.18.0.1", "prefix_length": 24,
        "mac": "02:00:00:00:00:0c", "port": {"id": "c-eth0", "name": "eth0"}
    }));
    let doc = client.open(&input.to_string());
    let run = client.submit(&doc, "automatic");
    client.wait(&run, &["finished"]);
    let probes = client
        .ok(json!({"type": "run.probes", "run_id": run["run"]["id"], "offset": 0, "count": 1}));
    let reference = probes["entries"][0]["outcome"]["coverage"].clone();
    assert_eq!(
        reference,
        json!({"source_component": "1", "destination_component": "1"})
    );
    input["import_source"]["gaps"][0]["value"] = json!("later raw");
    input["import_source"]["gaps"][0]["message"] = json!("后来的策略说明");
    client.ok(json!({"type": "document.replace_experiment", "document_id": doc["id"], "expected_revision": "1", "input": input.to_string()}));
    client.ok(json!({"type": "document.close", "document_id": doc["id"], "expected_revision": "2", "discard_unsaved": true}));
    let all =
        client.entries(json!({"type": "run.import_gaps", "run_id": run["run"]["id"], "count": 1}));
    let all_ids: Vec<_> = all.iter().map(|row| row["id"].as_str().unwrap()).collect();
    assert_eq!(
        all_ids,
        ["return-policy", "global-policy", "isolated-policy"]
    );
    assert!(all.iter().all(|row| row.get("value").is_none()));
    let affected = client.entries(json!({"type": "run.import_gaps", "run_id": run["run"]["id"], "reference": reference, "count": 1}));
    let ids: Vec<_> = affected
        .iter()
        .map(|row| row["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["return-policy", "global-policy"]);
    assert_eq!(affected[0]["message"], "原回程策略尚未映射");
    let detail = client.ok(
        json!({"type": "run.import_gap", "run_id": run["run"]["id"], "gap_id": "return-policy"}),
    );
    assert_eq!(detail["origin"], run["origin"]);
    assert_eq!(detail["gap"]["value"], raw);
    assert_eq!(detail["gap"]["message"], "原回程策略尚未映射");
    let invalid_reference =
        json!({"source_component": "18446744073709551615", "destination_component": "1"});
    let invalid = client.call(
        json!({"type": "run.import_gaps", "run_id": run["run"]["id"],
        "reference": invalid_reference, "offset": 0, "count": 1}),
    );
    assert_eq!(invalid["error"]["code"], "invalid_coverage_reference");
    assert_eq!(invalid["error"]["details"]["reference"], invalid_reference);
    let missing = client
        .call(json!({"type": "run.import_gap", "run_id": run["run"]["id"], "gap_id": "missing"}));
    assert_eq!(missing["error"]["code"], "unknown_import_gap");
    assert_eq!(missing["error"]["details"]["gap_id"], "missing");
    let beyond = client.ok(
        json!({"type": "run.import_gaps", "run_id": run["run"]["id"], "offset": 999, "count": 128}),
    );
    assert_eq!(beyond["entries"], json!([]));
    assert!(beyond["next_offset"].is_null());
}

#[test]
fn undetermined_outcomes_preserve_modeled_states_coverage_and_separate_counts() {
    for (modeled, terminal) in [
        ("reachable", "finished"),
        ("timeout", "finished"),
        ("incomplete", "incomplete"),
    ] {
        let mut config = WorkspaceConfig::default();
        if modeled == "incomplete" {
            config.run_limits.pending_events = 1;
        }
        let mut client = Client::with_config(config);
        client.hello();
        let mut input: Value = serde_json::from_str(IPV4).unwrap();
        input["format_version"] = json!(3);
        input["import_source"] = json!({
            "format": "containerlab",
            "source_name": "coverage.clab.yml",
            "gaps": [{
                "id": "return-policy",
                "source_path": "/topology/nodes/r2/exec/0",
                "scope": {"kind": "device", "device_id": "r2"},
                "value": "unsupported return policy",
                "message": "回程策略尚未映射",
            }],
        });
        if modeled == "timeout" {
            input["devices"][2]["routes"] = json!([]);
        }
        let doc = client.open(&input.to_string());
        assert_eq!(doc["format_version"], 3);
        let validated = client.ok(json!({
            "type": "document.validate", "document_id": doc["id"],
            "expected_revision": "1",
        }));
        assert_eq!(validated["status"], "valid");
        let run = client.submit(&doc, "automatic");
        let done = client.wait(&run, &[terminal]);
        let counts = &done["summary"]["probe_counts"];
        assert_eq!(counts["undetermined"], 2, "{modeled}: {done}");
        assert_eq!(counts["reachable"], 0);
        assert_eq!(counts["timed_out"], 0);
        if modeled == "incomplete" {
            assert_eq!(counts["completed"], 0);
            assert_eq!(counts["incomplete"], 2);
        } else {
            assert_eq!(counts["completed"], 2);
            assert_eq!(counts["incomplete"], 0);
        }
        let page = client.ok(json!({
            "type": "run.probes", "run_id": run["run"]["id"],
            "offset": 0, "count": 1,
        }));
        let outcome = &page["entries"][0]["outcome"];
        assert_eq!(outcome["status"], "undetermined");
        assert_eq!(outcome["modeled"]["status"], modeled);
        assert!(outcome["modeled"]["at_ns"].is_string());
        assert_eq!(outcome["coverage"]["source_component"], "1");
        assert_eq!(outcome["coverage"]["destination_component"], "1");
        if modeled == "reachable" {
            assert!(outcome["modeled"]["evidence_id"].is_string());
        } else if modeled == "timeout" {
            assert!(outcome["modeled"]["waiting_for"].is_string());
            assert!(outcome["modeled"]["cause"].is_string());
        } else {
            assert!(!outcome["modeled"]["reason"].is_null());
        }
        let decoded: reliner::Outcome = serde_json::from_value(outcome.clone()).unwrap();
        assert_eq!(serde_json::to_value(decoded).unwrap(), *outcome);
    }
}
