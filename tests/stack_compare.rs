use std::net::Ipv4Addr;

use reliner::real_stack::{
    CleanupObservation, EchoOutcome, ImageObservation, KernelLookupObservation, KernelNextHop,
    LINUX_KERNEL_STATIC, ObservationClock, ProbeObservation, RealStackError, StackObservation,
    plan_real_stack,
};
use reliner::stack_compare::{ComparisonStatus, FibDirection, StackComparison, compare_real_stack};
use serde_json::{Value, json};

fn routed() -> Value {
    let mut doc: Value = serde_json::from_str(include_str!("../examples/ipv4-lab.json")).unwrap();
    doc["scenario"]["warmup"] = json!([]);
    doc["scenario"]["end_ns"] = json!("5000");
    doc["scenario"]["probes"][0]["deadline_ns"] = json!("2600");
    doc["scenario"]["probes"][1]["at_ns"] = json!("3000");
    doc["scenario"]["probes"][1]["deadline_ns"] = json!("4600");
    doc
}

fn ip(value: &str) -> Ipv4Addr {
    value.parse().unwrap()
}

fn external_capture() -> Value {
    let prefix = "203.0.113.0/24";
    let record = |argv: Value, stdout: Value| json!({"argv":argv,"exit_code":0,"stdout":stdout});
    let sample = |ns, rib, kernel| {
        json!({
            "monotonic_ns":ns,
            "rib":record(json!(["vtysh","-d","zebra","-c","show ip route static json"]),rib),
            "kernel":record(json!(["ip","-N","-j","-4","route","show","table","main","exact",prefix]),kernel)
        })
    };
    json!({
        "capture_version":1,"native_id":"stack-static-lab","native_revision":"1","native_format_version":2,
        "origin":"synthetic test fixture","network_namespace":"net:[123]","node_id":"r1","prefix":prefix,"ifname":"eth1",
        "version":record(json!(["vtysh","-d","zebra","-c","show version"]),json!("FRRouting 10.7.1_git (test)")),
        "interface":record(json!(["ip","-j","-4","addr","show","dev","eth1"]),json!([{"ifname":"eth1","addr_info":[{"family":"inet","local":"198.51.100.1","prefixlen":30}]}])),
        "installed":sample(110,json!({(prefix):[{"prefix":prefix,"vrfId":0,"vrfName":"default","protocol":"static","selected":true,"destSelected":true,"installed":true,
            "nexthops":[{"ip":"198.51.100.2","interfaceName":"eth1","active":true,"fib":true}]}]}),
            json!([{"dst":prefix,"gateway":"198.51.100.2","dev":"eth1","protocol":"196","flags":[]}])),
        "withdrawn":sample(200,json!({}),json!([]))
    })
}

const STATIC_LAB: &str = include_str!("../examples/stack-static-lab.json");

#[test]
fn external_route_types_only_the_selected_rib_prefix_in_each_phase() {
    use reliner::stack_compare::compare_external_frr_route;

    let prefix = "203.0.113.0/24";
    let other = "198.18.0.0/24";
    let mut capture = external_capture();
    let interface_route = json!([{
        "prefix":other, "vrfId":0, "vrfName":"default", "protocol":"static",
        "nexthops":[{"interfaceName":"eth2","active":true,"fib":true}]
    }]);
    for phase in ["installed", "withdrawn"] {
        capture[phase]["rib"]["stdout"][other] = interface_route.clone();
    }
    let result = compare_external_frr_route(STATIC_LAB, &capture.to_string()).unwrap();
    assert_eq!(result.status, ComparisonStatus::Match);
    assert_eq!(result.installed.kernel.unwrap().protocol, 196);
    assert!(result.withdrawn.rib.is_none());

    for phase in ["installed", "withdrawn"] {
        let mut bad = capture.clone();
        let mut selected = interface_route.clone();
        selected[0]["prefix"] = json!(prefix);
        bad[phase]["rib"]["stdout"][prefix] = selected;
        assert!(compare_external_frr_route(STATIC_LAB, &bad.to_string()).is_err());
    }
    for (field, value) in [
        ("prefix", json!(other)),
        ("vrfId", json!(1)),
        ("vrfName", json!("blue")),
        ("table", json!(253)),
        ("protocol", json!("ospf")),
        ("nexthops", json!([])),
    ] {
        let mut bad = capture.clone();
        bad["installed"]["rib"]["stdout"][prefix][0][field] = value;
        assert!(
            compare_external_frr_route(STATIC_LAB, &bad.to_string()).is_err(),
            "{field}"
        );
    }
}

#[test]
fn external_route_compares_installed_and_withdrawn_rib_and_numeric_kernel_evidence() {
    use reliner::stack_compare::compare_external_frr_route;
    let mut capture = external_capture();
    let run = |c: &Value| compare_external_frr_route(STATIC_LAB, &c.to_string()).unwrap();
    let result = run(&capture);
    assert_eq!(result.scope, "external_frr_route_comparison");
    assert_eq!(result.status, ComparisonStatus::Match);
    assert_eq!(result.installed.kernel.as_ref().unwrap().protocol, 196);
    assert_eq!(result.expected.egress_port, "r1-transit");
    let mut guest = capture.clone();
    guest["version"]["stdout"] = json!("FRRouting 8.4.4 (guest)");
    assert_eq!(run(&guest).frr_version, "FRRouting 8.4.4 (guest)");
    for flag in ["selected", "destSelected", "installed"] {
        let mut pending = capture.clone();
        pending["installed"]["rib"]["stdout"]["203.0.113.0/24"][0][flag] = json!(false);
        let phase = run(&pending).installed;
        assert_eq!(phase.rib_status, ComparisonStatus::Mismatch);
        assert_eq!(phase.kernel_status, ComparisonStatus::Match);
    }
    for field in ["rib", "kernel"] {
        let mut stale = capture.clone();
        stale["withdrawn"][field] = stale["installed"][field].clone();
        assert_eq!(run(&stale).withdrawn.status, ComparisonStatus::Mismatch);
    }
    capture["installed"]["kernel"]["stdout"][0]["protocol"] = json!(4);
    assert_eq!(run(&capture).installed.status, ComparisonStatus::Mismatch);
    capture["installed"]["kernel"]["stdout"][0]["protocol"] = json!(196);
    capture["installed"]["rib"]["stdout"]["203.0.113.0/24"][0]["nexthops"][0]["ip"] =
        json!("198.51.100.1");
    assert_eq!(run(&capture).installed.status, ComparisonStatus::Mismatch);
}

#[test]
fn external_route_rejects_wrong_identity_failed_queries_and_ambiguous_output() {
    for (pointer, value) in [
        ("/installed/kernel/stdout/0/protocol", json!("static")),
        ("/installed/kernel/stdout/0/dst", json!("192.0.2.0/24")),
        ("/withdrawn/kernel/argv/6", json!("local")),
        ("/withdrawn/rib/exit_code", json!(1)),
        (
            "/withdrawn/rib/stdout",
            json!({"error":"daemon unavailable"}),
        ),
        ("/withdrawn/monotonic_ns", json!(100)),
        ("/native_revision", json!("other")),
        ("/node_id", json!("pc-a")),
        (
            "/interface/stdout/0/addr_info/0/local",
            json!("203.0.113.1"),
        ),
    ] {
        let mut capture = external_capture();
        *capture.pointer_mut(pointer).unwrap() = value;
        assert!(
            reliner::stack_compare::compare_external_frr_route(STATIC_LAB, &capture.to_string())
                .is_err(),
            "{pointer}"
        );
    }
}

fn lookup(
    node: &str,
    direction: &str,
    destination: &str,
    port: &str,
    via: Option<&str>,
) -> KernelLookupObservation {
    KernelLookupObservation {
        node_id: node.into(),
        direction: direction.into(),
        destination: ip(destination),
        observed_ms: 10,
        selected: Some(KernelNextHop {
            prefix: format!("{destination}/32"),
            egress_port: port.into(),
            gateway: via.map(ip),
            protocol: None,
        }),
        stdout: String::new(),
        stderr: String::new(),
    }
}

fn observation(doc: &Value) -> StackObservation {
    let plan = plan_real_stack(&doc.to_string()).unwrap();
    let probes = plan
        .probes()
        .iter()
        .map(|p| ProbeObservation {
            id: p.id.clone(),
            source: ip("192.0.2.10"),
            destination: ip("203.0.113.20"),
            payload_bytes: 56,
            ipv4_dont_fragment: true,
            engine: "python3_linux_raw_icmp_df".into(),
            outcome: EchoOutcome::Reply,
            reply_ttl: Some(62),
            reply_ipv4_dont_fragment: Some(false),
            container_elapsed_ms: 917.0,
            started_ms: 100,
            finished_ms: 1017,
            reachable: true,
            transmitted: Some(1),
            received: Some(1),
            route_observations: vec![],
            kernel_lookups: vec![
                lookup(
                    "pc-a",
                    "request",
                    "203.0.113.20",
                    "a-eth0",
                    Some("192.0.2.1"),
                ),
                lookup(
                    "r1",
                    "request",
                    "203.0.113.20",
                    "r1-transit",
                    Some("198.51.100.2"),
                ),
                lookup("r2", "request", "203.0.113.20", "r2-lan", None),
                lookup("pc-b", "reply", "192.0.2.10", "b-eth0", Some("203.0.113.1")),
                lookup("r1", "reply", "192.0.2.10", "r1-lan", None),
                lookup(
                    "r2",
                    "reply",
                    "192.0.2.10",
                    "r2-transit",
                    Some("198.51.100.1"),
                ),
            ],
            stdout: String::new(),
            stderr: String::new(),
        })
        .collect();
    StackObservation {
        native_id: plan.native_id().into(),
        native_revision: plan.native_revision().into(),
        native_format_version: plan.native_format_version(),
        image: ImageObservation {
            reference: plan.image().into(),
            id: format!("sha256:{}", "1".repeat(64)),
            frr_version: "zebra version 10.7.1".into(),
            ..Default::default()
        },
        effective_plan: plan,
        route_configuration_source: LINUX_KERNEL_STATIC,
        owner: "fixture".into(),
        clock: ObservationClock {
            kind: "fixture".into(),
            unix_start_ms: 0,
            elapsed_ms: 1000,
        },
        comparison_scope: String::new(),
        interfaces: vec![],
        routes: vec![],
        probes,
        commands: vec![],
        cleanup: CleanupObservation {
            complete: true,
            errors: vec![],
        },
    }
}

fn compare(doc: &Value, observed: &StackObservation) -> StackComparison {
    compare_real_stack(&doc.to_string(), observed).unwrap()
}

fn invalid(doc: &Value, observed: &StackObservation, expected: &str) {
    match compare_real_stack(&doc.to_string(), observed).unwrap_err() {
        RealStackError::InvalidObservation(message) => {
            assert!(message.contains(expected), "{message}")
        }
        other => panic!("expected invalid observation, got {other:?}"),
    }
}

fn no_reply(probe: &mut ProbeObservation, outcome: EchoOutcome) {
    probe.outcome = outcome;
    probe.reachable = false;
    probe.transmitted = Some(u32::from(outcome != EchoOutcome::NoRoute));
    probe.received = Some(0);
    probe.reply_ttl = None;
    probe.reply_ipv4_dont_fragment = None;
}

#[test]
fn compares_echo_and_both_fib_directions_without_timing_or_observation_order() {
    let doc = routed();
    let mut observed = observation(&doc);
    observed.probes.reverse();
    observed.probes[0].kernel_lookups.reverse();
    let result = compare(&doc, &observed);
    assert_eq!(result.native_id, "ipv4-lab");
    assert_eq!(result.native_revision, "1");
    assert_eq!(result.image_id, observed.image.id);
    assert_eq!(result.route_configuration_source, LINUX_KERNEL_STATIC);
    assert_eq!(result.probes[0].probe_id, "first");
    assert!(result.scope.contains("not a packet path"));
    for row in result.probes {
        assert_eq!(row.reachability.status, ComparisonStatus::Match);
        assert_eq!(row.fib.len(), 6);
        assert!(row.fib.iter().all(|r| r.status == ComparisonStatus::Match));
        let back = row
            .fib
            .iter()
            .find(|r| r.node_id == "r2" && r.direction == FibDirection::Reply)
            .unwrap();
        assert_eq!(
            back.expected.as_ref().unwrap().gateway,
            Some(ip("198.51.100.1"))
        );
    }
}

#[test]
fn same_native_identity_cannot_match_a_different_execution_plan() {
    let doc = routed();
    let observed = observation(&doc);
    for pointer in [
        "/devices/1/routes",
        "/scenario/probes/0/ttl",
        "/scenario/probes/0/payload_bytes",
    ] {
        let mut changed = doc.clone();
        let value = match pointer {
            "/devices/1/routes" => json!([]),
            "/scenario/probes/0/ttl" => json!(63),
            _ => json!(128),
        };
        let (parent, key) = pointer.rsplit_once('/').unwrap();
        changed.pointer_mut(parent).unwrap()[key] = value;
        invalid(&changed, &observed, "effective_plan");
    }
}

#[test]
fn mismatched_identity_image_route_source_and_cleanup_are_invalid() {
    let doc = routed();
    let mut observed = observation(&doc);
    observed.native_format_version = 3;
    invalid(&doc, &observed, "native_format_version");
    observed.native_format_version = 2;
    observed.image.reference = "different:image".into();
    invalid(&doc, &observed, "image reference");
    observed.image.reference = observed.effective_plan.image().into();
    observed.image.frr_version = "zebra version 10.7.10".into();
    invalid(&doc, &observed, "FRR version");
    observed.image.frr_version = "zebra version 10.7.1".into();
    let image_id = std::mem::replace(&mut observed.image.id, "sha256:bad".into());
    invalid(&doc, &observed, "image ID");
    observed.image.id = image_id;
    observed.route_configuration_source = "another_route_source";
    invalid(&doc, &observed, "route_configuration_source");
    observed.route_configuration_source = LINUX_KERNEL_STATIC;
    observed.cleanup.complete = false;
    invalid(&doc, &observed, "cleanup");
}

#[test]
fn missing_probe_and_lookup_are_not_mismatches_or_observed_no_route() {
    let doc = routed();
    let mut observed = observation(&doc);
    observed.probes.pop();
    observed.probes[0].kernel_lookups.remove(4);
    let result = compare(&doc, &observed);
    assert_eq!(
        result.probes[0].reachability.status,
        ComparisonStatus::Match
    );
    let row = result.probes[0]
        .fib
        .iter()
        .find(|r| r.node_id == "r1" && r.direction == FibDirection::Reply)
        .unwrap();
    assert_eq!(row.status, ComparisonStatus::NotComparable);
    assert_eq!(row.reason, "missing_kernel_lookup");
    assert_eq!(
        result.probes[1].reachability.reason,
        "missing_probe_observation"
    );
    assert!(
        result.probes[1]
            .fib
            .iter()
            .all(|r| r.status == ComparisonStatus::NotComparable)
    );
}

#[test]
fn unknown_duplicate_and_wrongly_identified_probe_or_lookup_are_errors() {
    let doc = routed();
    let mut observed = observation(&doc);
    observed.probes[1].id = "unknown".into();
    invalid(&doc, &observed, "unknown probe");
    observed.probes[1].id = "first".into();
    invalid(&doc, &observed, "duplicate probe");
    observed.probes[1].id = "second".into();
    observed.probes[1].payload_bytes = 88;
    invalid(&doc, &observed, "payload_bytes");
    observed.probes[1].payload_bytes = 56;
    observed.probes[1].source = ip("192.0.2.99");
    invalid(&doc, &observed, "source");
    observed.probes[1].source = ip("192.0.2.10");
    observed.probes[1]
        .kernel_lookups
        .push(lookup("r1", "reply", "192.0.2.10", "r1-lan", None));
    invalid(&doc, &observed, "duplicate kernel lookup");
    observed.probes[1].kernel_lookups.pop();
    observed.probes[1].kernel_lookups[0].direction = "unknown".into();
    invalid(&doc, &observed, "unknown kernel lookup direction");
    observed.probes[1].kernel_lookups[0].direction = "request".into();
    observed.probes[1].kernel_lookups[0].node_id = "unknown".into();
    invalid(&doc, &observed, "unexpected kernel lookup node");
}

#[test]
fn model_deadline_and_wall_timeout_do_not_establish_unreachability() {
    let mut doc = routed();
    let mut observed = observation(&doc);
    doc["scenario"]["probes"][0]["deadline_ns"] = json!("1001");
    no_reply(&mut observed.probes[1], EchoOutcome::Timeout);
    let result = compare(&doc, &observed);
    assert_eq!(
        result.probes[0].reachability.status,
        ComparisonStatus::NotComparable
    );
    assert_eq!(
        result.probes[0].reachability.reason,
        "model_timeout_without_structural_evidence"
    );
    assert_eq!(
        result.probes[1].reachability.status,
        ComparisonStatus::NotComparable
    );
    assert_eq!(
        result.probes[1].reachability.reason,
        "echo_timeout_without_structural_evidence"
    );
    assert!(
        result.probes[0]
            .fib
            .iter()
            .all(|r| r.status == ComparisonStatus::Match)
    );
}

#[test]
fn structural_return_route_miss_requires_a_matching_observed_fib_miss() {
    let mut doc = routed();
    doc["devices"][2]["routes"] = json!([]);
    let mut observed = observation(&doc);
    no_reply(&mut observed.probes[0], EchoOutcome::Timeout);
    observed.probes[0].kernel_lookups[5].selected = None;
    let result = compare(&doc, &observed);
    let row = &result.probes[0].reachability;
    assert_eq!(row.status, ComparisonStatus::Match);
    assert_eq!(row.reason, "structural_route_miss_corroborated_by_fib");
    let block = row.route_miss.as_ref().unwrap();
    assert_eq!(block.node_id, "r2");
    assert_eq!(block.direction, FibDirection::Reply);
    assert_eq!(block.destination, ip("192.0.2.10"));
    assert_eq!(
        result.probes[1].reachability.status,
        ComparisonStatus::Mismatch
    );
    observed.probes[0].kernel_lookups.pop();
    let result = compare(&doc, &observed);
    assert_eq!(
        result.probes[0].reachability.status,
        ComparisonStatus::NotComparable
    );
    assert_eq!(
        result.probes[0].reachability.reason,
        "structural_route_miss_without_matching_fib"
    );
}

#[test]
fn a_confirmed_local_no_route_contradicts_model_reachability() {
    let doc = routed();
    let mut observed = observation(&doc);
    no_reply(&mut observed.probes[0], EchoOutcome::NoRoute);
    observed.probes[0].kernel_lookups[0].selected = None;
    let result = compare(&doc, &observed);
    assert_eq!(
        result.probes[0].reachability.status,
        ComparisonStatus::Mismatch
    );
    assert_eq!(
        result.probes[0].reachability.reason,
        "model_reachable_but_local_no_route"
    );
}

#[test]
fn matching_local_route_miss_requires_an_observed_failed_send() {
    let mut doc = routed();
    doc["devices"][0].as_object_mut().unwrap().remove("gateway");
    let mut observed = observation(&doc);
    let first = &mut observed.probes[0];
    no_reply(first, EchoOutcome::NoRoute);
    first.kernel_lookups[0].selected = None;
    let result = compare(&doc, &observed);
    let row = &result.probes[0].reachability;
    assert_eq!(row.status, ComparisonStatus::Match);
    let miss = row.route_miss.as_ref().unwrap();
    assert_eq!(miss.node_id, "pc-a");
    assert_eq!(miss.direction, FibDirection::Request);

    no_reply(&mut observed.probes[0], EchoOutcome::Timeout);
    let row = &compare(&doc, &observed).probes[0].reachability;
    assert_eq!(row.status, ComparisonStatus::NotComparable);
    assert_eq!(
        row.reason,
        "echo_failure_does_not_identify_the_modeled_route_miss"
    );
}

#[test]
fn longest_prefix_wins_and_fib_difference_does_not_change_echo_status() {
    let mut doc = routed();
    doc["devices"][1]["routes"].as_array_mut().unwrap().insert(
        0,
        json!({
            "network":"0.0.0.0", "prefix_length":0, "egress":"r1-lan", "next_hop":"192.0.2.10"
        }),
    );
    let mut observed = observation(&doc);
    let wrong_route = observed.probes[0].kernel_lookups[1]
        .selected
        .as_mut()
        .unwrap();
    wrong_route.egress_port = "r1-lan".into();
    wrong_route.gateway = Some(ip("192.0.2.10"));
    let result = compare(&doc, &observed);
    assert_eq!(
        result.probes[0].reachability.status,
        ComparisonStatus::Match
    );
    let row = result.probes[0]
        .fib
        .iter()
        .find(|r| r.node_id == "r1" && r.direction == FibDirection::Request)
        .unwrap();
    assert_eq!(row.status, ComparisonStatus::Mismatch);
    assert_eq!(row.expected.as_ref().unwrap().egress_port, "r1-transit");
    assert_eq!(
        row.expected.as_ref().unwrap().gateway,
        Some(ip("198.51.100.2"))
    );
    assert_eq!(
        row.observed.as_ref().unwrap().gateway,
        Some(ip("192.0.2.10"))
    );
}
