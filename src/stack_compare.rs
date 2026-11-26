use std::collections::{BTreeMap, BTreeSet};
use std::net::Ipv4Addr;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::real_stack::{
    EchoOutcome, KernelLookupObservation, LINUX_KERNEL_STATIC, ProbeObservation, RealStackError,
    RealStackPlan, StackObservation, StackProbe, plan_real_stack,
};
use crate::topology::{DeviceKind, Topology};
use crate::{Evidence, EvidenceFact, EvidenceId, FrameKind, Outcome, RunReport, run_lab};

#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct StackComparison {
    pub native_id: String,
    pub native_revision: String,
    pub native_format_version: u32,
    pub model_version: u32,
    pub observation_owner: String,
    pub image_reference: String,
    pub image_id: String,
    pub route_configuration_source: &'static str,
    pub scope: &'static str,
    pub probes: Vec<StackProbeComparison>,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct StackProbeComparison {
    pub probe_id: String,
    pub source_node: String,
    pub destination_node: String,
    pub source: Ipv4Addr,
    pub destination: Ipv4Addr,
    pub ttl: u8,
    pub payload_bytes: u16,
    pub ipv4_dont_fragment: bool,
    pub reachability: ReachabilityComparison,
    pub fib: Vec<FibComparison>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ComparisonStatus {
    Match,
    Mismatch,
    NotComparable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelEchoOutcome {
    Reachable,
    Timeout,
    Incomplete,
    Undetermined,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct ReachabilityComparison {
    pub status: ComparisonStatus,
    pub reason: &'static str,
    pub modeled: ModelEchoOutcome,
    pub observed: Option<EchoOutcome>,
    pub route_miss: Option<StructuralRouteMiss>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FibDirection {
    Request,
    Reply,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct StructuralRouteMiss {
    pub evidence_id: EvidenceId,
    pub node_id: String,
    pub direction: FibDirection,
    pub destination: Ipv4Addr,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct FibSelection {
    pub egress_port: String,
    /// None denotes an on-link destination, not a failed lookup.
    pub gateway: Option<Ipv4Addr>,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct FibComparison {
    pub node_id: String,
    pub direction: FibDirection,
    pub destination: Ipv4Addr,
    pub status: ComparisonStatus,
    pub reason: &'static str,
    pub expected: Option<FibSelection>,
    pub observed: Option<FibSelection>,
}

fn invalid(message: impl Into<String>) -> RealStackError {
    RealStackError::InvalidObservation(message.into())
}

/// Compares two externally collected route-table snapshots. This result has no
/// local execution, packet observations, or authenticated capture provenance.
#[derive(Debug, Serialize)]
pub struct ExternalFrrRouteComparison {
    pub scope: &'static str,
    pub native_id: String,
    pub native_revision: String,
    pub native_format_version: u32,
    pub origin: String,
    pub network_namespace: String,
    pub frr_version: String,
    pub node_id: String,
    pub prefix: String,
    pub ifname: String,
    pub expected: FibSelection,
    pub expected_kernel_protocol: u8,
    pub installed: ExternalFrrPhase,
    pub withdrawn: ExternalFrrPhase,
    pub status: ComparisonStatus,
}

#[derive(Debug, Serialize)]
pub struct ExternalFrrPhase {
    pub monotonic_ns: u64,
    pub rib: Option<FrrRibRow>,
    pub kernel: Option<FrrKernelRow>,
    pub rib_status: ComparisonStatus,
    pub kernel_status: ComparisonStatus,
    pub status: ComparisonStatus,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct FrrRibRow {
    pub prefix: String,
    pub protocol: String,
    #[serde(rename = "vrfId")]
    pub vrf_id: u32,
    #[serde(rename = "vrfName")]
    pub vrf_name: String,
    pub table: Option<u32>,
    #[serde(default)]
    pub selected: bool,
    #[serde(default, rename = "destSelected")]
    pub dest_selected: bool,
    #[serde(default)]
    pub installed: bool,
    pub nexthops: Vec<FrrNexthop>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct FrrNexthop {
    pub ip: Ipv4Addr,
    #[serde(rename = "interfaceName")]
    pub ifname: String,
    #[serde(default)]
    pub active: bool,
    #[serde(default)]
    pub fib: bool,
}

#[derive(Debug, Serialize)]
pub struct FrrKernelRow {
    pub gateway: Ipv4Addr,
    pub dev: String,
    pub protocol: u8,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FrrCapture {
    capture_version: u32,
    native_id: String,
    native_revision: String,
    native_format_version: u32,
    origin: String,
    network_namespace: String,
    node_id: String,
    prefix: String,
    ifname: String,
    version: CaptureCommand,
    interface: CaptureCommand,
    installed: FrrSample,
    withdrawn: FrrSample,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CaptureCommand {
    argv: Vec<String>,
    exit_code: i32,
    stdout: Value,
}

impl CaptureCommand {
    fn output(&self, expected: &[&str]) -> Result<&Value, RealStackError> {
        if self.argv != expected || self.exit_code != 0 {
            return Err(invalid(format!(
                "expected successful {expected:?}, observed {:?} (exit {})",
                self.argv, self.exit_code
            )));
        }
        Ok(&self.stdout)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FrrSample {
    monotonic_ns: u64,
    rib: CaptureCommand,
    kernel: CaptureCommand,
}

// FRR 10.7.1 zebra/rt_netlink.c maps ZEBRA_ROUTE_STATIC to RTPROT_ZSTATIC.
const RTPROT_ZSTATIC: u8 = 196;

fn match_status(matches: bool) -> ComparisonStatus {
    if matches {
        ComparisonStatus::Match
    } else {
        ComparisonStatus::Mismatch
    }
}

fn external_phase(
    sample: &FrrSample,
    prefix: &str,
    dev: &str,
    via: Ipv4Addr,
    installed: bool,
) -> Result<ExternalFrrPhase, RealStackError> {
    let output = sample
        .rib
        .output(&["vtysh", "-d", "zebra", "-c", "show ip route static json"])?;
    let table = output
        .as_object()
        .ok_or_else(|| invalid("expected a Zebra static route table"))?;
    // A daemon diagnostic object cannot establish that the route was withdrawn.
    if !table.keys().all(|key| {
        key.split_once('/').is_some_and(|(ip, len)| {
            ip.parse::<Ipv4Addr>().is_ok() && len.parse::<u8>().is_ok_and(|n| n <= 32)
        })
    }) {
        return Err(invalid(
            "expected a Zebra static route table keyed by IPv4 prefix",
        ));
    }
    let rib = if let Some(rows) = table.get(prefix) {
        let [row] = <[FrrRibRow; 1]>::deserialize(rows).map_err(|e| {
            invalid(format!(
                "invalid Zebra static route entry for {prefix}: {e}"
            ))
        })?;
        if row.prefix != prefix
            || row.vrf_id != 0
            || row.vrf_name != "default"
            || row.table.is_some_and(|table| table != 254)
            || row.protocol != "static"
        {
            return Err(invalid(format!(
                "unexpected Zebra static route entry for {prefix}"
            )));
        }
        if row.nexthops.len() != 1 {
            return Err(invalid("expected a single-path Zebra static route"));
        }
        Some(row)
    } else {
        None
    };
    let output = sample.kernel.output(&[
        "ip", "-N", "-j", "-4", "route", "show", "table", "main", "exact", prefix,
    ])?;
    let entries = output
        .as_array()
        .ok_or_else(|| invalid("kernel route output must be an array"))?;
    let kernel = match entries.as_slice() {
        [] => None,
        [row] => {
            let dst = row["dst"]
                .as_str()
                .ok_or_else(|| invalid("missing kernel prefix"))?;
            let normalized = if dst == "default" {
                "0.0.0.0/0".to_owned()
            } else if !dst.contains('/') {
                format!("{dst}/32")
            } else {
                dst.to_owned()
            };
            if normalized != prefix
                || row.get("nexthops").is_some()
                || row
                    .get("type")
                    .is_some_and(|v| v != "unicast" && v != "1" && v.as_u64() != Some(1))
                || row
                    .get("table")
                    .is_some_and(|v| v != "main" && v != "254" && v.as_u64() != Some(254))
            {
                return Err(invalid(
                    "kernel output differs from the requested single-path main-table prefix",
                ));
            }
            let p = &row["protocol"];
            let protocol = p
                .as_u64()
                .and_then(|n| u8::try_from(n).ok())
                .or_else(|| p.as_str()?.parse::<u8>().ok())
                .ok_or_else(|| invalid("expected numeric kernel route protocol"))?;
            Some(FrrKernelRow {
                protocol,
                gateway: row["gateway"]
                    .as_str()
                    .and_then(|s| s.parse().ok())
                    .ok_or_else(|| invalid("missing IPv4 kernel gateway"))?,
                dev: row["dev"]
                    .as_str()
                    .ok_or_else(|| invalid("missing kernel egress"))?
                    .into(),
            })
        }
        _ => return Err(invalid("ambiguous kernel rows for exact prefix")),
    };
    let rib_matches = if installed {
        rib.as_ref().is_some_and(|r| {
            r.selected && r.dest_selected && r.installed && {
                let hop = &r.nexthops[0];
                hop.active && hop.fib && hop.ifname == dev && hop.ip == via
            }
        })
    } else {
        rib.is_none()
    };
    let kernel_matches = if installed {
        kernel
            .as_ref()
            .is_some_and(|r| r.dev == dev && r.gateway == via && r.protocol == RTPROT_ZSTATIC)
    } else {
        kernel.is_none()
    };
    Ok(ExternalFrrPhase {
        monotonic_ns: sample.monotonic_ns,
        rib,
        kernel,
        rib_status: match_status(rib_matches),
        kernel_status: match_status(kernel_matches),
        status: match_status(rib_matches && kernel_matches),
    })
}

/// Accepts the version-1 capture produced by examples/capture-frr-route.py in
/// a separately operated, trusted router namespace. No commands are executed.
pub fn compare_external_frr_route(
    native: &str,
    capture: &str,
) -> Result<ExternalFrrRouteComparison, RealStackError> {
    let plan = plan_real_stack(native)?;
    if capture.len() > 1_048_576 {
        return Err(invalid("external route capture exceeds 1 MiB"));
    }
    let c: FrrCapture = serde_json::from_str(capture.trim_start_matches('\u{feff}'))
        .map_err(|e| invalid(format!("invalid external FRR capture: {e}")))?;
    if c.capture_version != 1
        || c.native_format_version != plan.native_format_version()
        || c.native_id != plan.native_id()
        || c.native_revision != plan.native_revision()
    {
        return Err(invalid(
            "capture version or native identity differs from plan",
        ));
    }
    if c.origin.trim().is_empty()
        || c.network_namespace.trim().is_empty()
        || c.installed.monotonic_ns >= c.withdrawn.monotonic_ns
    {
        return Err(invalid(
            "missing capture provenance or unordered route samples",
        ));
    }
    let route = plan
        .routes()
        .iter()
        .find(|r| {
            plan.nodes()[r.node].id == c.node_id
                && format!("{}/{}", r.network, r.prefix_length) == c.prefix
        })
        .ok_or_else(|| invalid("capture node and static prefix do not identify a planned route"))?;
    let port = &plan.interfaces()[route.egress];
    let version = c
        .version
        .output(&["vtysh", "-d", "zebra", "-c", "show version"])?
        .as_str()
        .and_then(|s| s.lines().next())
        .ok_or_else(|| invalid("missing FRR version output"))?;
    let mut words = version.split_whitespace();
    if words.next() != Some("FRRouting")
        || !words.next().is_some_and(|release| {
            release.starts_with(|c: char| c.is_ascii_digit()) && release.contains('.')
        })
    {
        return Err(invalid(
            "expected FRRouting release in external version output",
        ));
    }
    let interfaces = c
        .interface
        .output(&["ip", "-j", "-4", "addr", "show", "dev", &c.ifname])?
        .as_array()
        .ok_or_else(|| invalid("invalid interface output"))?;
    let local = port.ipv4.to_string();
    let [link] = interfaces.as_slice() else {
        return Err(invalid("expected exactly one external egress interface"));
    };
    let has_address = link["addr_info"].as_array().is_some_and(|addresses| {
        addresses.iter().any(|a| {
            a["family"] == "inet"
                && a["local"] == local
                && a["prefixlen"].as_u64() == Some(port.prefix_length.into())
        })
    });
    if link["ifname"] != c.ifname || !has_address {
        return Err(invalid(
            "external interface address does not match the modeled egress",
        ));
    }
    let installed = external_phase(&c.installed, &c.prefix, &c.ifname, route.next_hop, true)?;
    let withdrawn = external_phase(&c.withdrawn, &c.prefix, &c.ifname, route.next_hop, false)?;
    Ok(ExternalFrrRouteComparison {
        scope: "external_frr_route_comparison",
        native_id: c.native_id,
        native_revision: c.native_revision,
        native_format_version: c.native_format_version,
        origin: c.origin,
        network_namespace: c.network_namespace,
        frr_version: version.into(),
        node_id: c.node_id,
        prefix: c.prefix,
        ifname: c.ifname,
        expected: FibSelection {
            egress_port: port.id.clone(),
            gateway: Some(route.next_hop),
        },
        expected_kernel_protocol: RTPROT_ZSTATIC,
        status: match_status(
            installed.status == ComparisonStatus::Match
                && withdrawn.status == ComparisonStatus::Match,
        ),
        installed,
        withdrawn,
    })
}

fn check_observation(
    plan: &RealStackPlan,
    observation: &StackObservation,
) -> Result<(), RealStackError> {
    for (field, actual, expected) in [
        (
            "native_id",
            observation.native_id.as_str(),
            plan.native_id(),
        ),
        (
            "native_revision",
            observation.native_revision.as_str(),
            plan.native_revision(),
        ),
        (
            "image reference",
            observation.image.reference.as_str(),
            plan.image(),
        ),
        (
            "route_configuration_source",
            observation.route_configuration_source,
            LINUX_KERNEL_STATIC,
        ),
    ] {
        if actual != expected {
            return Err(invalid(format!(
                "{field}: observed {actual:?}, expected {expected:?}"
            )));
        }
    }
    if observation.native_format_version != plan.native_format_version() {
        return Err(invalid(format!(
            "native_format_version: observed {}, expected {}",
            observation.native_format_version,
            plan.native_format_version()
        )));
    }
    let value = |plan: &RealStackPlan| {
        serde_json::to_value(plan)
            .map_err(|e| invalid(format!("cannot serialize effective_plan: {e}")))
    };
    if value(&observation.effective_plan)? != value(plan)? {
        return Err(invalid(
            "observation effective_plan differs from the native execution plan",
        ));
    }
    let has_probes = !observation.probes.is_empty();
    if (has_probes || !observation.image.id.is_empty())
        && !observation
            .image
            .id
            .strip_prefix("sha256:")
            .is_some_and(|digest| {
                digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
            })
    {
        return Err(invalid("observation has no valid Docker image ID"));
    }
    let version = plan.image().rsplit_once(':').map(|(_, tag)| tag);
    if (has_probes || !observation.image.frr_version.is_empty())
        && !version.is_some_and(|tag| {
            observation
                .image
                .frr_version
                .split(|c: char| !c.is_ascii_digit() && c != '.')
                .any(|version| version == tag)
        })
    {
        return Err(invalid(
            "observed FRR version differs from the planned image version",
        ));
    }
    if !observation.cleanup.complete || !observation.cleanup.errors.is_empty() {
        return Err(invalid(format!(
            "real-stack cleanup was not successful: {:?}",
            observation.cleanup.errors
        )));
    }
    Ok(())
}

fn check_probe(
    plan: &RealStackPlan,
    probe: &StackProbe,
    observed: &ProbeObservation,
    observation: &StackObservation,
) -> Result<(), RealStackError> {
    let source = &plan.interfaces()[probe.source];
    let destination = &plan.interfaces()[probe.destination];
    for (field, matches) in [
        ("source", observed.source == source.ipv4),
        ("destination", observed.destination == destination.ipv4),
        (
            "payload_bytes",
            observed.payload_bytes == probe.payload_bytes,
        ),
        (
            "ipv4_dont_fragment",
            observed.ipv4_dont_fragment == probe.ipv4_dont_fragment,
        ),
        ("engine", observed.engine == "python3_linux_raw_icmp_df"),
    ] {
        if !matches {
            return Err(invalid(format!(
                "probe {:?}: observed {field} differs from plan",
                probe.id
            )));
        }
    }
    // Request TTL is part of effective_plan; reply TTL measures a different packet.
    let reply = observed.outcome == EchoOutcome::Reply;
    let sent = u32::from(observed.outcome != EchoOutcome::NoRoute);
    if observed.reachable != reply
        || observed.transmitted.is_some_and(|count| count != sent)
        || observed
            .received
            .is_some_and(|count| count != u32::from(reply))
        || (!reply && (observed.reply_ttl.is_some() || observed.reply_ipv4_dont_fragment.is_some()))
        || observed.reply_ttl == Some(0)
    {
        return Err(invalid(format!(
            "probe {:?}: inconsistent Echo outcome or counters",
            probe.id
        )));
    }
    let mut nodes = BTreeSet::new();
    for &index in &observed.route_observations {
        let snapshot = observation.routes.get(index).ok_or_else(|| {
            invalid(format!(
                "probe {:?}: unknown route observation index {index}",
                probe.id
            ))
        })?;
        if !plan.nodes().iter().any(|node| node.id == snapshot.node_id) {
            return Err(invalid(format!(
                "probe {:?}: unknown route observation node {:?}",
                probe.id, snapshot.node_id
            )));
        }
        if !nodes.insert(snapshot.node_id.as_str()) {
            return Err(invalid(format!(
                "probe {:?}: duplicate route observation for node {:?}",
                probe.id, snapshot.node_id
            )));
        }
    }
    Ok(())
}

fn expected_fib(
    plan: &RealStackPlan,
    topology: &Topology,
    node: usize,
    from: usize,
    destination: Ipv4Addr,
) -> Option<FibSelection> {
    let (egress, gateway) = match &topology.devices[node].kind {
        DeviceKind::Host { gateway } if node == plan.interfaces()[from].node => {
            if topology.interface(from).on_link(destination) {
                (from, None)
            } else {
                (from, Some((*gateway)?))
            }
        }
        DeviceKind::Router { routes } => {
            // Native compilation already sorted connected and static routes by prefix length.
            let route = routes.iter().find(|route| route.matches(destination))?;
            (route.egress, route.next_hop)
        }
        _ => unreachable!("real-stack plan only queries routers and the sending host"),
    };
    Some(FibSelection {
        egress_port: plan.interfaces()[egress].id.clone(),
        gateway,
    })
}

fn compare_fib(
    plan: &RealStackPlan,
    topology: &Topology,
    probe: &StackProbe,
    observed: Option<&ProbeObservation>,
) -> Result<Vec<FibComparison>, RealStackError> {
    use ComparisonStatus::{Match, Mismatch, NotComparable};
    let directions = [
        (FibDirection::Request, probe.source, probe.destination),
        (FibDirection::Reply, probe.destination, probe.source),
    ];
    let mut lookups: BTreeMap<_, &KernelLookupObservation> = BTreeMap::new();
    for lookup in observed.into_iter().flat_map(|probe| &probe.kernel_lookups) {
        let (direction, from, to) = match lookup.direction.as_str() {
            "request" => directions[0],
            "reply" => directions[1],
            other => {
                return Err(invalid(format!(
                    "probe {:?}: unknown kernel lookup direction {other:?}",
                    probe.id
                )));
            }
        };
        let from = &plan.interfaces()[from];
        let to = &plan.interfaces()[to];
        let node = plan
            .nodes()
            .iter()
            .enumerate()
            .position(|(i, node)| node.id == lookup.node_id && (i == from.node || node.router))
            .ok_or_else(|| {
                invalid(format!(
                    "probe {:?}: unexpected kernel lookup node {:?} for {:?}",
                    probe.id, lookup.node_id, direction
                ))
            })?;
        if lookup.destination != to.ipv4 {
            return Err(invalid(format!(
                "probe {:?}: kernel lookup destination {} differs from {} for {:?}",
                probe.id, lookup.destination, to.ipv4, direction
            )));
        }
        if let Some(route) = &lookup.selected
            && !plan
                .interfaces()
                .iter()
                .any(|port| port.id == route.egress_port && port.node == node)
        {
            return Err(invalid(format!(
                "probe {:?}: unknown kernel lookup egress {:?} for node {:?}",
                probe.id, route.egress_port, lookup.node_id
            )));
        }
        if lookups.insert((direction, node), lookup).is_some() {
            return Err(invalid(format!(
                "probe {:?}: duplicate kernel lookup for {:?} at {:?}",
                probe.id, direction, lookup.node_id
            )));
        }
    }
    let mut rows = Vec::new();
    for (direction, from, to) in directions {
        let source_node = plan.interfaces()[from].node;
        let destination = plan.interfaces()[to].ipv4;
        for (i, node) in plan.nodes().iter().enumerate() {
            if i != source_node && !node.router {
                continue;
            }
            let expected = expected_fib(plan, topology, i, from, destination);
            let lookup = lookups.get(&(direction, i));
            // ip route get can report dst/32, not the installed route's matching prefix.
            let selected = lookup
                .and_then(|entry| entry.selected.as_ref())
                .map(|route| FibSelection {
                    egress_port: route.egress_port.clone(),
                    gateway: route.gateway,
                });
            let (status, reason) = match (observed, lookup) {
                (None, _) => (NotComparable, "missing_probe_observation"),
                (_, None) => (NotComparable, "missing_kernel_lookup"),
                (_, Some(_)) if expected == selected => (Match, "fib_selection_agrees"),
                (_, Some(_)) => (Mismatch, "fib_selection_differs"),
            };
            rows.push(FibComparison {
                node_id: node.id.clone(),
                direction,
                destination,
                status,
                reason,
                expected,
                observed: selected,
            });
        }
    }
    Ok(rows)
}

fn route_miss(evidence: &Evidence) -> Option<StructuralRouteMiss> {
    let EvidenceFact::Ipv4RouteMiss { destination, .. } = &evidence.fact else {
        return None;
    };
    let direction = match evidence.frame.kind {
        FrameKind::EchoRequest => FibDirection::Request,
        FrameKind::EchoReply => FibDirection::Reply,
        _ => return None,
    };
    (!evidence.model_only).then(|| StructuralRouteMiss {
        evidence_id: evidence.id,
        node_id: evidence.port.device_id.clone(),
        direction,
        destination: *destination,
    })
}

fn compare_echo(
    modeled: Outcome,
    cause: Option<&Evidence>,
    observed: Option<&ProbeObservation>,
    source_node: &str,
    fib: &[FibComparison],
) -> Result<ReachabilityComparison, RealStackError> {
    use ComparisonStatus::{Match, Mismatch, NotComparable};

    let model = match modeled {
        Outcome::Reachable { .. } => ModelEchoOutcome::Reachable,
        Outcome::Timeout { .. } => ModelEchoOutcome::Timeout,
        Outcome::Incomplete { .. } => ModelEchoOutcome::Incomplete,
        Outcome::Undetermined { .. } => ModelEchoOutcome::Undetermined,
    };
    let route_miss = cause.and_then(route_miss);
    let outcome = observed.map(|probe| probe.outcome);
    let source_lookup = fib
        .iter()
        .find(|row| row.node_id == source_node && row.direction == FibDirection::Request);
    let source_route = source_lookup
        .filter(|row| row.status != NotComparable)
        .map(|row| row.observed.is_some());
    if let Some(probe) = observed
        && matches!(
            (probe.outcome, source_route),
            (EchoOutcome::Reply, Some(false)) | (EchoOutcome::NoRoute, Some(true))
        )
    {
        return Err(invalid(format!(
            "probe {:?}: Echo outcome contradicts the source kernel lookup",
            probe.id
        )));
    }
    let (status, reason) = match (model, outcome) {
        (_, None) => (NotComparable, "missing_probe_observation"),
        (ModelEchoOutcome::Incomplete, _) => (NotComparable, "model_incomplete"),
        (ModelEchoOutcome::Undetermined, _) => (NotComparable, "model_undetermined"),
        (ModelEchoOutcome::Reachable, Some(EchoOutcome::Reply)) => {
            (Match, "model_reachable_and_echo_reply")
        }
        (ModelEchoOutcome::Reachable, Some(EchoOutcome::NoRoute))
            if source_route == Some(false) =>
        {
            (Mismatch, "model_reachable_but_local_no_route")
        }
        (ModelEchoOutcome::Reachable, Some(EchoOutcome::Timeout)) => {
            (NotComparable, "echo_timeout_without_structural_evidence")
        }
        (ModelEchoOutcome::Reachable, _) => (
            NotComparable,
            "echo_failure_without_confirmed_local_route_miss",
        ),
        (ModelEchoOutcome::Timeout, Some(outcome)) => match &route_miss {
            None => (NotComparable, "model_timeout_without_structural_evidence"),
            Some(_) if outcome == EchoOutcome::Reply => {
                (Mismatch, "model_route_miss_but_echo_reply")
            }
            Some(miss) => {
                let corroborated = fib.iter().any(|row| {
                    row.node_id == miss.node_id
                        && row.direction == miss.direction
                        && row.destination == miss.destination
                        && row.status == Match
                        && row.expected.is_none()
                        && row.observed.is_none()
                });
                let local = miss.node_id == source_node && miss.direction == FibDirection::Request;
                if corroborated
                    && (outcome == EchoOutcome::Timeout && !local
                        || outcome == EchoOutcome::NoRoute && local)
                {
                    (Match, "structural_route_miss_corroborated_by_fib")
                } else if !corroborated {
                    (NotComparable, "structural_route_miss_without_matching_fib")
                } else {
                    (
                        NotComparable,
                        "echo_failure_does_not_identify_the_modeled_route_miss",
                    )
                }
            }
        },
    };
    Ok(ReachabilityComparison {
        status,
        reason,
        modeled: model,
        observed: outcome,
        route_miss,
    })
}

fn model_echo<'a>(
    report: &'a RunReport,
    id: &str,
) -> Result<(Outcome, Option<&'a Evidence>), RealStackError> {
    match report {
        RunReport::Legacy(report) if id == "1" => Ok((report.outcome, report.cause())),
        RunReport::Scenario(report) => {
            let probe = report
                .probes
                .iter()
                .find(|probe| probe.id == id)
                .ok_or_else(|| invalid(format!("model report has no planned probe {id:?}")))?;
            let cause = match probe.outcome {
                Outcome::Timeout {
                    cause: Some(id), ..
                } => report
                    .evidence
                    .iter()
                    .find(|item| item.probe_id == probe.id && item.evidence.id == id)
                    .map(|item| &item.evidence),
                _ => None,
            };
            Ok((probe.outcome, cause))
        }
        RunReport::Legacy(_) => Err(invalid(format!(
            "legacy model report has no planned probe {id:?}"
        ))),
    }
}

/// Compares a supplied live observation without running Docker or claiming packet-path parity.
pub fn compare_real_stack(
    input: &str,
    observation: &StackObservation,
) -> Result<StackComparison, RealStackError> {
    let plan = plan_real_stack(input)?;
    check_observation(&plan, observation)?;
    let mut observations = BTreeMap::new();
    for observed in &observation.probes {
        let probe = plan
            .probes()
            .iter()
            .find(|probe| probe.id == observed.id)
            .ok_or_else(|| invalid(format!("unknown probe observation {:?}", observed.id)))?;
        if observations
            .insert(observed.id.as_str(), observed)
            .is_some()
        {
            return Err(invalid(format!(
                "duplicate probe observation {:?}",
                observed.id
            )));
        }
        check_probe(&plan, probe, observed, observation)?;
    }
    let report = run_lab(input).map_err(RealStackError::InvalidInput)?;
    let topology = Topology::from_json(input, false).map_err(RealStackError::InvalidInput)?;
    let mut probes = Vec::with_capacity(plan.probes().len());
    for probe in plan.probes() {
        let observed = observations.get(probe.id.as_str()).copied();
        let source = &plan.interfaces()[probe.source];
        let destination = &plan.interfaces()[probe.destination];
        let source_node = &plan.nodes()[source.node].id;
        let (modeled, cause) = model_echo(&report, &probe.id)?;
        let fib = compare_fib(&plan, &topology, probe, observed)?;
        let reachability = compare_echo(modeled, cause, observed, source_node, &fib)?;
        probes.push(StackProbeComparison {
            probe_id: probe.id.clone(),
            source_node: source_node.clone(),
            destination_node: plan.nodes()[destination.node].id.clone(),
            source: source.ipv4,
            destination: destination.ipv4,
            ttl: probe.ttl,
            payload_bytes: probe.payload_bytes,
            ipv4_dont_fragment: probe.ipv4_dont_fragment,
            reachability,
            fib,
        });
    }
    let model_version = match &report {
        RunReport::Legacy(report) => report.model_version,
        RunReport::Scenario(report) => report.model_version,
    };
    Ok(StackComparison {
        native_id: plan.native_id().into(),
        native_revision: plan.native_revision().into(),
        native_format_version: plan.native_format_version(),
        model_version,
        observation_owner: observation.owner.clone(),
        image_reference: observation.image.reference.clone(),
        image_id: observation.image.id.clone(),
        route_configuration_source: observation.route_configuration_source,
        scope: "steady unfragmented IPv4 Echo and kernel FIB selections only; a FIB selection is not a packet path; simulation timing and wall RTT are not compared",
        probes,
    })
}
