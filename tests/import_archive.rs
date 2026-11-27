use reliner::{RunLimits, RunReport, read_archive, run_lab, run_lab_with_limits, write_archive};
use serde_json::{Value, json};

const RAW_MARKER: &str = "RAW_CONFIG_MUST_NOT_APPEAR_IN_ERRORS";

fn lab() -> Value {
    let mut doc: Value = serde_json::from_str(include_str!("../examples/ipv4-lab.json")).unwrap();
    doc["format_version"] = json!(3);
    doc["import_source"] = json!({
        "format": "containerlab",
        "source_name": "example.clab.yml",
        "metadata": {"description": "保留来源信息"},
        "gaps": [
            gap("device-policy", json!({"kind": "device", "device_id": "r2"})),
            gap("port-policy", json!({"kind": "port", "port_id": "r2-lan"})),
            gap("link-policy", json!({"kind": "link", "link_id": "r2-b"}))
        ]
    });
    doc["devices"].as_array_mut().unwrap().extend([
        json!({"kind": "host", "id": "pc-c", "name": "PC-C",
            "ipv4": "198.18.0.1", "prefix_length": 24,
            "mac": "02:00:00:00:00:0c", "port": {"id": "c-eth0", "name": "eth0"}}),
        json!({"kind": "host", "id": "pc-d", "name": "PC-D",
            "ipv4": "198.18.0.2", "prefix_length": 24,
            "mac": "02:00:00:00:00:0d", "port": {"id": "d-eth0", "name": "eth0"}}),
    ]);
    doc["links"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id": "c-d", "ports": ["c-eth0", "d-eth0"], "delay_ns": "100"}));
    doc["scenario"]["probes"]
        .as_array_mut()
        .unwrap()
        .push(json!({
            "id": "independent", "source": "pc-c", "destination": "pc-d",
            "at_ns": "0", "deadline_ns": "2000"
        }));
    doc
}

fn gap(id: &str, scope: Value) -> Value {
    json!({
        "id": id, "source_path": "/topology/unsupported", "scope": scope,
        "value": {"opaque": RAW_MARKER}, "message": "未映射配置"
    })
}

fn imported() -> Value {
    envelope(&run_lab(&lab().to_string()).unwrap())
}

fn envelope(report: &RunReport) -> Value {
    json!({"archive_version": 1, "report": report})
}

fn assert_round_trip(value: Value) {
    let restored = read_archive(serde_json::to_vec(&value).unwrap().as_slice()).unwrap();
    assert_eq!(
        serde_json::to_value(&restored.report).unwrap(),
        value["report"]
    );
    let mut bytes = Vec::new();
    write_archive(&restored.report, None, None, &mut bytes).unwrap();
    let reopened = read_archive(bytes.as_slice()).unwrap();
    assert_eq!(reopened.report, restored.report);
    let mut rewritten = Vec::new();
    write_archive(&reopened.report, None, None, &mut rewritten).unwrap();
    assert_eq!(rewritten, bytes);
}

fn assert_rejected(value: Value, field: &str) {
    let bytes = serde_json::to_vec(&value).unwrap();
    let read_error = read_archive(bytes.as_slice()).unwrap_err().to_string();
    assert!(read_error.contains(field), "{read_error}");
    assert!(!read_error.contains(RAW_MARKER));

    let report: RunReport = serde_json::from_value(value["report"].clone()).unwrap();
    let write_error = write_archive(&report, None, None, Vec::new())
        .unwrap_err()
        .to_string();
    assert!(write_error.contains(field), "{write_error}");
    assert!(!write_error.contains(RAW_MARKER));
}

#[test]
fn imported_facts_and_shared_coverage_round_trip_without_optional_trace() {
    let value = imported();
    assert_eq!(value["report"]["report_version"], 8);
    assert_eq!(value["report"]["model_version"], 14);
    assert_eq!(
        value["report"]["probes"][0]["outcome"]["status"],
        "undetermined"
    );
    assert_eq!(
        value["report"]["probes"][2]["outcome"]["status"],
        "reachable"
    );
    assert_eq!(value["report"]["trace"]["status"], "disabled");
    assert_round_trip(value);
}

#[test]
fn an_unsubmitted_probe_keeps_unknown_coverage_and_the_actual_stop() {
    let report = run_lab_with_limits(
        &lab().to_string(),
        RunLimits {
            pending_events: 1,
            ..Default::default()
        },
    )
    .unwrap();
    let value = envelope(&report);
    assert!(value["report"]["last_event_id"].is_null());
    assert!(value["report"]["evidence"].as_array().unwrap().is_empty());
    assert_eq!(
        value["report"]["probes"][0]["outcome"]["modeled"]["status"],
        "incomplete"
    );
    assert_eq!(report.probe_counts().completed, 0);
    assert_eq!(report.probe_counts().incomplete, 3);
    assert_eq!(report.probe_counts().undetermined, 2);
    assert_round_trip(value);
}

#[test]
fn missing_and_empty_sources_need_no_fabricated_component_table() {
    for with_source in [false, true] {
        let mut doc = lab();
        if with_source {
            doc["import_source"]["gaps"] = json!([]);
        } else {
            doc.as_object_mut().unwrap().remove("import_source");
        }
        let original = envelope(&run_lab(&doc.to_string()).unwrap());
        assert_eq!(original["report"]["model_version"], 14);
        assert_round_trip(original.clone());
        for coverage in [Value::Null, json!({"global_gaps": [], "components": []})] {
            let mut value = original.clone();
            value["report"]["coverage"] = coverage;
            // Optional fields are compared in their canonical serialized form.
            let report: RunReport = serde_json::from_value(value["report"].clone()).unwrap();
            assert_round_trip(envelope(&report));
        }
    }
}

#[test]
fn report_eight_accepts_a_later_model_but_rejects_an_older_one() {
    let mut value = imported();
    value["report"]["model_version"] = json!(12);
    assert_round_trip(value);
    let mut value = imported();
    value["report"]["model_version"] = json!(10);
    assert_rejected(value, "/report/model_version");
}

#[test]
fn each_old_scenario_version_rejects_each_new_semantic_feature() {
    let ordinary = envelope(&run_lab(include_str!("../examples/ipv4-lab.json")).unwrap());
    let new = imported();
    for version in 2..=7 {
        for feature in ["import_source", "coverage", "outcome", "model_only"] {
            let mut value = ordinary.clone();
            value["report"]["report_version"] = json!(version);
            let field = match feature {
                "import_source" => {
                    value["report"][feature] = new["report"][feature].clone();
                    "/report/import_source"
                }
                "coverage" => {
                    value["report"][feature] = new["report"][feature].clone();
                    "/report/coverage"
                }
                "outcome" => {
                    let modeled = value["report"]["probes"][0][feature].clone();
                    value["report"]["probes"][0][feature] = json!({
                        "status": "undetermined", "modeled": modeled,
                        "coverage": {"source_component": "1", "destination_component": "1"}
                    });
                    "/report/probes/0/outcome"
                }
                _ => {
                    value["report"]["evidence"][0][feature] = json!(true);
                    "/report/evidence/0/model_only"
                }
            };
            assert_rejected(value, field);
        }
    }
}

#[test]
fn old_reports_keep_their_serialized_bytes() {
    for input in [
        include_str!("../examples/ipv4-lab.json"),
        include_str!("../examples/failover-lab.json"),
        include_str!("../examples/mtu-lab.json"),
        include_str!("../examples/congestion-lab.json"),
        include_str!("../examples/routing-lab.json"),
        include_str!("../examples/stp-lab.json"),
    ] {
        let report = run_lab(input).unwrap();
        let original = serde_json::to_vec(&report).unwrap();
        let mut bytes = Vec::new();
        write_archive(&report, None, None, &mut bytes).unwrap();
        let restored = read_archive(bytes.as_slice()).unwrap();
        assert_eq!(serde_json::to_vec(&restored.report).unwrap(), original);
    }
}

#[test]
fn gaps_require_a_coverage_table_and_nonempty_unique_definitions() {
    let mut value = imported();
    value["report"].as_object_mut().unwrap().remove("coverage");
    assert_rejected(value, "/report/coverage");

    let mut value = imported();
    let duplicate = value["report"]["import_source"]["gaps"][0].clone();
    value["report"]["import_source"]["gaps"]
        .as_array_mut()
        .unwrap()
        .push(duplicate);
    assert_rejected(value, "/report/import_source/gaps/3/id");

    let mut value = imported();
    value["report"]["import_source"]["gaps"][0]["id"] = json!(" ");
    assert_rejected(value, "/report/import_source/gaps/0/id");
}

#[test]
fn source_diagnostics_name_fields_without_dumping_raw_configuration() {
    for (pointer, invalid) in [
        ("/report/import_source/format", json!(" ")),
        ("/report/import_source/source_name", json!("")),
        (
            "/report/import_source/gaps/0/source_path",
            json!("not-a-path"),
        ),
        ("/report/import_source/gaps/0/message", json!(" ")),
    ] {
        let mut value = imported();
        *value.pointer_mut(pointer).unwrap() = invalid;
        assert_rejected(value, pointer);
    }
}

#[test]
fn component_ids_and_typed_members_cannot_be_duplicated_or_empty() {
    for member in ["devices", "ports", "links"] {
        let mut value = imported();
        let repeated = value["report"]["coverage"]["components"][0][member][0].clone();
        value["report"]["coverage"]["components"][1][member]
            .as_array_mut()
            .unwrap()
            .push(repeated);
        assert_rejected(value, &format!("/report/coverage/components/1/{member}"));

        let mut value = imported();
        value["report"]["coverage"]["components"][0][member]
            .as_array_mut()
            .unwrap()
            .push(json!(" "));
        assert_rejected(value, &format!("/report/coverage/components/0/{member}"));
    }
    for bad_id in ["0", "1"] {
        let mut value = imported();
        value["report"]["coverage"]["components"][1]["id"] = json!(bad_id);
        assert_rejected(value, "/report/coverage/components/1/id");
    }
    let mut value = imported();
    value["report"]["coverage"]["components"][0]["devices"] = json!([]);
    assert_rejected(value, "/report/coverage/components/0/devices");
}

#[test]
fn equal_device_and_port_names_remain_in_separate_namespaces() {
    let mut doc = lab();
    doc["devices"][4]["id"] = json!("a-eth0");
    doc["scenario"]["probes"][2]["source"] = json!("a-eth0");
    doc["import_source"]["gaps"]
        .as_array_mut()
        .unwrap()
        .push(gap(
            "same-name-port",
            json!({"kind": "port", "port_id": "a-eth0"}),
        ));
    let value = envelope(&run_lab(&doc.to_string()).unwrap());
    assert_eq!(
        value["report"]["probes"][2]["outcome"]["status"],
        "reachable"
    );
    assert_round_trip(value);
}

#[test]
fn gap_targets_use_the_declared_namespace_and_component() {
    for (index, field, wrong_target) in [
        (0, "device_id", "pc-c"),
        (1, "port_id", "c-eth0"),
        (2, "link_id", "c-d"),
    ] {
        let mut value = imported();
        value["report"]["import_source"]["gaps"][index]["scope"][field] = json!(wrong_target);
        assert_rejected(value, "/report/coverage/components/0/gaps");
    }
    let mut value = imported();
    value["report"]["import_source"]["gaps"][1]["scope"]["port_id"] = json!("r2");
    assert_rejected(value, "/report/import_source/gaps/1/scope/port_id");
}

#[test]
fn gap_assignments_cannot_be_missing_repeated_or_undeclared() {
    let mut value = imported();
    value["report"]["coverage"]["components"][0]["gaps"]
        .as_array_mut()
        .unwrap()
        .pop();
    assert_rejected(value, "/report/coverage/components/0/gaps");
    for id in ["device-policy", "undeclared"] {
        let mut value = imported();
        value["report"]["coverage"]["components"][0]["gaps"]
            .as_array_mut()
            .unwrap()
            .push(json!(id));
        assert_rejected(value, "/report/coverage/components/0/gaps");
    }
    let mut value = imported();
    value["report"]["coverage"]["global_gaps"] = json!(["device-policy"]);
    assert_rejected(value, "/report/coverage/global_gaps");
}

#[test]
fn global_scope_cannot_be_disguised_as_a_local_gap() {
    let mut value = imported();
    value["report"]["import_source"]["gaps"][0]["scope"] = json!({"kind": "all"});
    assert_rejected(value, "/report/coverage/components/0/gaps");
}

#[test]
fn global_gaps_cannot_be_omitted_or_leave_an_independent_probe_determined() {
    let mut doc = lab();
    doc["import_source"]["gaps"] = json!([gap("global", json!({"kind": "all"}))]);
    let original = envelope(&run_lab(&doc.to_string()).unwrap());
    assert_round_trip(original.clone());

    let mut value = original.clone();
    value["report"]["coverage"]["global_gaps"] = json!([]);
    assert_rejected(value, "/report/coverage/global_gaps");

    let mut value = original;
    let modeled = value["report"]["probes"][2]["outcome"]["modeled"].clone();
    value["report"]["probes"][2]["outcome"] = modeled;
    assert_rejected(value, "/report/probes/2/outcome");
}

#[test]
fn coverage_references_must_exist_and_bind_both_probe_endpoints() {
    for (field, wrong_id) in [
        ("source_component", "0"),
        ("source_component", "99"),
        ("source_component", "2"),
        ("destination_component", "2"),
    ] {
        let mut value = imported();
        value["report"]["probes"][0]["outcome"]["coverage"][field] = json!(wrong_id);
        assert_rejected(value, &format!("/report/probes/0/outcome/coverage/{field}"));
    }
}

#[test]
fn cross_component_probes_keep_source_and_destination_bindings() {
    let mut doc = lab();
    doc["scenario"]["probes"].as_array_mut().unwrap().extend([
        json!({
            "id": "cross", "source": "pc-a", "destination": "pc-d",
            "at_ns": "0", "deadline_ns": "2000"
        }),
        json!({
            "id": "reverse-cross", "source": "pc-c", "destination": "pc-a",
            "at_ns": "0", "deadline_ns": "2000"
        }),
    ]);
    let value = envelope(&run_lab(&doc.to_string()).unwrap());
    assert_eq!(
        value["report"]["probes"][3]["outcome"]["coverage"],
        json!({"source_component": "1", "destination_component": "2"})
    );
    assert_eq!(
        value["report"]["probes"][4]["outcome"]["coverage"],
        json!({"source_component": "2", "destination_component": "1"})
    );
    assert_round_trip(value);
}

#[test]
fn affected_results_cannot_be_unwrapped_and_unaffected_results_cannot_be_wrapped() {
    let mut value = imported();
    let modeled = value["report"]["probes"][0]["outcome"]["modeled"].clone();
    value["report"]["probes"][0]["outcome"] = modeled;
    assert_rejected(value, "/report/probes/0/outcome");

    let mut value = imported();
    let modeled = value["report"]["probes"][2]["outcome"].clone();
    value["report"]["probes"][2]["outcome"] = json!({
        "status": "undetermined", "modeled": modeled,
        "coverage": {"source_component": "2", "destination_component": "2"}
    });
    assert_rejected(value, "/report/probes/2/outcome");
}

#[test]
fn evidence_model_only_matches_its_own_probe_in_both_directions() {
    for probe_id in ["first", "independent"] {
        let mut value = imported();
        let facts = value["report"]["evidence"].as_array_mut().unwrap();
        let index = facts
            .iter()
            .position(|fact| fact["probe_id"] == probe_id)
            .unwrap();
        facts[index]["model_only"] = json!(probe_id == "independent");
        assert_rejected(value, &format!("/report/evidence/{index}/model_only"));
    }
}

#[test]
fn missing_endpoint_members_cannot_silently_make_a_probe_unaffected() {
    let mut value = imported();
    value["report"]["coverage"]["components"][1]["devices"] = json!(["pc-d"]);
    assert_rejected(value, "/report/probes/2/source/device_id");
}

#[test]
fn zero_gap_reports_cannot_invent_unknown_results_or_gap_references() {
    let mut doc = lab();
    doc["import_source"]["gaps"] = json!([]);
    let original = envelope(&run_lab(&doc.to_string()).unwrap());
    let mut value = original.clone();
    let modeled = value["report"]["probes"][0]["outcome"].clone();
    value["report"]["probes"][0]["outcome"] = json!({
        "status": "undetermined", "modeled": modeled,
        "coverage": {"source_component": "1", "destination_component": "1"}
    });
    assert_rejected(value, "/report/probes/0/outcome");

    let mut value = original;
    value["report"]["coverage"]["global_gaps"] = json!(["undeclared"]);
    assert_rejected(value, "/report/coverage/global_gaps");
}

#[test]
fn gap_checks_also_apply_to_an_early_stop_without_evidence() {
    let report = run_lab_with_limits(
        &lab().to_string(),
        RunLimits {
            pending_events: 1,
            ..Default::default()
        },
    )
    .unwrap();
    let mut value = envelope(&report);
    let modeled = value["report"]["probes"][0]["outcome"]["modeled"].clone();
    value["report"]["probes"][0]["outcome"] = modeled;
    assert_rejected(value, "/report/probes/0/outcome");
}

#[test]
fn local_and_global_gaps_can_share_a_report_without_copying_global_assignments() {
    let mut doc = lab();
    doc["import_source"]["gaps"]
        .as_array_mut()
        .unwrap()
        .push(gap("global", json!({"kind": "all"})));
    let value = envelope(&run_lab(&doc.to_string()).unwrap());
    assert_eq!(
        value["report"]["coverage"]["global_gaps"],
        json!(["global"])
    );
    assert_eq!(
        value["report"]["probes"][2]["outcome"]["status"],
        "undetermined"
    );
    assert_round_trip(value);
}

#[test]
fn global_gap_assignments_cannot_repeat_or_name_undeclared_gaps() {
    let mut doc = lab();
    doc["import_source"]["gaps"] = json!([gap("global", json!({"kind": "all"}))]);
    let original = envelope(&run_lab(&doc.to_string()).unwrap());
    for id in ["global", "undeclared"] {
        let mut value = original.clone();
        value["report"]["coverage"]["global_gaps"]
            .as_array_mut()
            .unwrap()
            .push(json!(id));
        assert_rejected(value, "/report/coverage/global_gaps/1");
    }
}
