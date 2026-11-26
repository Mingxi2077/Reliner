use std::time::Duration;

use reliner::real_stack::{
    LINUX_KERNEL_STATIC, MAX_REAL_STACK_NODES, MAX_REAL_STACK_PROBES, REAL_STACK_IMAGE,
    RealStackError, RealStackOptions, plan_real_stack, run_real_stack,
};
use serde_json::{Value, json};

fn direct() -> Value {
    json!({
        "format_version": 2, "id": "direct", "revision": "native-r17",
        "devices": [
            {"kind":"host", "id":"a", "name":"A", "ipv4":"192.0.2.10",
             "prefix_length":24, "mac":"02:00:00:00:00:01", "port":{"id":"ap","name":"Ethernet"}},
            {"kind":"host", "id":"b", "name":"B", "ipv4":"192.0.2.20",
             "prefix_length":24, "mac":"02:00:00:00:00:02", "port":{"id":"bp","name":"Ethernet"}}
        ],
        "links":[{"id":"wire", "ports":["ap","bp"], "delay_ns":"100"}],
        "scenario":{"end_ns":"10000", "arp_lifetime_ns":"1000000000",
            "mac_lifetime_ns":"1000000000", "probes":[
                {"id":"echo", "source":"a", "destination":"b", "at_ns":"0", "deadline_ns":"9000"}
            ]}
    })
}

#[test]
fn planning_compiles_native_without_docker_or_simulated_wall_time() {
    let plan = plan_real_stack(&direct().to_string()).unwrap();
    assert_eq!(plan.native_revision(), "native-r17");
    assert_eq!(plan.native_format_version(), 2);
    assert_eq!(plan.nodes().len(), 2);
    assert_eq!(plan.links().len(), 1);
    assert_eq!(plan.probes().len(), 1);
    assert_eq!(plan.image(), REAL_STACK_IMAGE);
    assert_eq!(plan.interfaces()[0].prefix_length, 24);
    assert_eq!(plan.interfaces()[0].mac, "02:00:00:00:00:01");
    assert_eq!(plan.interfaces()[0].ipv4_mtu_bytes, 1500);
    assert_eq!(plan.probes()[0].payload_bytes, 56);
    assert!(plan.probes()[0].ipv4_dont_fragment);
    let serialized = serde_json::to_value(&plan).unwrap();
    assert!(serialized.get("end_ns").is_none());
    assert!(serialized["probes"][0].get("deadline_ns").is_none());
}

#[test]
fn native3_without_import_gaps_retains_version_and_effective_probe_parameters() {
    let mut doc = direct();
    doc["format_version"] = json!(3);
    doc["import_source"] = json!({"format":"containerlab","source_name":"original.yaml","gaps":[]});
    reliner::validate_lab(&doc.to_string()).unwrap();
    let plan = plan_real_stack(&doc.to_string()).unwrap();
    assert_eq!(plan.native_format_version(), 3);
    assert_eq!(plan.native_revision(), "native-r17");
    assert_eq!(plan.probes()[0].payload_bytes, 56);
    assert!(plan.probes()[0].ipv4_dont_fragment);
    let serialized = serde_json::to_value(&plan).unwrap();
    assert_eq!(serialized["native_format_version"], 3);
    assert_eq!(serialized["probes"][0]["payload_bytes"], 56);
    assert_eq!(serialized["probes"][0]["ipv4_dont_fragment"], true);
    doc["scenario"]["probes"][0]["payload_bytes"] = json!(128);
    assert_eq!(
        plan_real_stack(&doc.to_string()).unwrap().probes()[0].payload_bytes,
        128
    );
}

#[test]
fn static_router_planning_uses_compiled_routes_and_rejects_warmup() {
    let mut doc: Value = serde_json::from_str(include_str!("../examples/ipv4-lab.json")).unwrap();
    assert!(matches!(
        plan_real_stack(&doc.to_string()),
        Err(RealStackError::Unsupported { .. })
    ));
    doc["scenario"]["warmup"] = json!([]);
    let plan = plan_real_stack(&doc.to_string()).unwrap();
    assert_eq!(plan.nodes().len(), 4);
    assert_eq!(plan.routes().len(), 2);
    assert_eq!(plan.routes()[0].next_hop.to_string(), "198.51.100.2");
}

#[test]
fn unsupported_features_are_never_silently_dropped() {
    for (field, value) in [
        (
            "link_actions",
            json!([{"link":"wire","state":"down","at_ns":"100"}]),
        ),
        (
            "warmup",
            json!([{"port":"ap","neighbor":"192.0.2.20","at_ns":"0"}]),
        ),
        ("routing_convergence_ns", json!("0")),
        ("stp_convergence_ns", json!("0")),
    ] {
        let mut doc = direct();
        doc["scenario"][field] = value;
        assert!(
            matches!(
                plan_real_stack(&doc.to_string()),
                Err(RealStackError::Unsupported { .. })
            ),
            "{field}"
        );
    }
    let mut doc = direct();
    doc["links"][0]["transmission"] = json!({"rate_bps":"1000000","queue_frames":2});
    assert!(matches!(
        plan_real_stack(&doc.to_string()),
        Err(RealStackError::Unsupported { .. })
    ));
    let mut doc = direct();
    doc["devices"][0]["port"]["ipv4_mtu_bytes"] = json!(1200);
    assert!(matches!(
        plan_real_stack(&doc.to_string()),
        Err(RealStackError::Unsupported { .. })
    ));
    let mut doc = direct();
    doc["devices"][0]["exec"] = json!("do not run imported commands");
    assert!(matches!(
        plan_real_stack(&doc.to_string()),
        Err(RealStackError::Unsupported { .. })
    ));
}

#[test]
fn dynamic_router_configuration_is_explicitly_unsupported_even_when_incomplete() {
    let mut doc: Value = serde_json::from_str(include_str!("../examples/ipv4-lab.json")).unwrap();
    doc["devices"][1]["link_state"] = json!({"peers":[],"advertise":[]});
    assert!(matches!(
        plan_real_stack(&doc.to_string()),
        Err(RealStackError::Unsupported { .. })
    ));
    let mut doc = direct();
    doc["devices"].as_array_mut().unwrap().push(
        json!({"kind":"switch","id":"s","name":"S","ports":[
            {"id":"s1","name":"p1","mode":{"kind":"access","vlan":1}}
        ]}),
    );
    assert!(matches!(
        plan_real_stack(&doc.to_string()),
        Err(RealStackError::Unsupported { .. })
    ));
}

#[test]
fn import_gaps_and_unknown_versions_are_unsupported() {
    for input in [
        r#"{"format_version":99}"#,
        r#"{"format_version":2,"id":"incomplete","devices":[]}"#,
        r#"{"nodes":[],"links":[],"exec":"must not execute"}"#,
    ] {
        assert!(matches!(
            plan_real_stack(input),
            Err(RealStackError::Unsupported { .. })
        ));
    }
    assert!(matches!(
        plan_real_stack("{"),
        Err(RealStackError::InvalidInput(_))
    ));
}

#[test]
fn declared_import_gaps_prevent_real_stack_comparisons() {
    let mut doc = direct();
    doc["format_version"] = json!(3);
    doc["import_source"] = json!({"format":"containerlab", "source_name":"lab.yaml", "gaps":[
        {"id":"exec", "source_path":"/topology/nodes/a/exec", "scope":{"kind":"all"},
         "value":["do not execute"], "message":"Imported exec has no native semantics"}
    ]});
    let input = doc.to_string();
    reliner::validate_lab(&input).unwrap();
    assert!(matches!(
        plan_real_stack(&input),
        Err(RealStackError::Unsupported { feature, .. }) if feature == "import_gaps"
    ));
}

#[test]
fn probe_and_node_limits_apply_before_execution() {
    let mut doc = direct();
    doc["scenario"]["probes"] = Value::Array(
        (0..=MAX_REAL_STACK_PROBES)
            .map(|i| {
                let mut p = direct()["scenario"]["probes"][0].clone();
                p["id"] = json!(format!("p{i}"));
                p
            })
            .collect(),
    );
    assert!(matches!(
        plan_real_stack(&doc.to_string()),
        Err(RealStackError::LimitExceeded { .. })
    ));
    let mut doc = direct();
    let devices = doc["devices"].as_array_mut().unwrap();
    for i in 2..=MAX_REAL_STACK_NODES {
        let mut node = devices[0].clone();
        node["id"] = json!(format!("node{i}"));
        node["port"]["id"] = json!(format!("port{i}"));
        node["ipv4"] = json!(format!("192.0.2.{}", i + 20));
        node["mac"] = json!(format!("02:00:00:00:01:{i:02x}"));
        devices.push(node);
    }
    assert!(matches!(
        plan_real_stack(&doc.to_string()),
        Err(RealStackError::LimitExceeded { .. })
    ));
}

#[test]
fn router_acl_is_outside_the_real_stack_model() {
    let mut doc: Value =
        serde_json::from_str(include_str!("../examples/stack-static-lab.json")).unwrap();
    doc["devices"][1]["ports"][0]["ipv4_acl"] = json!({"ingress":[]});
    assert!(
        matches!(plan_real_stack(&doc.to_string()), Err(RealStackError::Unsupported {feature,..}) if feature == "ipv4_acl")
    );
}

#[test]
fn invalid_runtime_budget_is_rejected_without_docker() {
    let plan = plan_real_stack(&direct().to_string()).unwrap();
    let options = RealStackOptions {
        run_timeout: Duration::ZERO,
        ..Default::default()
    };
    assert!(matches!(
        run_real_stack(&plan, options),
        Err(RealStackError::InvalidOptions(_))
    ));
}

#[test]
#[ignore = "Docker Linux kernel IPv4 direct link; FRR image is a tools environment only"]
fn docker_direct_ipv4_records_real_observations_and_cleans_resources() {
    let plan = plan_real_stack(&direct().to_string()).unwrap();
    let observed = run_real_stack(&plan, RealStackOptions::default()).unwrap();
    assert_eq!(observed.native_revision, "native-r17");
    assert_eq!(
        serde_json::to_value(&observed.effective_plan).unwrap(),
        serde_json::to_value(&plan).unwrap()
    );
    assert_eq!(observed.native_format_version, plan.native_format_version());
    assert_eq!(observed.image.reference, REAL_STACK_IMAGE);
    assert_eq!(observed.route_configuration_source, LINUX_KERNEL_STATIC);
    assert!(observed.image.frr_version.contains("10.7.1"));
    assert!(observed.probes[0].reachable);
    assert_eq!(observed.probes[0].engine, "python3_linux_raw_icmp_df");
    assert_eq!(observed.probes[0].payload_bytes, 56);
    assert!(observed.probes[0].ipv4_dont_fragment);
    assert_eq!(observed.probes[0].transmitted, Some(1));
    assert_eq!(observed.probes[0].received, Some(1));
    assert!(observed.cleanup.complete);
    assert!(!observed.commands.is_empty());
    assert_eq!(observed.probes[0].kernel_lookups.len(), 2);
    assert!(
        observed.probes[0]
            .kernel_lookups
            .iter()
            .all(|l| l.selected.as_ref().is_some_and(|r| r.gateway.is_none()))
    );
    assert!(
        observed
            .commands
            .iter()
            .all(|c| c.finished_ms >= c.started_ms)
    );
}

#[test]
#[ignore = "Docker Linux kernel IPv4 static routes and bidirectional FIB; no FRR daemons"]
fn docker_linux_static_routes_forward_in_both_directions() {
    let mut doc: Value = serde_json::from_str(include_str!("../examples/ipv4-lab.json")).unwrap();
    doc["scenario"]["warmup"] = json!([]);
    doc["scenario"]["probes"][1]["source"] = json!("pc-b");
    doc["scenario"]["probes"][1]["destination"] = json!("pc-a");
    let plan = plan_real_stack(&doc.to_string()).unwrap();
    let observation = run_real_stack(&plan, RealStackOptions::default()).unwrap();
    assert_eq!(observation.route_configuration_source, LINUX_KERNEL_STATIC);
    assert!(observation.cleanup.complete);
    for (probe, node, port, gateway) in [
        (&observation.probes[0], "r1", "r1-transit", "198.51.100.2"),
        (&observation.probes[1], "r2", "r2-transit", "198.51.100.1"),
    ] {
        assert!(probe.reachable, "{probe:?}");
        let lookup = probe
            .kernel_lookups
            .iter()
            .find(|entry| entry.node_id == node && entry.direction == "request")
            .unwrap();
        let selected = lookup.selected.as_ref().unwrap();
        assert_eq!(selected.egress_port, port);
        assert_eq!(selected.gateway, Some(gateway.parse().unwrap()));
        assert!(
            probe
                .route_observations
                .iter()
                .map(|&i| &observation.routes[i])
                .any(|snapshot| {
                    snapshot.node_id == node
                        && snapshot.routes.iter().any(|route| {
                            route.gateway == selected.gateway
                                && route.protocol.as_deref() == Some("static")
                        })
                })
        );
    }
}
