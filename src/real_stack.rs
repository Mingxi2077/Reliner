//! Explicit, bounded Docker experiments. Only steady IPv4 reachability and kernel
//! next hops are comparable; simulation timestamps, Ethernet identities and RTTs are not.

use std::collections::BTreeMap;
use std::fmt;
use std::io::Read;
use std::net::Ipv4Addr;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::InputError;
use crate::topology::{DeviceKind, Topology};

pub const REAL_STACK_IMAGE: &str = "quay.io/frrouting/frr:10.7.1";
pub const LINUX_KERNEL_STATIC: &str = "linux_kernel_static";
pub const MAX_REAL_STACK_NODES: usize = 8;
pub const MAX_REAL_STACK_LINKS: usize = 12;
pub const MAX_REAL_STACK_PROBES: usize = 16;
const OWNER_LABEL: &str = "reliner.real-stack.owner";
const MAX_INPUT_BYTES: usize = 262_144;
const MAX_STATIC_ROUTES: usize = 64;
const MAX_RUNTIME: Duration = Duration::from_secs(180);
const CLEANUP_BUDGET: Duration = Duration::from_secs(20);
static NEXT_OWNER: AtomicU64 = AtomicU64::new(0);

// The official image has BusyBox ping without -M. This is the sole probe engine,
// using its bundled Python standard library and a checked Linux DF socket policy.
const FIXED_DF_ECHO: &str = r#"import errno, json, os, socket, struct, sys, time

source, destination = sys.argv[1:3]
ttl, size, seconds = int(sys.argv[3]), int(sys.argv[4]), float(sys.argv[5])
identifier, sequence = os.getpid() & 65535, 1
payload = bytes((i % 251 for i in range(size)))

def checksum(data):
    if len(data) % 2:
        data += b"\x00"
    value = sum(struct.unpack("!%dH" % (len(data) // 2), data))
    value = (value & 65535) + (value >> 16)
    value = (value & 65535) + (value >> 16)
    return (~value) & 65535

header = struct.pack("!BBHHH", 8, 0, 0, identifier, sequence)
packet = struct.pack("!BBHHH", 8, 0, checksum(header + payload), identifier, sequence) + payload
started = time.monotonic()
result = {"engine":"python3_linux_raw_icmp_df", "source":source, "destination":destination,
          "payload_bytes":size, "ipv4_dont_fragment":True, "transmitted":0, "received":0,
          "outcome":"timeout", "reply_ttl":None, "reply_ipv4_dont_fragment":None,
          "send_errno":None, "icmp_error_type":None, "icmp_error_code":None,
          "clock":"container_monotonic_elapsed_ms"}
with socket.socket(socket.AF_INET, socket.SOCK_RAW, socket.IPPROTO_ICMP) as connection:
    connection.bind((source, 0))
    connection.setsockopt(socket.IPPROTO_IP, socket.IP_TTL, ttl)
    connection.setsockopt(socket.IPPROTO_IP, 10, 2)
    if connection.getsockopt(socket.IPPROTO_IP, 10) != 2:
        raise RuntimeError("IP_MTU_DISCOVER did not retain IP_PMTUDISC_DO")
    try:
        sent = connection.sendto(packet, (destination, 0))
        if sent != len(packet):
            raise RuntimeError("partial ICMP send")
        result["transmitted"] = 1
    except OSError as error:
        if error.errno not in (errno.ENETUNREACH, errno.EHOSTUNREACH):
            raise
        result["outcome"], result["send_errno"] = "no_route", error.errno
    deadline = started + seconds
    while result["transmitted"]:
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            break
        connection.settimeout(remaining)
        try:
            data, peer = connection.recvfrom(65535)
        except socket.timeout:
            break
        if len(data) < 28 or data[0] >> 4 != 4:
            continue
        offset = (data[0] & 15) * 4
        icmp = data[offset:]
        if len(icmp) < 8:
            continue
        kind, code, _, found_id, found_seq = struct.unpack("!BBHHH", icmp[:8])
        if kind == 0 and code == 0 and peer[0] == destination and (found_id, found_seq) == (identifier, sequence):
            if icmp[8:] != payload or checksum(icmp) != 0:
                raise RuntimeError("Echo reply checksum or payload mismatch")
            result["received"], result["outcome"] = 1, "reply"
            result["reply_ttl"] = data[8]
            result["reply_ipv4_dont_fragment"] = bool(struct.unpack("!H", data[6:8])[0] & 16384)
            break
        if kind in (3, 11) and len(icmp) >= 36:
            quoted = icmp[8:]
            inner_offset = (quoted[0] & 15) * 4
            inner = quoted[inner_offset:inner_offset + 8]
            if len(inner) != 8 or socket.inet_ntoa(quoted[16:20]) != destination:
                continue
            inner_kind, _, _, inner_id, inner_seq = struct.unpack("!BBHHH", inner)
            if inner_kind == 8 and (inner_id, inner_seq) == (identifier, sequence):
                result["outcome"] = "icmp_error"
                result["icmp_error_type"], result["icmp_error_code"] = kind, code
                break
result["elapsed_ms"] = (time.monotonic() - started) * 1000
print(json.dumps(result, separators=(",", ":"), sort_keys=True))
"#;

#[derive(Debug)]
pub enum RealStackError {
    InvalidInput(InputError),
    Unsupported {
        feature: String,
        detail: String,
    },
    LimitExceeded {
        resource: String,
        actual: usize,
        maximum: usize,
    },
    InvalidOptions(String),
    CommandFailed {
        argv: Vec<String>,
        exit_code: Option<i32>,
        stdout: String,
        stderr: String,
    },
    Io(String),
    InvalidObservation(String),
    Timeout {
        stage: String,
        last_observation: String,
    },
    RunFailed {
        cause: Box<RealStackError>,
        observation: Box<StackObservation>,
    },
}

impl fmt::Display for RealStackError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidInput(e) => write!(f, "{e}"),
            Self::Unsupported { feature, detail } => write!(f, "Unsupported ({feature}): {detail}"),
            Self::LimitExceeded {
                resource,
                actual,
                maximum,
            } => write!(f, "{resource}: {actual} exceeds {maximum}"),
            Self::InvalidOptions(s) | Self::Io(s) | Self::InvalidObservation(s) => f.write_str(s),
            Self::CommandFailed {
                argv,
                exit_code,
                stdout,
                stderr,
            } => write!(f, "docker {argv:?}: exit {exit_code:?}\n{stdout}\n{stderr}"),
            Self::Timeout {
                stage,
                last_observation,
            } => write!(f, "timeout: {stage}; last observation: {last_observation}"),
            Self::RunFailed { cause, observation } => write!(
                f,
                "{cause}; cleanup complete: {}; cleanup errors: {:?}",
                observation.cleanup.complete, observation.cleanup.errors
            ),
        }
    }
}

impl std::error::Error for RealStackError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidInput(e) => Some(e),
            Self::RunFailed { cause, .. } => Some(cause.as_ref()),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct StackNode {
    pub id: String,
    pub router: bool,
    pub gateway: Option<Ipv4Addr>,
}

#[derive(Clone, Debug, Serialize)]
pub struct StackInterface {
    pub id: String,
    pub node: usize,
    pub ipv4: Ipv4Addr,
    pub mac: String,
    pub ipv4_mtu_bytes: u16,
    pub prefix_length: u8,
}

#[derive(Clone, Debug, Serialize)]
pub struct StackLink {
    pub id: String,
    pub interfaces: [usize; 2],
}

#[derive(Clone, Debug, Serialize)]
pub struct StackRoute {
    pub node: usize,
    pub network: Ipv4Addr,
    pub prefix_length: u8,
    pub egress: usize,
    pub next_hop: Ipv4Addr,
}

#[derive(Clone, Debug, Serialize)]
pub struct StackProbe {
    pub id: String,
    pub source: usize,
    pub destination: usize,
    pub ttl: u8,
    pub payload_bytes: u16,
    pub ipv4_dont_fragment: bool,
}

/// Private storage prevents callers from bypassing native compilation and limits.
#[derive(Clone, Debug, Serialize)]
pub struct RealStackPlan {
    native_id: String,
    native_revision: String,
    native_format_version: u32,
    image: String,
    nodes: Vec<StackNode>,
    interfaces: Vec<StackInterface>,
    links: Vec<StackLink>,
    routes: Vec<StackRoute>,
    probes: Vec<StackProbe>,
}

impl RealStackPlan {
    pub fn native_id(&self) -> &str {
        &self.native_id
    }
    pub fn native_revision(&self) -> &str {
        &self.native_revision
    }
    pub fn native_format_version(&self) -> u32 {
        self.native_format_version
    }
    pub fn image(&self) -> &str {
        &self.image
    }
    pub fn nodes(&self) -> &[StackNode] {
        &self.nodes
    }
    pub fn interfaces(&self) -> &[StackInterface] {
        &self.interfaces
    }
    pub fn links(&self) -> &[StackLink] {
        &self.links
    }
    pub fn routes(&self) -> &[StackRoute] {
        &self.routes
    }
    pub fn probes(&self) -> &[StackProbe] {
        &self.probes
    }
}

fn unsupported(feature: &str, detail: impl Into<String>) -> RealStackError {
    RealStackError::Unsupported {
        feature: feature.into(),
        detail: detail.into(),
    }
}

fn limit(resource: &str, actual: usize, maximum: usize) -> Result<(), RealStackError> {
    if actual > maximum {
        Err(RealStackError::LimitExceeded {
            resource: resource.into(),
            actual,
            maximum,
        })
    } else {
        Ok(())
    }
}

/// Pure planning: no Docker commands, imports, services, or simulated execution.
pub fn plan_real_stack(input: &str) -> Result<RealStackPlan, RealStackError> {
    limit("input bytes", input.len(), MAX_INPUT_BYTES)?;
    let input_error = |error: InputError| {
        if error.code == "unsupported_version"
            || error.code == "unsupported_feature"
            || error.code == "invalid_routing_configuration"
            || error.code == "invalid_stp_configuration"
            || error.code == "invalid_stp_convergence"
            || error.code == "missing_stp_convergence"
            || error.code == "missing_switch"
            || error.message.contains("unknown field")
            || error.message.contains("missing field")
            || error.message.contains("unknown variant")
        {
            unsupported("native_schema", error.to_string())
        } else {
            RealStackError::InvalidInput(error)
        }
    };
    crate::validate_lab(input).map_err(input_error)?;
    let topology = Topology::from_json(input, false).map_err(input_error)?;
    if topology.has_ipv4_acls() {
        return Err(unsupported(
            "ipv4_acl",
            "router interface IPv4 ACLs are not implemented by the real stack",
        ));
    }
    topology
        .validate_workspace_labels()
        .map_err(RealStackError::InvalidInput)?;
    limit("nodes", topology.devices.len(), MAX_REAL_STACK_NODES)?;
    limit("links", topology.links.len(), MAX_REAL_STACK_LINKS)?;
    limit("probes", topology.probes.len(), MAX_REAL_STACK_PROBES)?;
    // Presence matters even if a convergence setting compiles to an empty plan.
    let document: Value = serde_json::from_str(input.trim_start_matches('\u{feff}'))
        .map_err(|e| RealStackError::InvalidObservation(e.to_string()))?;
    if topology
        .import_source
        .as_ref()
        .is_some_and(|source| !source.gaps.is_empty())
    {
        return Err(unsupported(
            "import_gaps",
            "imported behavior has unimplemented or unknown native semantics",
        ));
    }
    for key in ["routing_convergence_ns", "stp_convergence_ns"] {
        if document["scenario"].get(key).is_some() {
            return Err(unsupported(
                key,
                "convergence is outside steady static IPv4",
            ));
        }
    }
    if document["devices"]
        .as_array()
        .is_some_and(|devices| devices.iter().any(|d| d.get("link_state").is_some()))
    {
        return Err(unsupported(
            "dynamic_routing",
            "link-state configuration is outside static IPv4",
        ));
    }
    if topology.routing.is_some() {
        return Err(unsupported(
            "dynamic_routing",
            "link-state routing is not implemented",
        ));
    }
    if topology.stp.is_some() {
        return Err(unsupported("stp", "STP is not implemented"));
    }
    if !topology.link_actions.is_empty() || !topology.protection.is_empty() {
        return Err(unsupported(
            "link_actions",
            "link changes and standby links are not implemented",
        ));
    }
    if !topology.warmup.is_empty() {
        return Err(unsupported(
            "warmup",
            "scheduled neighbor actions are not steady-state probes",
        ));
    }
    if topology
        .ports
        .iter()
        .any(|p| p.interface().is_some_and(|i| i.ipv4_mtu_bytes.is_some()))
    {
        return Err(unsupported(
            "mtu",
            "explicit MTU and fragmentation semantics are not comparable yet",
        ));
    }
    if topology.probes.iter().any(|p| p.payload_bytes > 1472) {
        return Err(unsupported(
            "payload_mtu",
            "only unfragmented IPv4 Echo within the default 1500-byte MTU is comparable",
        ));
    }
    let mut nodes = Vec::new();
    let mut routes = Vec::new();
    for (node, device) in topology.devices.iter().enumerate() {
        let (router, gateway) = match &device.kind {
            DeviceKind::Host { gateway } => (false, *gateway),
            DeviceKind::Router { routes: compiled } => {
                for route in compiled {
                    if let Some(next_hop) = route.next_hop {
                        if compiled.iter().any(|r| {
                            r.next_hop.is_none()
                                && r.network == route.network
                                && r.length == route.length
                        }) {
                            return Err(unsupported(
                                "shadowed_static_route",
                                "static prefix equals a connected prefix",
                            ));
                        }
                        routes.push(StackRoute {
                            node,
                            network: route.network,
                            prefix_length: route.length,
                            egress: route.egress,
                            next_hop,
                        });
                    }
                }
                (true, None)
            }
            DeviceKind::Switch => {
                return Err(unsupported(
                    "switch",
                    "only IPv4 hosts and static routers are supported",
                ));
            }
        };
        nodes.push(StackNode {
            id: device.id.clone(),
            router,
            gateway,
        });
    }
    limit("static routes", routes.len(), MAX_STATIC_ROUTES)?;
    let mut interfaces = Vec::new();
    for (port, item) in topology.ports.iter().enumerate() {
        if item.link.is_none() {
            return Err(unsupported(
                "unattached_port",
                format!("port {} has no physical link", item.id),
            ));
        }
        let interface = item
            .interface()
            .ok_or_else(|| unsupported("switch_port", &item.id))?;
        if interface.ipv4.is_loopback()
            || interface.ipv4.is_multicast()
            || interface.ipv4.octets()[0] == 0
            || interface.ipv4.octets()[0] >= 240
        {
            return Err(unsupported("special_ipv4", interface.ipv4.to_string()));
        }
        interfaces.push(StackInterface {
            id: item.id.clone(),
            node: item.device,
            ipv4: interface.ipv4,
            mac: interface.mac.text(),
            ipv4_mtu_bytes: 1500,
            prefix_length: interface.connected_route(port).length,
        });
    }
    let mut links = Vec::new();
    for item in &topology.links {
        if item.transmission.is_some() {
            return Err(unsupported(
                "transmission",
                "rate and queue limits are not implemented",
            ));
        }
        let [a, b] = item.ports.map(|p| {
            topology.ports[p]
                .interface()
                .expect("checked IP interfaces")
        });
        if !a.on_link(b.ipv4) || !b.on_link(a.ipv4) {
            return Err(unsupported("asymmetric_link_subnet", &item.id));
        }
        links.push(StackLink {
            id: item.id.clone(),
            interfaces: item.ports,
        });
    }
    let probes = topology
        .probes
        .iter()
        .map(|p| StackProbe {
            id: topology.probe_names[p.id].clone(),
            source: p.source,
            destination: p.destination,
            ttl: p.ttl,
            payload_bytes: p.payload_bytes,
            ipv4_dont_fragment: true,
        })
        .collect();
    Ok(RealStackPlan {
        native_id: topology.id,
        native_revision: topology.revision,
        native_format_version: topology.format_version,
        image: REAL_STACK_IMAGE.into(),
        nodes,
        interfaces,
        links,
        routes,
        probes,
    })
}

#[derive(Clone, Copy, Debug)]
pub struct RealStackOptions {
    /// Includes setup, kernel route verification and probes; cleanup has a separate 20s budget.
    pub run_timeout: Duration,
    pub command_timeout: Duration,
    /// Budget for reading back the kernel tables after synchronous route installation.
    pub route_ready_timeout: Duration,
    pub ping_timeout: Duration,
}

impl Default for RealStackOptions {
    fn default() -> Self {
        Self {
            run_timeout: Duration::from_secs(120),
            command_timeout: Duration::from_secs(10),
            route_ready_timeout: Duration::from_secs(8),
            ping_timeout: Duration::from_secs(2),
        }
    }
}

impl RealStackOptions {
    fn validate(self) -> Result<(), RealStackError> {
        if self.run_timeout < Duration::from_secs(1)
            || self.run_timeout > MAX_RUNTIME
            || self.command_timeout.is_zero()
            || self.command_timeout > Duration::from_secs(15)
            || self.route_ready_timeout.is_zero()
            || self.route_ready_timeout > Duration::from_secs(30)
            || self.ping_timeout < Duration::from_secs(1)
            || self.ping_timeout > Duration::from_secs(5)
            || self.ping_timeout.subsec_nanos() != 0
            || self.command_timeout < self.ping_timeout + Duration::from_secs(1)
        {
            return Err(RealStackError::InvalidOptions("run timeout must be 1..180s, command timeout <=15s and >=ping+1s, readiness <=30s, and ping an integer 1..5s".into()));
        }
        Ok(())
    }
}

#[derive(Debug, Default, Serialize)]
pub struct ImageObservation {
    pub reference: String,
    pub id: String,
    pub repo_digests: Vec<String>,
    /// Packaged software version only; no FRR daemon runs in the experiment.
    pub frr_version: String,
    pub ip_version: String,
    pub ping_implementation: String,
    pub python_version: String,
    pub docker_server: String,
}

#[derive(Debug, Serialize)]
pub struct ObservationClock {
    pub kind: String,
    pub unix_start_ms: u128,
    pub elapsed_ms: u128,
}

#[derive(Clone, Debug, Serialize)]
pub struct CommandObservation {
    pub argv: Vec<String>,
    pub started_ms: u128,
    pub finished_ms: u128,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug, Serialize)]
pub struct InterfaceObservation {
    pub port_id: String,
    pub container: String,
    pub network: String,
    pub endpoint_mac: String,
    pub native_mac: String,
    pub ifname: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct KernelNextHop {
    pub prefix: String,
    pub egress_port: String,
    pub gateway: Option<Ipv4Addr>,
    pub protocol: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct RouteObservation {
    pub node_id: String,
    pub observed_ms: u128,
    pub routes: Vec<KernelNextHop>,
    pub raw_json: String,
}

fn verify_static_route(
    observed: &RouteObservation,
    prefix: &str,
    port: &str,
    gateway: Ipv4Addr,
) -> Result<(), RealStackError> {
    if observed.routes.iter().any(|r| {
        r.prefix == prefix
            && r.egress_port == port
            && r.gateway == Some(gateway)
            && r.protocol.as_deref() == Some("static")
    }) {
        return Ok(());
    }
    Err(observation_error(format!(
        "{}: kernel did not retain {prefix} via {gateway} dev {port} proto static; observed {}",
        observed.node_id, observed.raw_json
    )))
}

#[derive(Debug, Serialize)]
pub struct KernelLookupObservation {
    pub node_id: String,
    pub direction: String,
    pub destination: Ipv4Addr,
    pub observed_ms: u128,
    /// A FIB selection, not a claim that the packet traversed this router.
    pub selected: Option<KernelNextHop>,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug, Serialize)]
pub struct ProbeObservation {
    pub id: String,
    pub source: Ipv4Addr,
    pub destination: Ipv4Addr,
    pub payload_bytes: u16,
    /// Request policy enforced by IP_PMTUDISC_DO; reply DF is observed separately.
    pub ipv4_dont_fragment: bool,
    pub engine: String,
    pub outcome: EchoOutcome,
    pub reply_ttl: Option<u8>,
    pub reply_ipv4_dont_fragment: Option<bool>,
    pub container_elapsed_ms: f64,
    pub started_ms: u128,
    pub finished_ms: u128,
    pub reachable: bool,
    /// Counts returned by the raw ICMP probe engine, including a failed send.
    pub transmitted: Option<u32>,
    pub received: Option<u32>,
    /// Actual route-table snapshot indices for this probe, including return routing.
    pub route_observations: Vec<usize>,
    pub kernel_lookups: Vec<KernelLookupObservation>,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EchoOutcome {
    Reply,
    Timeout,
    NoRoute,
    IcmpError,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawEchoObservation {
    engine: String,
    source: Ipv4Addr,
    destination: Ipv4Addr,
    payload_bytes: u16,
    ipv4_dont_fragment: bool,
    transmitted: u32,
    received: u32,
    outcome: EchoOutcome,
    reply_ttl: Option<u8>,
    reply_ipv4_dont_fragment: Option<bool>,
    send_errno: Option<i32>,
    icmp_error_type: Option<u8>,
    icmp_error_code: Option<u8>,
    clock: String,
    elapsed_ms: f64,
}

fn parse_echo(
    raw: &str,
    source: Ipv4Addr,
    destination: Ipv4Addr,
    payload_bytes: u16,
    source_has_route: bool,
) -> Result<RawEchoObservation, RealStackError> {
    let echo: RawEchoObservation = serde_json::from_str(raw)
        .map_err(|e| observation_error(format!("invalid raw Echo observation: {e}")))?;
    if echo.engine != "python3_linux_raw_icmp_df"
        || echo.source != source
        || echo.destination != destination
        || echo.payload_bytes != payload_bytes
        || !echo.ipv4_dont_fragment
        || echo.clock != "container_monotonic_elapsed_ms"
        || !echo.elapsed_ms.is_finite()
        || echo.elapsed_ms < 0.0
    {
        return Err(observation_error(
            "Echo identity, payload, DF policy or observation clock differs from plan",
        ));
    }
    let no_reply = echo.reply_ttl.is_none() && echo.reply_ipv4_dont_fragment.is_none();
    let no_icmp_error = echo.icmp_error_type.is_none() && echo.icmp_error_code.is_none();
    let valid = match echo.outcome {
        EchoOutcome::Reply => {
            source_has_route
                && echo.transmitted == 1
                && echo.received == 1
                && echo.reply_ttl.is_some_and(|ttl| ttl > 0)
                && echo.reply_ipv4_dont_fragment.is_some()
                && echo.send_errno.is_none()
                && no_icmp_error
        }
        EchoOutcome::Timeout => {
            echo.transmitted == 1
                && echo.received == 0
                && no_reply
                && echo.send_errno.is_none()
                && no_icmp_error
        }
        EchoOutcome::NoRoute => {
            !source_has_route
                && echo.transmitted == 0
                && echo.received == 0
                && no_reply
                && matches!(echo.send_errno, Some(101 | 113))
                && no_icmp_error
        }
        EchoOutcome::IcmpError => {
            echo.transmitted == 1
                && echo.received == 0
                && no_reply
                && echo.send_errno.is_none()
                && matches!(echo.icmp_error_type, Some(3 | 11))
                && echo.icmp_error_code.is_some()
        }
    };
    if !valid {
        return Err(observation_error(
            "inconsistent Echo counters, FIB result or packet metadata",
        ));
    }
    Ok(echo)
}

#[derive(Debug, Default, Serialize)]
pub struct CleanupObservation {
    pub complete: bool,
    pub errors: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct StackObservation {
    pub native_id: String,
    pub native_revision: String,
    pub native_format_version: u32,
    pub effective_plan: RealStackPlan,
    /// Source of configured IPv4 routes, independent of the tools image.
    pub route_configuration_source: &'static str,
    pub owner: String,
    pub image: ImageObservation,
    pub clock: ObservationClock,
    pub comparison_scope: String,
    pub interfaces: Vec<InterfaceObservation>,
    pub routes: Vec<RouteObservation>,
    pub probes: Vec<ProbeObservation>,
    pub commands: Vec<CommandObservation>,
    pub cleanup: CleanupObservation,
}

fn strings(args: &[&str]) -> Vec<String> {
    args.iter().map(|s| (*s).into()).collect()
}

fn container_args(
    owner: &str,
    name: &str,
    image_id: &str,
    router: bool,
    network: &str,
) -> Vec<String> {
    strings(&[
        "create",
        "--name",
        name,
        "--label",
        &format!("{OWNER_LABEL}={owner}"),
        "--network",
        network,
        "--cap-drop",
        "ALL",
        "--cap-add",
        "NET_ADMIN",
        "--cap-add",
        "NET_RAW",
        "--sysctl",
        if router {
            "net.ipv4.ip_forward=1"
        } else {
            "net.ipv4.ip_forward=0"
        },
        "--sysctl",
        "net.ipv4.conf.all.rp_filter=0",
        "--sysctl",
        "net.ipv4.conf.default.rp_filter=0",
        "--sysctl",
        "net.ipv4.conf.all.send_redirects=0",
        "--sysctl",
        "net.ipv4.conf.default.send_redirects=0",
        "--sysctl",
        "net.ipv4.conf.all.accept_redirects=0",
        "--sysctl",
        "net.ipv4.conf.default.accept_redirects=0",
        "--entrypoint",
        "/bin/sleep",
        image_id,
        "240",
    ])
}

fn network_args(owner: &str, name: &str) -> Vec<String> {
    // Docker's IPv4 internal rules drop transit packets outside its IPAM subnet.
    // IPv6-only IPAM supplies an isolated L2 bridge; the experiment removes IPv6.
    strings(&[
        "network",
        "create",
        "--driver",
        "bridge",
        "--internal",
        "--ipv4=false",
        "--ipv6",
        "--opt",
        "com.docker.network.bridge.gateway_mode_ipv6=isolated",
        "--label",
        &format!("{OWNER_LABEL}={owner}"),
        name,
    ])
}

fn static_route_args<'a>(prefix: &'a str, gateway: &'a str, ifname: &'a str) -> [&'a str; 11] {
    [
        "ip", "-4", "route", "replace", prefix, "via", gateway, "dev", ifname, "proto", "static",
    ]
}

fn json(raw: &str) -> Result<Value, RealStackError> {
    serde_json::from_str(raw)
        .map_err(|e| RealStackError::InvalidObservation(format!("invalid command JSON: {e}")))
}

fn observation_error(message: impl Into<String>) -> RealStackError {
    RealStackError::InvalidObservation(message.into())
}

fn endpoint_interface(
    network: &Value,
    container: &str,
    links: &Value,
) -> Result<(String, String), RealStackError> {
    let endpoints = network[0]["Containers"]
        .as_object()
        .ok_or_else(|| observation_error("missing network endpoints"))?;
    let matching: Vec<_> = endpoints
        .values()
        .filter(|v| v["Name"].as_str() == Some(container))
        .collect();
    if matching.len() != 1 {
        return Err(observation_error("expected one owned container endpoint"));
    }
    let mac = matching[0]["MacAddress"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| observation_error("endpoint has no MAC"))?;
    let interfaces = links
        .as_array()
        .ok_or_else(|| observation_error("ip link is not an array"))?;
    let matching: Vec<_> = interfaces
        .iter()
        .filter(|v| {
            v["address"]
                .as_str()
                .is_some_and(|s| s.eq_ignore_ascii_case(mac))
        })
        .collect();
    if matching.len() != 1 {
        return Err(observation_error(
            "endpoint MAC does not identify exactly one interface",
        ));
    }
    let name = matching[0]["ifname"]
        .as_str()
        .ok_or_else(|| observation_error("interface has no name"))?;
    // Only observed kernel names are used as interface arguments.
    if name.is_empty()
        || name.len() > 15
        || !name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_.-".contains(&c))
    {
        return Err(observation_error("unsafe kernel interface name"));
    }
    Ok((mac.into(), name.into()))
}

fn parse_routes(
    raw: &str,
    mappings: &BTreeMap<String, String>,
) -> Result<Vec<KernelNextHop>, RealStackError> {
    let value = json(raw)?;
    let entries = value
        .as_array()
        .ok_or_else(|| observation_error("route table is not an array"))?;
    let mut routes = Vec::new();
    for route in entries {
        if route.get("nexthops").is_some() {
            return Err(unsupported(
                "multipath_observation",
                "unexpected ECMP kernel route",
            ));
        }
        if route
            .get("type")
            .and_then(Value::as_str)
            .is_some_and(|t| t != "unicast")
        {
            return Err(observation_error(
                "unexpected non-unicast route in experiment",
            ));
        }
        let dev = route["dev"]
            .as_str()
            .ok_or_else(|| observation_error("kernel route has no egress"))?;
        let port = mappings.get(dev).ok_or_else(|| {
            observation_error(format!("kernel route uses unknown interface {dev}"))
        })?;
        let prefix = route["dst"]
            .as_str()
            .ok_or_else(|| observation_error("kernel route has no destination"))?;
        let prefix: String = if prefix == "default" {
            "0.0.0.0/0".into()
        } else if prefix.contains('/') {
            prefix.into()
        } else {
            format!("{prefix}/32")
        };
        let (address, length) = prefix
            .split_once('/')
            .ok_or_else(|| observation_error("invalid route prefix"))?;
        let _: Ipv4Addr = address
            .parse()
            .map_err(|_| observation_error("invalid kernel IPv4 prefix"))?;
        let length: u8 = length
            .parse()
            .map_err(|_| observation_error("invalid kernel prefix length"))?;
        if length > 32 {
            return Err(observation_error("invalid kernel prefix length"));
        }
        let gateway = route
            .get("gateway")
            .map(|v| {
                v.as_str()
                    .ok_or_else(|| observation_error("gateway is not IPv4"))?
                    .parse()
                    .map_err(|_| observation_error("invalid kernel gateway"))
            })
            .transpose()?;
        routes.push(KernelNextHop {
            prefix,
            egress_port: port.clone(),
            gateway,
            protocol: route.get("protocol").map(|p| {
                p.as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| p.to_string())
            }),
        });
    }
    Ok(routes)
}

fn parse_lookup(
    output: &CommandObservation,
    mappings: &BTreeMap<String, String>,
) -> Result<Option<KernelNextHop>, RealStackError> {
    if output.exit_code == Some(2)
        && output.stdout.is_empty()
        && output
            .stderr
            .lines()
            .eq(["RTNETLINK answers: Network unreachable"])
    {
        return Ok(None);
    }
    if output.exit_code != Some(0) {
        return Err(RealStackError::CommandFailed {
            argv: output.argv.clone(),
            exit_code: output.exit_code,
            stdout: output.stdout.clone(),
            stderr: output.stderr.clone(),
        });
    }
    let mut routes = parse_routes(&output.stdout, mappings)?;
    if routes.len() != 1 {
        return Err(observation_error(
            "kernel route get did not select exactly one route",
        ));
    }
    Ok(routes.pop())
}

struct Runner {
    started: Instant,
    deadline: Instant,
    options: RealStackOptions,
    observation: StackObservation,
}

impl Runner {
    fn call(&mut self, argv: Vec<String>) -> Result<CommandObservation, RealStackError> {
        let remaining = self.deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(RealStackError::Timeout {
                stage: "run budget".into(),
                last_observation: String::new(),
            });
        }
        let started_ms = self.started.elapsed().as_millis();
        let mut child = Command::new("docker")
            .args(&argv)
            .env("LC_ALL", "C")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| RealStackError::Io(format!("docker {argv:?}: {e}")))?;
        let out = child.stdout.take().expect("piped stdout");
        let err = child.stderr.take().expect("piped stderr");
        // Drain pipes concurrently so a failed Docker command cannot block on stderr.
        let read = |mut stream: Box<dyn Read + Send>| {
            let mut bytes = Vec::new();
            stream.read_to_end(&mut bytes).map(|_| bytes)
        };
        let stdout = thread::spawn(move || read(Box::new(out)));
        let stderr = thread::spawn(move || read(Box::new(err)));
        let command_deadline = Instant::now() + remaining.min(self.options.command_timeout);
        let mut timed_out = false;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if Instant::now() >= command_deadline => {
                    timed_out = true;
                    child.kill().map_err(|e| {
                        RealStackError::Io(format!("kill timed-out docker command: {e}"))
                    })?;
                    break child
                        .wait()
                        .map_err(|e| RealStackError::Io(e.to_string()))?;
                }
                Ok(None) => thread::sleep(Duration::from_millis(10)),
                Err(e) => {
                    let kill = child.kill();
                    let wait = child.wait();
                    return Err(RealStackError::Io(format!(
                        "docker wait: {e}; kill={kill:?}; reap={wait:?}"
                    )));
                }
            }
        };
        let decode = |handle: thread::JoinHandle<std::io::Result<Vec<u8>>>| -> Result<String, RealStackError> {
            let bytes = handle.join().map_err(|_| RealStackError::Io("Docker output reader panicked".into()))?
                .map_err(|e| RealStackError::Io(e.to_string()))?;
            String::from_utf8(bytes).map_err(|e| observation_error(e.to_string()))
        };
        let result = CommandObservation {
            argv,
            started_ms,
            finished_ms: self.started.elapsed().as_millis(),
            exit_code: status.code(),
            timed_out,
            stdout: decode(stdout)?,
            stderr: decode(stderr)?,
        };
        self.observation.commands.push(result.clone());
        if timed_out {
            return Err(RealStackError::Timeout {
                stage: format!("docker {:?}", result.argv),
                last_observation: format!("{}\n{}", result.stdout, result.stderr),
            });
        }
        Ok(result)
    }

    fn checked(&mut self, args: Vec<String>) -> Result<CommandObservation, RealStackError> {
        let result = self.call(args)?;
        if result.exit_code != Some(0) {
            return Err(RealStackError::CommandFailed {
                argv: result.argv,
                exit_code: result.exit_code,
                stdout: result.stdout,
                stderr: result.stderr,
            });
        }
        Ok(result)
    }

    fn exec(
        &mut self,
        container: &str,
        args: &[&str],
    ) -> Result<CommandObservation, RealStackError> {
        let mut argv = strings(&["exec", "--env", "LC_ALL=C", container]);
        argv.extend(strings(args));
        self.checked(argv)
    }

    fn snapshot(
        &mut self,
        plan: &RealStackPlan,
        node: usize,
        container: &str,
    ) -> Result<usize, RealStackError> {
        let output = self.exec(
            container,
            &["ip", "-j", "-4", "route", "show", "table", "main"],
        )?;
        let mappings = self
            .observation
            .interfaces
            .iter()
            .filter(|m| m.container == container)
            .map(|m| (m.ifname.clone(), m.port_id.clone()))
            .collect();
        let routes = parse_routes(&output.stdout, &mappings)?;
        let index = self.observation.routes.len();
        self.observation.routes.push(RouteObservation {
            node_id: plan.nodes[node].id.clone(),
            observed_ms: output.finished_ms,
            routes,
            raw_json: output.stdout,
        });
        Ok(index)
    }

    fn setup(&mut self, plan: &RealStackPlan) -> Result<Vec<String>, RealStackError> {
        let server = self.checked(strings(&[
            "version",
            "--format",
            "{{.Server.Version}} {{.Server.Os}} {{.Server.Arch}}",
        ]))?;
        self.observation.image.docker_server = server.stdout.trim().into();
        if !server.stdout.contains("linux amd64") {
            return Err(unsupported("docker_platform", server.stdout));
        }
        // Pull is an explicit caller action; the execution budget never waits on a registry.
        let image = self.checked(strings(&[
            "image",
            "inspect",
            REAL_STACK_IMAGE,
            "--format",
            "{{.Id}}\n{{json .RepoDigests}}\n{{.Os}} {{.Architecture}}",
        ]))?;
        let mut image_lines = image.stdout.lines();
        let image_id = image_lines
            .next()
            .filter(|s| s.starts_with("sha256:"))
            .ok_or_else(|| observation_error("image ID is missing"))?
            .to_owned();
        self.observation.image.id = image_id.clone();
        self.observation.image.repo_digests = serde_json::from_str(
            image_lines
                .next()
                .ok_or_else(|| observation_error("image digest is missing"))?,
        )
        .map_err(|e| observation_error(format!("invalid image digests: {e}")))?;
        if image_lines.next() != Some("linux amd64") {
            return Err(unsupported("image_platform", image.stdout));
        }
        let owner = self.observation.owner.clone();
        let containers: Vec<_> = (0..plan.nodes.len())
            .map(|i| format!("{owner}-n{i}"))
            .collect();
        let networks: Vec<_> = (0..plan.links.len())
            .map(|i| format!("{owner}-l{i}"))
            .collect();
        let first_links: Vec<_> = (0..plan.nodes.len())
            .map(|node| {
                plan.links
                    .iter()
                    .position(|l| {
                        l.interfaces
                            .iter()
                            .any(|&p| plan.interfaces[p].node == node)
                    })
                    .expect("attached node")
            })
            .collect();
        for network in &networks {
            self.checked(network_args(&owner, network))?;
        }
        for (node, container) in containers.iter().enumerate() {
            self.checked(container_args(
                &owner,
                container,
                &image_id,
                plan.nodes[node].router,
                &networks[first_links[node]],
            ))?;
            self.checked(strings(&["start", container]))?;
        }
        let tools = &containers[0];
        for path in ["/sbin/ip", "/bin/ping"] {
            self.exec(tools, &["test", "-x", path])?;
        }
        self.observation.image.ip_version =
            self.exec(tools, &["ip", "-Version"])?.stdout.trim().into();
        self.observation.image.frr_version = self
            .exec(tools, &["/usr/lib/frr/zebra", "--version"])?
            .stdout
            .trim()
            .into();
        // BusyBox --help exits successfully; -V is unsupported in this official image.
        let ping_help = self.exec(tools, &["ping", "--help"])?;
        self.observation.image.ping_implementation =
            format!("{}{}", ping_help.stdout, ping_help.stderr);
        self.observation.image.python_version = self
            .exec(tools, &["python3", "--version"])?
            .stdout
            .trim()
            .into();
        if !self.observation.image.frr_version.contains("10.7.1") {
            return Err(unsupported("frr_version", "expected official FRR 10.7.1"));
        }
        let mut names = vec![String::new(); plan.interfaces.len()];
        for (index, link) in plan.links.iter().enumerate() {
            let network = &networks[index];
            for &port in &link.interfaces {
                let node = plan.interfaces[port].node;
                if first_links[node] != index {
                    self.checked(strings(&["network", "connect", network, &containers[node]]))?;
                }
            }
            let inspected = self.checked(strings(&["network", "inspect", network]))?;
            let inspected = json(&inspected.stdout)?;
            for &port in &link.interfaces {
                let interface = &plan.interfaces[port];
                let container = &containers[interface.node];
                let links = self.exec(container, &["ip", "-j", "link"])?;
                let (mac, name) = endpoint_interface(&inspected, container, &json(&links.stdout)?)?;
                self.observation.interfaces.push(InterfaceObservation {
                    port_id: interface.id.clone(),
                    container: container.clone(),
                    network: network.clone(),
                    endpoint_mac: mac,
                    native_mac: interface.mac.clone(),
                    ifname: name.clone(),
                });
                self.exec(container, &["ip", "-4", "address", "flush", "dev", &name])?;
                self.exec(container, &["ip", "-6", "address", "flush", "dev", &name])?;
                self.exec(
                    container,
                    &["ip", "link", "set", "dev", &name, "address", &interface.mac],
                )?;
                self.exec(
                    container,
                    &[
                        "ip",
                        "link",
                        "set",
                        "dev",
                        &name,
                        "mtu",
                        &interface.ipv4_mtu_bytes.to_string(),
                    ],
                )?;
                self.exec(
                    container,
                    &[
                        "ip",
                        "address",
                        "add",
                        &format!("{}/{}", interface.ipv4, interface.prefix_length),
                        "dev",
                        &name,
                    ],
                )?;
                self.exec(container, &["ip", "link", "set", "dev", &name, "up"])?;
                names[port] = name;
            }
        }
        for (node, device) in plan.nodes.iter().enumerate() {
            if let Some(gateway) = device.gateway {
                let port = plan
                    .interfaces
                    .iter()
                    .position(|p| p.node == node)
                    .expect("compiled host port");
                self.exec(
                    &containers[node],
                    &static_route_args("default", &gateway.to_string(), &names[port]),
                )?;
            }
        }
        for route in &plan.routes {
            self.exec(
                &containers[route.node],
                &static_route_args(
                    &format!("{}/{}", route.network, route.prefix_length),
                    &route.next_hop.to_string(),
                    &names[route.egress],
                ),
            )?;
        }
        let run_deadline = self.deadline;
        self.deadline = run_deadline.min(Instant::now() + self.options.route_ready_timeout);
        // Route replacement is synchronous; verify the acknowledged route once.
        for (node, container) in containers.iter().enumerate() {
            let snapshot = self.snapshot(plan, node, container)?;
            let observed = &self.observation.routes[snapshot];
            for route in plan.routes.iter().filter(|r| r.node == node) {
                verify_static_route(
                    observed,
                    &format!("{}/{}", route.network, route.prefix_length),
                    &plan.interfaces[route.egress].id,
                    route.next_hop,
                )?;
            }
            if let Some(gateway) = plan.nodes[node].gateway {
                let port = plan
                    .interfaces
                    .iter()
                    .find(|port| port.node == node)
                    .expect("compiled host port");
                verify_static_route(observed, "0.0.0.0/0", &port.id, gateway)?;
            }
        }
        self.deadline = run_deadline;
        Ok(containers)
    }

    fn probe(&mut self, plan: &RealStackPlan, containers: &[String]) -> Result<(), RealStackError> {
        for probe in &plan.probes {
            let mut snapshots = Vec::new();
            for (node, container) in containers.iter().enumerate() {
                snapshots.push(self.snapshot(plan, node, container)?);
            }
            let source = &plan.interfaces[probe.source];
            let destination = &plan.interfaces[probe.destination];
            let mut kernel_lookups = Vec::new();
            for (direction, from, to) in [
                ("request", source, destination),
                ("reply", destination, source),
            ] {
                for (node, container) in containers.iter().enumerate() {
                    if node != from.node && !plan.nodes[node].router {
                        continue;
                    }
                    let mut args = strings(&[
                        "exec",
                        "--env",
                        "LC_ALL=C",
                        container,
                        "ip",
                        "-j",
                        "-4",
                        "route",
                        "get",
                        &to.ipv4.to_string(),
                    ]);
                    if node == from.node {
                        args.extend(strings(&["from", &from.ipv4.to_string()]));
                    }
                    let lookup = self.call(args)?;
                    let mappings = self
                        .observation
                        .interfaces
                        .iter()
                        .filter(|m| m.container == *container)
                        .map(|m| (m.ifname.clone(), m.port_id.clone()))
                        .collect();
                    let selected = parse_lookup(&lookup, &mappings)?;
                    kernel_lookups.push(KernelLookupObservation {
                        node_id: plan.nodes[node].id.clone(),
                        direction: direction.into(),
                        destination: to.ipv4,
                        observed_ms: lookup.finished_ms,
                        selected,
                        stdout: lookup.stdout,
                        stderr: lookup.stderr,
                    });
                }
            }
            let ping = self.exec(
                &containers[source.node],
                &[
                    "python3",
                    "-c",
                    FIXED_DF_ECHO,
                    &source.ipv4.to_string(),
                    &destination.ipv4.to_string(),
                    &probe.ttl.to_string(),
                    &probe.payload_bytes.to_string(),
                    &self.options.ping_timeout.as_secs().to_string(),
                ],
            )?;
            let source_has_route = kernel_lookups.iter().any(|l| {
                l.direction == "request"
                    && l.node_id == plan.nodes[source.node].id
                    && l.selected.is_some()
            });
            let echo = parse_echo(
                &ping.stdout,
                source.ipv4,
                destination.ipv4,
                probe.payload_bytes,
                source_has_route,
            )?;
            self.observation.probes.push(ProbeObservation {
                id: probe.id.clone(),
                source: source.ipv4,
                destination: destination.ipv4,
                payload_bytes: echo.payload_bytes,
                ipv4_dont_fragment: echo.ipv4_dont_fragment,
                engine: echo.engine,
                outcome: echo.outcome,
                reply_ttl: echo.reply_ttl,
                reply_ipv4_dont_fragment: echo.reply_ipv4_dont_fragment,
                container_elapsed_ms: echo.elapsed_ms,
                started_ms: ping.started_ms,
                finished_ms: ping.finished_ms,
                reachable: echo.outcome == EchoOutcome::Reply,
                transmitted: Some(echo.transmitted),
                received: Some(echo.received),
                route_observations: snapshots,
                kernel_lookups,
                stdout: ping.stdout,
                stderr: ping.stderr,
            });
        }
        Ok(())
    }

    fn cleanup(&mut self) {
        self.deadline = Instant::now() + CLEANUP_BUDGET;
        let owner = self.observation.owner.clone();
        // Discover by our unique owner even when create timed out after succeeding.
        for (list, remove) in [
            (
                strings(&[
                    "ps",
                    "-aq",
                    "--filter",
                    &format!("label={OWNER_LABEL}={owner}"),
                ]),
                strings(&["rm", "--force"]),
            ),
            (
                strings(&[
                    "network",
                    "ls",
                    "-q",
                    "--filter",
                    &format!("label={OWNER_LABEL}={owner}"),
                ]),
                strings(&["network", "rm"]),
            ),
        ] {
            match self.checked(list.clone()) {
                Ok(result) => {
                    for id in result.stdout.split_whitespace() {
                        if !id.bytes().all(|b| b.is_ascii_hexdigit()) {
                            self.observation
                                .cleanup
                                .errors
                                .push("unexpected Docker resource ID".into());
                            continue;
                        }
                        let mut command = remove.clone();
                        command.push(id.into());
                        if let Err(e) = self.checked(command) {
                            self.observation.cleanup.errors.push(e.to_string());
                        }
                    }
                    match self.checked(list) {
                        Ok(result) if result.stdout.trim().is_empty() => {}
                        Ok(result) => self
                            .observation
                            .cleanup
                            .errors
                            .push(format!("owned resources remain: {}", result.stdout.trim())),
                        Err(e) => self.observation.cleanup.errors.push(e.to_string()),
                    }
                }
                Err(e) => self.observation.cleanup.errors.push(e.to_string()),
            }
        }
        self.observation.cleanup.complete = self.observation.cleanup.errors.is_empty();
    }
}

/// Uses only the fixed public official image already pulled by the caller.
/// Every runtime failure carries the observations and the cleanup outcome.
pub fn run_real_stack(
    plan: &RealStackPlan,
    options: RealStackOptions,
) -> Result<StackObservation, RealStackError> {
    run_with(plan, options, |runner, containers| {
        runner.probe(plan, containers)
    })
}

fn run_with(
    plan: &RealStackPlan,
    options: RealStackOptions,
    observe: impl FnOnce(&mut Runner, &[String]) -> Result<(), RealStackError>,
) -> Result<StackObservation, RealStackError> {
    options.validate()?;
    let unix_start_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| RealStackError::Io(e.to_string()))?
        .as_millis();
    let owner = format!(
        "reliner-rs-{}-{unix_start_ms:x}-{:x}",
        std::process::id(),
        NEXT_OWNER.fetch_add(1, Ordering::Relaxed)
    );
    let started = Instant::now();
    let mut runner = Runner { started, deadline: started + options.run_timeout, options,
        observation: StackObservation { native_id: plan.native_id.clone(), native_revision: plan.native_revision.clone(), native_format_version: plan.native_format_version, effective_plan: plan.clone(), route_configuration_source: LINUX_KERNEL_STATIC, owner,
            image: ImageObservation { reference: REAL_STACK_IMAGE.into(), ..Default::default() },
            clock: ObservationClock { kind: "independent_host_monotonic_ms_with_unix_origin".into(), unix_start_ms, elapsed_ms: 0 },
            comparison_scope: "steady unfragmented IPv4 reachability and kernel next hops only; requests enforce DF, Linux reply DF is recorded independently; simulation ns, link delay, scheduling, cache lifetimes and wall RTT are not compared".into(),
            interfaces: Vec::new(), routes: Vec::new(), probes: Vec::new(), commands: Vec::new(), cleanup: CleanupObservation::default() } };
    let result = runner
        .setup(plan)
        .and_then(|containers| observe(&mut runner, &containers));
    runner.cleanup();
    runner.observation.clock.elapsed_ms = runner.started.elapsed().as_millis();
    let cause = result.err().or_else(|| {
        (!runner.observation.cleanup.complete)
            .then(|| RealStackError::Io("owned Docker resource cleanup failed".into()))
    });
    match cause {
        Some(cause) => Err(RealStackError::RunFailed {
            cause: Box::new(cause),
            observation: Box::new(runner.observation),
        }),
        None => Ok(runner.observation),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "Docker IPv4 Linux static routing; deletes the return route in owned containers only"]
    fn docker_linux_static_route_delete_changes_fib_and_echo_results() {
        let mut doc: Value =
            serde_json::from_str(include_str!("../examples/ipv4-lab.json")).unwrap();
        doc["scenario"]["warmup"] = serde_json::json!([]);
        doc["scenario"]["probes"][1]["source"] = serde_json::json!("pc-b");
        doc["scenario"]["probes"][1]["destination"] = serde_json::json!("pc-a");
        let plan = plan_real_stack(&doc.to_string()).unwrap();
        let daemon_free = |command: &CommandObservation| {
            !command.argv.iter().any(|arg| {
                matches!(
                    arg.as_str(),
                    "vtysh"
                        | "/usr/lib/frr/staticd"
                        | "/usr/lib/frr/mgmtd"
                        | "/usr/lib/frr/frrinit.sh"
                )
            }) && (!command.argv.iter().any(|arg| arg == "/usr/lib/frr/zebra")
                || command.argv.last().is_some_and(|arg| arg == "--version"))
        };
        let observation = run_with(&plan, RealStackOptions::default(), |runner, containers| {
            runner.probe(&plan, containers)?;
            let route = plan
                .routes
                .iter()
                .find(|route| plan.nodes[route.node].id == "r2")
                .unwrap();
            let port = &plan.interfaces[route.egress];
            let dev = runner
                .observation
                .interfaces
                .iter()
                .find(|iface| iface.port_id == port.id)
                .unwrap()
                .ifname
                .clone();
            runner.exec(
                &containers[route.node],
                &[
                    "ip",
                    "-4",
                    "route",
                    "del",
                    &format!("{}/{}", route.network, route.prefix_length),
                    "via",
                    &route.next_hop.to_string(),
                    "dev",
                    &dev,
                    "proto",
                    "static",
                ],
            )?;
            runner.probe(&plan, containers)
        })
        .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(observation.route_configuration_source, LINUX_KERNEL_STATIC);
        assert!(observation.cleanup.complete);
        assert!(observation.commands.iter().all(daemon_free));
        for (index, router, direction, gateway) in [
            (0, "r1", "request", "198.51.100.2"),
            (1, "r2", "request", "198.51.100.1"),
        ] {
            let probe = &observation.probes[index];
            assert_eq!(probe.outcome, EchoOutcome::Reply, "{probe:?}");
            let fib = probe
                .kernel_lookups
                .iter()
                .find(|item| item.node_id == router && item.direction == direction)
                .unwrap();
            let selected = fib.selected.as_ref().unwrap();
            assert_eq!(selected.gateway, Some(gateway.parse().unwrap()));
            println!(
                "before: {} {router} {} via {gateway}",
                probe.id, selected.egress_port
            );
        }
        for (probe, direction) in [
            (&observation.probes[2], "reply"),
            (&observation.probes[3], "request"),
        ] {
            assert!(!probe.reachable, "{probe:?}");
            assert_eq!((probe.transmitted, probe.received), (Some(1), Some(0)));
            let fib = probe
                .kernel_lookups
                .iter()
                .find(|item| item.node_id == "r2" && item.direction == direction)
                .unwrap();
            assert!(fib.selected.is_none(), "{fib:?}");
            let r2 = probe
                .route_observations
                .iter()
                .map(|&i| &observation.routes[i])
                .find(|table| table.node_id == "r2")
                .unwrap();
            assert!(!r2.routes.iter().any(|route| route.prefix == "192.0.2.0/24"));
            println!(
                "after: {} {:?}; r2 {direction} no route; {}",
                probe.id,
                probe.outcome,
                probe.stdout.trim()
            );
        }
    }

    #[test]
    fn commands_use_two_caps_internal_network_and_generated_identifiers() {
        let args = container_args(
            "test-owner",
            "test-owner-n0",
            "sha256:test",
            true,
            "test-owner-l0",
        );
        assert_eq!(args.iter().filter(|s| s.as_str() == "--cap-add").count(), 2);
        assert!(args.windows(2).any(|s| s == ["--cap-drop", "ALL"]));
        for forbidden in [
            "--privileged",
            "--volume",
            "--mount",
            "--publish",
            "SYS_ADMIN",
            "host",
        ] {
            assert!(!args.iter().any(|s| s == forbidden));
        }
        let args = network_args("test-owner", "test-owner-l0");
        assert!(args.iter().any(|s| s == "--internal"));
        assert!(args.iter().any(|s| s == "--ipv4=false"));
        assert_eq!(
            static_route_args("203.0.113.0/24", "198.51.100.2", "eth3"),
            [
                "ip",
                "-4",
                "route",
                "replace",
                "203.0.113.0/24",
                "via",
                "198.51.100.2",
                "dev",
                "eth3",
                "proto",
                "static"
            ]
        );
    }

    #[test]
    fn interface_mapping_uses_mac_not_array_order_or_eth_number() {
        let network = serde_json::json!([{"Containers":{"opaque":{"Name":"generated-n0","MacAddress":"02:00:00:00:00:02"}}}]);
        let links = serde_json::json!([{"ifname":"eth7","address":"02:00:00:00:00:01"},{"ifname":"eth3","address":"02:00:00:00:00:02"}]);
        assert_eq!(
            endpoint_interface(&network, "generated-n0", &links)
                .unwrap()
                .1,
            "eth3"
        );
        assert!(endpoint_interface(&network, "other-owner", &links).is_err());
        let links = serde_json::json!([{"ifname":"eth0\nquit","address":"02:00:00:00:00:02"}]);
        assert!(endpoint_interface(&network, "generated-n0", &links).is_err());
    }

    #[test]
    fn raw_echo_requires_matching_payload_df_identity_and_observed_counters() {
        let source = "192.0.2.1".parse().unwrap();
        let destination = "192.0.2.2".parse().unwrap();
        let mut value = serde_json::json!({
            "engine":"python3_linux_raw_icmp_df", "source":"192.0.2.1", "destination":"192.0.2.2",
            "payload_bytes":56, "ipv4_dont_fragment":true, "transmitted":1, "received":1,
            "outcome":"reply", "reply_ttl":64, "reply_ipv4_dont_fragment":false,
            "send_errno":null, "icmp_error_type":null, "icmp_error_code":null,
            "clock":"container_monotonic_elapsed_ms", "elapsed_ms":0.2
        });
        let parsed = parse_echo(&value.to_string(), source, destination, 56, true).unwrap();
        assert_eq!(parsed.outcome, EchoOutcome::Reply);
        assert_eq!(parsed.reply_ipv4_dont_fragment, Some(false));
        assert!(parse_echo(&value.to_string(), source, destination, 128, true).is_err());
        value["ipv4_dont_fragment"] = serde_json::json!(false);
        assert!(parse_echo(&value.to_string(), source, destination, 56, true).is_err());
        value["ipv4_dont_fragment"] = serde_json::json!(true);
        value["received"] = serde_json::json!(0);
        assert!(parse_echo(&value.to_string(), source, destination, 56, true).is_err());
        value["outcome"] = serde_json::json!("timeout");
        value["reply_ttl"] = serde_json::Value::Null;
        value["reply_ipv4_dont_fragment"] = serde_json::Value::Null;
        assert_eq!(
            parse_echo(&value.to_string(), source, destination, 56, true)
                .unwrap()
                .outcome,
            EchoOutcome::Timeout
        );
        value["outcome"] = serde_json::json!("no_route");
        value["transmitted"] = serde_json::json!(0);
        value["send_errno"] = serde_json::json!(101);
        assert_eq!(
            parse_echo(&value.to_string(), source, destination, 56, false)
                .unwrap()
                .outcome,
            EchoOutcome::NoRoute
        );
        assert!(parse_echo(&value.to_string(), source, destination, 56, true).is_err());
    }
    #[test]
    fn kernel_routes_retain_actual_next_hops_and_reject_unknown_egress() {
        let mapping = BTreeMap::from([("eth3".into(), "r1-transit".into())]);
        let raw = r#"[{"dst":"203.0.113.0/24","gateway":"198.51.100.2","dev":"eth3","protocol":"static"}]"#;
        let routes = parse_routes(raw, &mapping).unwrap();
        assert_eq!(routes[0].gateway, Some("198.51.100.2".parse().unwrap()));
        assert_eq!(routes[0].egress_port, "r1-transit");
        assert_eq!(routes[0].protocol.as_deref(), Some("static"));
        assert!(parse_routes(raw, &BTreeMap::new()).is_err());
        assert!(parse_routes(r#"[{"dst":"default","nexthops":[]}]"#, &mapping).is_err());
    }

    #[test]
    fn route_lookup_distinguishes_missing_route_from_execution_failure() {
        let mut output = CommandObservation {
            argv: strings(&[
                "exec",
                "generated-n0",
                "ip",
                "-j",
                "-4",
                "route",
                "get",
                "203.0.113.2",
            ]),
            started_ms: 1,
            finished_ms: 2,
            exit_code: Some(2),
            timed_out: false,
            stdout: String::new(),
            stderr: "RTNETLINK answers: Network unreachable\n".into(),
        };
        assert!(parse_lookup(&output, &BTreeMap::new()).unwrap().is_none());
        for (exit, stdout, stderr) in [
            (2, "", "RTNETLINK answers: Operation not permitted\n"),
            (
                2,
                "",
                "RTNETLINK answers: Network unreachable\nother error\n",
            ),
            (2, "", "RTNETLINK answers: Network is unreachable\n"),
            (2, "[]\n", "RTNETLINK answers: Network unreachable\n"),
            (1, "", "RTNETLINK answers: Network unreachable\n"),
        ] {
            output.exit_code = Some(exit);
            output.stdout = stdout.into();
            output.stderr = stderr.into();
            assert!(
                matches!(
                    parse_lookup(&output, &BTreeMap::new()),
                    Err(RealStackError::CommandFailed { .. })
                ),
                "{output:?}"
            );
        }
        output.exit_code = Some(0);
        output.stderr.clear();
        output.stdout = r#"[{"dst":"203.0.113.2","gateway":"198.51.100.2","dev":"eth3"}]"#.into();
        let mapping = BTreeMap::from([("eth3".into(), "transit".into())]);
        assert_eq!(
            parse_lookup(&output, &mapping).unwrap().unwrap().gateway,
            Some("198.51.100.2".parse().unwrap())
        );
    }
}
