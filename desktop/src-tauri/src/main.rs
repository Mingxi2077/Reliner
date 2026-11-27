#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    reliner_desktop_lib::run()
}

#[cfg(test)]
mod tests {
    use reliner_desktop_lib::bridge::{list_examples, read_example_text, BridgeManager};
    use serde_json::json;
    use std::thread;
    use std::time::{Duration, Instant};

    #[test]
    fn handshake_open_example_submit_and_read_vlan_fault() {
        let manager = BridgeManager::new();
        let hello = manager.handshake().expect("handshake succeeds");
        assert!(!hello.session_id.is_empty());
        assert_eq!(hello.hello["protocol_version"], 1);
        assert_eq!(hello.hello["backend_api_version"], 2);

        let examples = list_examples().expect("list examples");
        assert!(examples.iter().any(|e| e.file_name == "vlan-lab.json"));

        let vlan_text = read_example_text("vlan-lab.json").expect("read vlan-lab.json");
        let open_resp = manager
            .call(json!({ "type": "document.open", "input": vlan_text }))
            .expect("open vlan-lab");
        assert_eq!(open_resp["status"], "ok", "{open_resp}");
        let doc_id = open_resp["data"]["id"].as_str().unwrap().to_string();
        let revision = open_resp["data"]["revision"].as_str().unwrap().to_string();

        let submit_resp = manager
            .call(json!({
                "type": "run.submit",
                "document_id": doc_id,
                "expected_revision": revision,
                "mode": "automatic"
            }))
            .expect("submit run");
        assert_eq!(submit_resp["status"], "ok", "{submit_resp}");
        let run_id = submit_resp["data"]["run"]["id"]
            .as_str()
            .unwrap()
            .to_string();

        let deadline = Instant::now() + Duration::from_secs(10);
        let summary = loop {
            let resp = manager
                .call(json!({ "type": "run.summary", "run_id": run_id }))
                .expect("poll summary");
            assert_eq!(resp["status"], "ok", "{resp}");
            let status = resp["data"]["summary"]["status"].as_str().unwrap();
            if matches!(status, "finished" | "incomplete" | "cancelled" | "failed") {
                break resp["data"]["summary"].clone();
            }
            assert!(Instant::now() < deadline, "run timed out");
            thread::sleep(Duration::from_millis(5));
        };
        assert_eq!(summary["status"], "finished");
        assert_eq!(summary["probe_counts"]["timed_out"], 1);

        let probes = manager
            .call(json!({ "type": "run.probes", "run_id": run_id, "offset": 0, "count": 10 }))
            .expect("read probes");
        assert_eq!(probes["status"], "ok");
        assert_eq!(probes["data"]["entries"][0]["outcome"]["status"], "timeout");
        let cause_id = probes["data"]["entries"][0]["outcome"]["cause"]
            .as_str()
            .expect("fault cause evidence id");

        let evidence = manager
            .call(json!({ "type": "run.evidence", "run_id": run_id, "offset": 0, "count": 20 }))
            .expect("read evidence");
        assert_eq!(evidence["status"], "ok");
        let cause = evidence["data"]["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["id"].as_str() == Some(cause_id))
            .expect("cause evidence exists");
        assert_eq!(cause["port"]["port_id"], "sw2-uplink");
        assert_eq!(cause["fact"]["kind"], "discarded");
        assert_eq!(cause["fact"]["reason"]["rule"], "trunk_vlan_not_allowed");

        manager.shutdown();
    }
}
