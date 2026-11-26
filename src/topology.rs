use std::collections::{HashMap, HashSet};
use std::net::Ipv4Addr;
use std::num::{NonZeroU32, NonZeroU64, NonZeroUsize};

use serde::{Deserialize, Deserializer, de};

use crate::dynamic::{LinkStateSpec, RoutingPlan};
use crate::stp::StpPlan;
use crate::{
    Endpoint, InputError, Ipv4AclAction, Ipv4AclMatch, LinkStatus, PortReference, PortStage, Probe,
};

#[derive(Deserialize)]
struct Header {
    format_version: u32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    format_version: u32,
    id: String,
    revision: String,
    devices: Vec<DeviceSpec>,
    links: Vec<LinkSpec>,
    probe: Option<LegacyProbeSpec>,
    scenario: Option<ScenarioSpec>,
    #[serde(default, deserialize_with = "provided")]
    import_source: Option<crate::ImportSource>,
    #[serde(rename = "presentation")]
    _presentation: Option<de::IgnoredAny>,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum DeviceSpec {
    Host {
        id: String,
        name: String,
        ipv4: Ipv4Addr,
        prefix_length: u8,
        mac: String,
        port: HostPortSpec,
        gateway: Option<Ipv4Addr>,
    },
    Switch {
        id: String,
        name: String,
        #[serde(default, deserialize_with = "provided")]
        stp_priority: Option<u16>,
        ports: Vec<SwitchPortSpec>,
    },
    Router {
        id: String,
        name: String,
        ports: Vec<IpPortSpec>,
        routes: Vec<RouteSpec>,
        #[serde(default, deserialize_with = "provided")]
        link_state: Option<LinkStateSpec>,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HostPortSpec {
    id: String,
    name: String,
    #[serde(default, deserialize_with = "provided")]
    ipv4_mtu_bytes: Option<u16>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SwitchPortSpec {
    id: String,
    name: String,
    mode: SwitchMode,
    #[serde(default, deserialize_with = "provided")]
    stp_cost: Option<NonZeroU32>,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum SwitchMode {
    Access {
        vlan: u16,
    },
    Trunk {
        allowed_vlans: Vec<u16>,
        #[serde(default, deserialize_with = "provided")]
        native_vlan: Option<u16>,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct IpPortSpec {
    id: String,
    name: String,
    ipv4: Ipv4Addr,
    prefix_length: u8,
    mac: String,
    #[serde(default, deserialize_with = "provided")]
    ipv4_mtu_bytes: Option<u16>,
    #[serde(default, deserialize_with = "provided")]
    ipv4_acl: Option<Box<Ipv4Acl>>,
}

pub(crate) const MAX_ACL_RULES: usize = 64;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Ipv4Acl {
    #[serde(default, deserialize_with = "provided")]
    ingress: Option<Vec<AclRule>>,
    #[serde(default, deserialize_with = "provided")]
    egress: Option<Vec<AclRule>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AclRule {
    action: Ipv4AclAction,
    network: Ipv4Addr,
    prefix_length: u8,
}

impl Ipv4Acl {
    fn validate(&self, port: &str) -> Result<(), InputError> {
        let error = |msg: &str, field: &str| {
            InputError::new(format!("接口 {port} 的 IPv4 ACL {msg}。"))
                .code("invalid_ipv4_acl")
                .port(port, field)
        };
        if self.ingress.is_none() && self.egress.is_none() {
            return Err(error("至少需要一个方向", "ipv4_acl"));
        }
        for (direction, rules) in [("ingress", &self.ingress), ("egress", &self.egress)] {
            let Some(rules) = rules else { continue };
            if rules.len() > MAX_ACL_RULES {
                return Err(error(
                    &format!("每个方向最多 {MAX_ACL_RULES} 条规则"),
                    &format!("ipv4_acl/{direction}"),
                ));
            }
            for (index, rule) in rules.iter().enumerate() {
                let field = if rule.prefix_length > 32 {
                    "prefix_length"
                } else if u32::from(rule.network) & !netmask(rule.prefix_length) != 0 {
                    "network"
                } else {
                    continue;
                };
                return Err(error(
                    "源前缀须为规范 IPv4 网络地址，前缀长度为 0 至 32",
                    &format!("ipv4_acl/{direction}/{index}/{field}"),
                ));
            }
        }
        Ok(())
    }

    fn rules(&self, direction: PortStage) -> Option<&[AclRule]> {
        match direction {
            PortStage::Ingress => self.ingress.as_deref(),
            PortStage::Egress => self.egress.as_deref(),
        }
    }

    pub(crate) fn decide(
        &self,
        direction: PortStage,
        source: Ipv4Addr,
    ) -> Option<(Ipv4AclAction, Option<Ipv4AclMatch>)> {
        for (index, rule) in self.rules(direction)?.iter().enumerate() {
            if u32::from(source) & netmask(rule.prefix_length) == u32::from(rule.network) {
                let matched = Ipv4AclMatch {
                    rule_number: index + 1,
                    network: rule.network,
                    prefix_length: rule.prefix_length,
                };
                return Some((rule.action, Some(matched)));
            }
        }
        Some((Ipv4AclAction::Deny, None))
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RouteSpec {
    network: Ipv4Addr,
    prefix_length: u8,
    egress: String,
    next_hop: Ipv4Addr,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LinkSpec {
    id: String,
    ports: [String; 2],
    #[serde(deserialize_with = "nanoseconds")]
    delay_ns: u64,
    standby_for: Option<String>,
    #[serde(default, deserialize_with = "provided")]
    transmission: Option<Transmission>,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Transmission {
    #[serde(deserialize_with = "rate_bps")]
    pub(crate) rate_bps: NonZeroU64,
    pub(crate) queue_frames: u16,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyProbeSpec {
    source: String,
    destination: String,
    #[serde(deserialize_with = "nanoseconds")]
    deadline_ns: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ScenarioSpec {
    #[serde(deserialize_with = "nanoseconds")]
    end_ns: u64,
    #[serde(deserialize_with = "nanoseconds")]
    arp_lifetime_ns: u64,
    #[serde(deserialize_with = "nanoseconds")]
    mac_lifetime_ns: u64,
    #[serde(default)]
    trace_limit: usize,
    #[serde(default, deserialize_with = "optional_nanoseconds")]
    routing_convergence_ns: Option<u64>,
    #[serde(default, deserialize_with = "optional_nanoseconds")]
    stp_convergence_ns: Option<u64>,
    #[serde(default)]
    link_actions: Vec<LinkActionSpec>,
    #[serde(default)]
    warmup: Vec<WarmupSpec>,
    probes: Vec<TimedProbeSpec>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LinkActionSpec {
    link: String,
    state: LinkStatus,
    #[serde(deserialize_with = "nanoseconds")]
    at_ns: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WarmupSpec {
    port: String,
    neighbor: Ipv4Addr,
    #[serde(deserialize_with = "nanoseconds")]
    at_ns: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TimedProbeSpec {
    id: String,
    source: String,
    destination: String,
    #[serde(deserialize_with = "nanoseconds")]
    at_ns: u64,
    #[serde(deserialize_with = "nanoseconds")]
    deadline_ns: u64,
    #[serde(default = "default_ttl")]
    ttl: u8,
    #[serde(default, deserialize_with = "provided")]
    payload_bytes: Option<u16>,
}

pub(crate) const DEFAULT_TTL: u8 = 64;
pub(crate) const ECHO_HEADERS_BYTES: u16 = 20 + 8;
const DEFAULT_ECHO_PAYLOAD_BYTES: u16 = 56;
const MAX_ECHO_PAYLOAD_BYTES: u16 = u16::MAX - ECHO_HEADERS_BYTES;
const MIN_IPV4_MTU_BYTES: u16 = 68;

fn provided<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
    deserializer: D,
) -> Result<Option<T>, D::Error> {
    T::deserialize(deserializer).map(Some)
}

fn rate_bps<'de, D: Deserializer<'de>>(deserializer: D) -> Result<NonZeroU64, D::Error> {
    let raw = String::deserialize(deserializer)?;
    if raw.is_empty() || !raw.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(de::Error::custom("链路速率必须是大于 0 的十进制整数字符串"));
    }
    raw.parse().map_err(de::Error::custom)
}

fn default_ttl() -> u8 {
    DEFAULT_TTL
}

fn optional_nanoseconds<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<u64>, D::Error> {
    nanoseconds(deserializer).map(Some)
}

fn nanoseconds<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u64, D::Error> {
    let raw = String::deserialize(deserializer)?;
    if raw.is_empty() || !raw.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(de::Error::custom("纳秒时间必须是十进制整数字符串"));
    }
    raw.parse().map_err(de::Error::custom)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Mac([u8; 6]);

impl Mac {
    pub(crate) const BROADCAST: Self = Self([0xff; 6]);

    pub(crate) fn text(self) -> String {
        self.0
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<Vec<_>>()
            .join(":")
    }

    fn parse(raw: &str) -> Option<Self> {
        let mut parts = raw.split(':');
        let mut bytes = [0; 6];
        for byte in &mut bytes {
            let part = parts.next()?;
            if part.len() != 2 || !part.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return None;
            }
            *byte = u8::from_str_radix(part, 16).ok()?;
        }
        (parts.next().is_none() && bytes[0] & 1 == 0 && bytes != [0; 6]).then_some(Self(bytes))
    }
}

pub(crate) type DeviceIndex = usize;
pub(crate) type PortIndex = usize;
pub(crate) type LinkIndex = usize;
pub(crate) type ProbeIndex = usize;

#[derive(Clone, Copy)]
pub(crate) struct Interface {
    pub(crate) ipv4: Ipv4Addr,
    pub(crate) mac: Mac,
    pub(crate) ipv4_mtu_bytes: Option<u16>,
    prefix_length: u8,
}

impl Interface {
    pub(crate) fn connected_route(self, egress: PortIndex) -> Route {
        Route {
            network: Ipv4Addr::from(u32::from(self.ipv4) & netmask(self.prefix_length)),
            length: self.prefix_length,
            egress,
            next_hop: None,
        }
    }

    pub(crate) fn on_link(self, ip: Ipv4Addr) -> bool {
        let mask = netmask(self.prefix_length);
        (u32::from(self.ipv4) & mask) == (u32::from(ip) & mask)
    }

    pub(crate) fn can_resolve(self, ip: Ipv4Addr) -> bool {
        self.ipv4 != ip && self.on_link(ip) && usable_ipv4(ip, self.prefix_length)
    }
}

pub(crate) enum DeviceKind {
    Host { gateway: Option<Ipv4Addr> },
    Switch,
    Router { routes: Vec<Route> },
}

pub(crate) struct Device {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) kind: DeviceKind,
    pub(crate) ports: Vec<PortIndex>,
    pub(crate) flood_work: usize,
    pub(crate) max_egress_acl_work: usize,
}

pub(crate) enum PortKind {
    Switch(SwitchMode),
    Ip(Interface),
}

pub(crate) struct Port {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) device: DeviceIndex,
    pub(crate) kind: PortKind,
    pub(crate) ipv4_acl: Option<Box<Ipv4Acl>>,
    pub(crate) link: Option<Attachment>,
}

impl Port {
    pub(crate) fn acl_work(&self, direction: PortStage) -> usize {
        self.ipv4_acl
            .as_ref()
            .and_then(|acl| acl.rules(direction))
            .map_or(0, |rules| 1 + rules.len())
    }

    pub(crate) fn filter_work(&self) -> usize {
        2 + self.vlan_work()
    }

    pub(crate) fn vlan_work(&self) -> usize {
        match &self.kind {
            PortKind::Switch(SwitchMode::Trunk { allowed_vlans, .. }) => allowed_vlans.len(),
            _ => 0,
        }
    }

    pub(crate) fn interface(&self) -> Option<Interface> {
        match self.kind {
            PortKind::Ip(interface) => Some(interface),
            PortKind::Switch(_) => None,
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Attachment {
    pub(crate) link: LinkIndex,
    pub(crate) peer: PortIndex,
}

pub(crate) struct Link {
    pub(crate) id: String,
    pub(crate) delay_ns: u64,
    pub(crate) transmission: Option<Transmission>,
    pub(crate) protection: Option<usize>,
    pub(crate) ports: [PortIndex; 2],
}

#[derive(Clone, Copy)]
pub(crate) struct LinkProtection {
    pub(crate) primary: LinkIndex,
    pub(crate) standby: LinkIndex,
}

#[derive(Clone, Copy)]
pub(crate) struct Route {
    pub(crate) network: Ipv4Addr,
    pub(crate) length: u8,
    pub(crate) egress: PortIndex,
    pub(crate) next_hop: Option<Ipv4Addr>,
}

impl Route {
    pub(crate) fn matches(self, ip: Ipv4Addr) -> bool {
        (u32::from(ip) & netmask(self.length)) == u32::from(self.network)
    }
}

#[derive(Default)]
struct Vertex {
    component: usize,
    entry: usize,
    end: usize,
}

#[derive(Clone, Copy)]
pub(crate) struct ProbeConfig {
    pub(crate) id: ProbeIndex,
    pub(crate) source: PortIndex,
    pub(crate) destination: PortIndex,
    pub(crate) at_ns: u64,
    pub(crate) deadline_ns: u64,
    pub(crate) ttl: u8,
    pub(crate) payload_bytes: u16,
    // Destinations of the Echo request and reply, in that order.
    pub(crate) advertised_destinations: [bool; 2],
}

#[derive(Clone, Copy)]
pub(crate) struct LinkUpdate {
    pub(crate) link: LinkIndex,
    pub(crate) state: LinkStatus,
}

#[derive(Clone, Copy)]
pub(crate) struct LinkChange {
    pub(crate) at_ns: u64,
    pub(crate) first: LinkUpdate,
    pub(crate) second: Option<LinkUpdate>,
}

impl LinkChange {
    pub(crate) fn updates(self) -> impl Iterator<Item = LinkUpdate> {
        std::iter::once(self.first).chain(self.second)
    }

    pub(crate) fn update_count(self) -> usize {
        1 + usize::from(self.second.is_some())
    }
}

pub(crate) struct Warmup {
    pub(crate) port: PortIndex,
    pub(crate) neighbor: Ipv4Addr,
    pub(crate) at_ns: u64,
}

pub(crate) struct ForwardingForest {
    vertices: Vec<Vertex>,
    children: Vec<Option<usize>>,
}

impl ForwardingForest {
    pub(crate) fn entries(&self) -> usize {
        self.vertices.len() + self.children.len()
    }

    fn on_required_branch(
        &self,
        link: usize,
        [source, target, from]: [usize; 3],
        stage: PortStage,
    ) -> bool {
        let Some(child) = self.children[link] else {
            return false;
        };
        let source = &self.vertices[source];
        let target = &self.vertices[target];
        let child = &self.vertices[child];
        if source.component != child.component || target.component != child.component {
            return false;
        }
        let inside = |vertex: &Vertex| (child.entry..child.end).contains(&vertex.entry);
        inside(source) != inside(target)
            && (inside(source) == inside(&self.vertices[from])) == (stage == PortStage::Egress)
    }
}

pub(crate) struct StpForestPlan {
    adjacency: Vec<Vec<(usize, usize)>>,
    managed_slots: Vec<Option<usize>>,
    port_slots: Vec<Option<NonZeroUsize>>,
    link_slots: Vec<Option<NonZeroUsize>>,
}

impl StpForestPlan {
    pub(crate) fn compile(
        device_count: usize,
        ports: &[Port],
        links: &[Link],
        managed_slot: impl Fn(LinkIndex) -> Option<usize>,
    ) -> Self {
        let mut controlled_switches = vec![false; device_count];
        for (index, link) in links.iter().enumerate() {
            if managed_slot(index).is_some() {
                for port in link.ports {
                    controlled_switches[ports[port].device] = true;
                }
            }
        }
        let mut switches = vec![None; device_count];
        let mut port_slots = vec![None; ports.len()];
        let mut link_slots = vec![None; links.len()];
        let mut edges = Vec::new();
        let mut managed_slots = Vec::new();
        let mut vertices = 0;
        let mut add_vertex = || {
            vertices += 1;
            NonZeroUsize::new(vertices).expect("nonzero vertex slot")
        };
        for (index, link) in links.iter().enumerate() {
            if !link
                .ports
                .iter()
                .any(|&port| controlled_switches[ports[port].device])
            {
                continue;
            }
            let endpoints = link.ports.map(|port| {
                let slot = match ports[port].kind {
                    PortKind::Switch(_) => {
                        *switches[ports[port].device].get_or_insert_with(&mut add_vertex)
                    }
                    PortKind::Ip(_) => add_vertex(),
                };
                port_slots[port] = Some(slot);
                slot.get() - 1
            });
            edges.push(endpoints);
            managed_slots.push(managed_slot(index));
            link_slots[index] = NonZeroUsize::new(edges.len());
        }
        let adjacency = forest_adjacency(vertices, edges.into_iter().enumerate());
        Self {
            adjacency,
            managed_slots,
            port_slots,
            link_slots,
        }
    }

    pub(crate) fn work(&self) -> usize {
        // Vertex initialization and traversal, plus child slots and both edge directions.
        4 * self.adjacency.len() + 3 * self.managed_slots.len()
    }

    pub(crate) fn build(&self, forwarding: &[bool]) -> ForwardingForest {
        index_forest(&self.adjacency, self.managed_slots.len(), |slot| {
            self.managed_slots[slot].is_none_or(|index| forwarding[index])
        })
        .expect("the elected forwarding links form a forest")
    }

    fn slot(&self, link: LinkIndex) -> Option<usize> {
        self.link_slots[link].map(|slot| slot.get() - 1)
    }

    fn on_required_branch(
        &self,
        forest: &ForwardingForest,
        link_slot: usize,
        ports: [PortIndex; 3],
        stage: PortStage,
    ) -> bool {
        let [Some(source), Some(target), Some(from)] = ports.map(|port| self.port_slots[port])
        else {
            return false;
        };
        forest.on_required_branch(
            link_slot,
            [source.get() - 1, target.get() - 1, from.get() - 1],
            stage,
        )
    }
}

pub(crate) struct Topology {
    pub(crate) format_version: u32,
    pub(crate) id: String,
    pub(crate) revision: String,
    pub(crate) legacy: bool,
    pub(crate) mtu_model: bool,
    pub(crate) native_vlan_count: usize,
    pub(crate) devices: Vec<Device>,
    pub(crate) ports: Vec<Port>,
    pub(crate) links: Vec<Link>,
    pub(crate) protection: Vec<LinkProtection>,
    pub(crate) routing: Option<RoutingPlan>,
    pub(crate) stp: Option<StpPlan>,
    pub(crate) probes: Vec<ProbeConfig>,
    pub(crate) probe_names: Vec<String>,
    pub(crate) link_actions: Vec<LinkChange>,
    pub(crate) warmup: Vec<Warmup>,
    pub(crate) end_ns: u64,
    pub(crate) arp_lifetime_ns: Option<u64>,
    pub(crate) mac_lifetime_ns: Option<u64>,
    pub(crate) trace_limit: usize,
    pub(crate) addresses: HashMap<Ipv4Addr, PortIndex>,
    pub(crate) forest: ForwardingForest,
    pub(crate) import_source: Option<crate::ImportSource>,
    pub(crate) coverage: Option<crate::import_source::CoveragePlan>,
}

impl Topology {
    pub(crate) fn has_ipv4_acls(&self) -> bool {
        self.ports.iter().any(|p| p.ipv4_acl.is_some())
    }

    pub(crate) fn from_json(input: &str, legacy_only: bool) -> Result<Self, InputError> {
        let input = input.strip_prefix('\u{feff}').unwrap_or(input);
        let header: Header =
            crate::validation::decode(input).map_err(|error| error.locate(input))?;
        if header.format_version != 1 && (legacy_only || !matches!(header.format_version, 2 | 3)) {
            return Err(InputError::new(format!(
                "暂不支持文件版本 format_version {}。",
                header.format_version
            ))
            .code("unsupported_version")
            .at("/format_version"));
        }
        let document = crate::validation::decode(input).map_err(|error| error.locate(input))?;
        Self::compile(document).map_err(|error| error.locate(input))
    }

    fn compile(document: Document) -> Result<Self, InputError> {
        let Document {
            format_version,
            id,
            revision,
            devices: specs,
            links: link_specs,
            probe,
            scenario,
            import_source,
            ..
        } = document;
        let legacy = format_version == 1;
        if import_source.is_some() && format_version != 3 {
            return Err(InputError::new("导入缺项需要 format_version 3。".into())
                .code("unsupported_feature")
                .at("/import_source"));
        }
        require_name(&id, "拓扑 ID").map_err(|error| error.at("/id"))?;
        require_name(&revision, "拓扑修订").map_err(|error| error.at("/revision"))?;
        let mut devices = Vec::with_capacity(specs.len());
        let mut ports = Vec::new();
        let mut native_vlan_count = 0;
        let mut device_ids = HashMap::new();
        let mut port_ids = HashMap::new();
        let mut addresses = HashMap::new();
        let mut macs = HashSet::new();
        let mut link_state_specs = Vec::new();
        let mut stp_priorities = HashMap::new();
        let mut stp_costs = HashMap::new();
        for (device_index, spec) in specs.into_iter().enumerate() {
            let (device_id, name, mut kind, mut port_specs, static_routes) = match spec {
                DeviceSpec::Host {
                    id,
                    name,
                    ipv4,
                    prefix_length,
                    mac,
                    port,
                    gateway,
                } => {
                    if legacy && port.ipv4_mtu_bytes.is_some() {
                        return Err(InputError::new(
                            "IPv4 接口 MTU 需要 format_version 2。".into(),
                        )
                        .code("unsupported_feature")
                        .port(&port.id, "ipv4_mtu_bytes")
                        .at(format!("/devices/{device_index}/port/ipv4_mtu_bytes")));
                    }
                    let interface =
                        interface(&port.id, ipv4, prefix_length, &mac, port.ipv4_mtu_bytes)
                            .map_err(|error| {
                                let field = if error.location.path == "ipv4_mtu_bytes" {
                                    "port/ipv4_mtu_bytes"
                                } else {
                                    error.location.path.as_str()
                                };
                                let path = format!("/devices/{device_index}/{field}");
                                error.at(path)
                            })?;
                    if legacy && gateway.is_some() {
                        return Err(InputError::new("主机网关需要 format_version 2。".into())
                            .code("unsupported_feature")
                            .device(&id, "gateway")
                            .at(format!("/devices/{device_index}/gateway")));
                    }
                    if let Some(gateway) = gateway.filter(|&ip| !interface.can_resolve(ip)) {
                        return Err(InputError::new(format!(
                            "主机 {id} 的网关 {gateway} 不在可直连的地址范围。"
                        ))
                        .code("invalid_gateway")
                        .device(&id, "gateway")
                        .at(format!("/devices/{device_index}/gateway")));
                    }
                    (
                        id,
                        name,
                        DeviceKind::Host { gateway },
                        vec![(port.id, port.name, PortKind::Ip(interface), None, None)],
                        Vec::new(),
                    )
                }
                DeviceSpec::Switch {
                    id,
                    name,
                    ports,
                    stp_priority,
                } => {
                    if let Some(priority) = stp_priority {
                        if !priority.is_multiple_of(4096) {
                            return Err(InputError::new(format!(
                                "交换机 {id} 的 STP 优先级须为 4096 的倍数。"
                            ))
                            .code("invalid_stp_priority")
                            .device(&id, "stp_priority")
                            .at(format!("/devices/{device_index}/stp_priority")));
                        }
                        stp_priorities.insert(devices.len(), priority);
                    }
                    let port_specs = ports
                        .into_iter()
                        .enumerate()
                        .map(|(port_index, p)| {
                            let mode = validated_mode(&p.id, p.mode, legacy).map_err(|error| {
                                let path = format!(
                                    "/devices/{device_index}/ports/{port_index}/{}",
                                    error.location.path
                                );
                                error.at(path)
                            })?;
                            if let Some(cost) = p.stp_cost {
                                stp_costs.insert(p.id.clone(), cost.get());
                            }
                            Ok((p.id, p.name, PortKind::Switch(mode), Some(port_index), None))
                        })
                        .collect::<Result<_, InputError>>()?;
                    (id, name, DeviceKind::Switch, port_specs, Vec::new())
                }
                DeviceSpec::Router {
                    id,
                    name,
                    ports,
                    routes,
                    link_state,
                } => {
                    if legacy {
                        return Err(InputError::new("路由器需要 format_version 2。".into())
                            .code("unsupported_feature")
                            .device(&id, "kind")
                            .at(format!("/devices/{device_index}/kind")));
                    }
                    if ports.is_empty() {
                        return Err(InputError::new(format!("路由器 {id} 至少需要一个接口。"))
                            .code("missing_interface")
                            .device(&id, "ports")
                            .at(format!("/devices/{device_index}/ports")));
                    }
                    if let Some(spec) = link_state {
                        link_state_specs.push((devices.len(), spec));
                    }
                    let port_specs = ports
                        .into_iter()
                        .enumerate()
                        .map(|(port_index, p)| {
                            let interface =
                                interface(&p.id, p.ipv4, p.prefix_length, &p.mac, p.ipv4_mtu_bytes)
                                    .map_err(|error| {
                                        let path = format!(
                                            "/devices/{device_index}/ports/{port_index}/{}",
                                            error.location.path
                                        );
                                        error.at(path)
                                    })?;
                            if let Some(acl) = &p.ipv4_acl {
                                acl.validate(&p.id).map_err(|error| {
                                    let path = format!(
                                        "/devices/{device_index}/ports/{port_index}/{}",
                                        error.location.path
                                    );
                                    error.at(path)
                                })?;
                            }
                            Ok((
                                p.id,
                                p.name,
                                PortKind::Ip(interface),
                                Some(port_index),
                                p.ipv4_acl,
                            ))
                        })
                        .collect::<Result<_, InputError>>()?;
                    (
                        id,
                        name,
                        DeviceKind::Router { routes: Vec::new() },
                        port_specs,
                        routes,
                    )
                }
            };
            require_name(&device_id, "设备 ID")
                .map_err(|error| error.at(format!("/devices/{}/id", devices.len())))?;
            require_name(&name, "设备名称")
                .map_err(|error| error.at(format!("/devices/{device_index}/name")))?;
            let device = devices.len();
            if device_ids.insert(device_id.clone(), device).is_some() {
                return Err(InputError::new(format!("设备 ID {device_id} 重复。"))
                    .code("duplicate_id")
                    .at(format!("/devices/{device}/id")));
            }
            let mut device_ports = Vec::with_capacity(port_specs.len());
            port_specs.sort_unstable_by(|a, b| a.0.cmp(&b.0));
            for (port_id, name, kind, source_index, ipv4_acl) in port_specs {
                let source_path = || match source_index {
                    Some(index) => format!("/devices/{device_index}/ports/{index}"),
                    None => format!("/devices/{device_index}/port"),
                };
                require_name(&port_id, "端口 ID")
                    .map_err(|error| error.at(format!("{}/id", source_path())))?;
                require_name(&name, "端口名称")
                    .map_err(|error| error.at(format!("{}/name", source_path())))?;
                let port = ports.len();
                if port_ids.insert(port_id.clone(), port).is_some() {
                    return Err(InputError::new(format!("端口 ID {port_id} 重复。"))
                        .code("duplicate_id")
                        .at(format!("{}/id", source_path())));
                }
                if let PortKind::Ip(interface) = kind
                    && (addresses.insert(interface.ipv4, port).is_some()
                        || !macs.insert(interface.mac))
                {
                    return Err(InputError::new(format!(
                        "接口 {port_id} 的 IPv4 或 MAC 地址重复。"
                    ))
                    .code("address_conflict")
                    .at(source_path()));
                }
                if matches!(
                    kind,
                    PortKind::Switch(SwitchMode::Trunk {
                        native_vlan: Some(_),
                        ..
                    })
                ) {
                    native_vlan_count += 1;
                }
                device_ports.push(port);
                ports.push(Port {
                    id: port_id,
                    name,
                    device,
                    kind,
                    ipv4_acl,
                    link: None,
                });
            }
            if let DeviceKind::Router { routes } = &mut kind {
                *routes =
                    compile_routes(&device_id, &device_ports, &ports, &port_ids, static_routes)?;
            }
            let flood_work = device_ports
                .iter()
                .map(|&port| ports[port].filter_work())
                .sum();
            devices.push(Device {
                max_egress_acl_work: device_ports
                    .iter()
                    .map(|&p| ports[p].acl_work(PortStage::Egress))
                    .max()
                    .unwrap_or(0),
                id: device_id,
                name,
                kind,
                ports: device_ports,
                flood_work,
            });
        }

        let mut link_ids = HashMap::new();
        let mut links = Vec::with_capacity(link_specs.len());
        let mut standby_specs = Vec::new();
        for (link_index, link) in link_specs.into_iter().enumerate() {
            require_name(&link.id, "链路 ID")
                .map_err(|error| error.at(format!("/links/{}/id", links.len())))?;
            if legacy && link.transmission.is_some() {
                return Err(
                    InputError::new("链路速率与发送队列需要 format_version 2。".into())
                        .code("unsupported_feature")
                        .link(&link.id, "transmission")
                        .at(format!("/links/{link_index}/transmission")),
                );
            }
            if link.delay_ns == 0 {
                return Err(
                    InputError::new(format!("链路 {} 的延迟必须至少为 1 ns。", link.id))
                        .code("invalid_delay")
                        .link(&link.id, "delay_ns")
                        .at(format!("/links/{link_index}/delay_ns")),
                );
            }
            let [a, b] = link.ports.each_ref().map(|port| find_port(&port_ids, port));
            let (a, b) = (
                a.map_err(|error| error.at(format!("/links/{link_index}/ports/0")))?,
                b.map_err(|error| error.at(format!("/links/{link_index}/ports/1")))?,
            );
            let index = links.len();
            if link_ids.insert(link.id.clone(), index).is_some() {
                return Err(InputError::new(format!("链路 ID {} 重复。", link.id))
                    .code("duplicate_id")
                    .at(format!("/links/{index}/id")));
            }
            if a == b
                || ports[a].device == ports[b].device
                || ports[a].link.is_some()
                || ports[b].link.is_some()
            {
                return Err(InputError::new(format!(
                    "链路 {} 需要两台设备上各一个未占用的端口，不能形成自环。",
                    link.id
                ))
                .code("invalid_connection")
                .link(&link.id, "ports"));
            }
            ports[a].link = Some(Attachment {
                link: index,
                peer: b,
            });
            ports[b].link = Some(Attachment {
                link: index,
                peer: a,
            });
            if let Some(primary) = link.standby_for {
                standby_specs.push((index, primary));
            }
            links.push(Link {
                id: link.id,
                delay_ns: link.delay_ns,
                transmission: link.transmission,
                protection: None,
                ports: [a, b],
            });
        }
        if legacy && !standby_specs.is_empty() {
            return Err(InputError::new("链路主备需要 format_version 2。".into())
                .code("unsupported_feature")
                .at("/links"));
        }
        let protection = compile_protection(standby_specs, &link_ids, &ports, &mut links)?;
        let endpoint = |id: &str| {
            device_ids
                .get(id)
                .and_then(|&index| {
                    let device = &devices[index];
                    matches!(device.kind, DeviceKind::Host { .. }).then(|| device.ports[0])
                })
                .ok_or_else(|| {
                    InputError::new(format!("探测端点 {id} 必须是已定义的主机。"))
                        .code("unknown_reference")
                })
        };
        let (
            end_ns,
            arp_lifetime_ns,
            mac_lifetime_ns,
            trace_limit,
            routing_convergence_ns,
            stp_convergence_ns,
            timed,
            action_specs,
            warmup_specs,
        ) = match (legacy, probe, scenario) {
            (true, Some(probe), None) => (
                probe.deadline_ns,
                None,
                None,
                0,
                None,
                None,
                vec![TimedProbeSpec {
                    id: "1".into(),
                    source: probe.source,
                    destination: probe.destination,
                    at_ns: 0,
                    deadline_ns: probe.deadline_ns,
                    ttl: DEFAULT_TTL,
                    payload_bytes: None,
                }],
                Vec::new(),
                Vec::new(),
            ),
            (false, None, Some(scenario)) => {
                if scenario.arp_lifetime_ns == 0 || scenario.mac_lifetime_ns == 0 {
                    return Err(
                        InputError::new("ARP 和 MAC 表项的有效期须大于 0 ns。".into())
                            .code("invalid_lifetime")
                            .at("/scenario"),
                    );
                }
                (
                    scenario.end_ns,
                    Some(scenario.arp_lifetime_ns),
                    Some(scenario.mac_lifetime_ns),
                    scenario.trace_limit,
                    scenario.routing_convergence_ns,
                    scenario.stp_convergence_ns,
                    scenario.probes,
                    scenario.link_actions,
                    scenario.warmup,
                )
            }
            _ => {
                return Err(
                    InputError::new("版本 1 需要 probe，版本 2 需要 scenario。".into())
                        .code("invalid_document"),
                );
            }
        };
        let stp = StpPlan::compile(
            stp_convergence_ns,
            stp_priorities,
            stp_costs,
            &devices,
            &ports,
            &links,
            !protection.is_empty(),
        )?;
        let forest = forwarding_forest(devices.len(), &ports, &links, &protection, |link| {
            stp.as_ref().is_none_or(|plan| plan.slot(link).is_none())
        })?;
        if timed.is_empty() || timed.len() > usize::from(u16::MAX) {
            return Err(InputError::new("一次实验须声明 1 至 65535 个探测。".into())
                .code("invalid_probe_count")
                .at(if legacy { "/probe" } else { "/scenario/probes" }));
        }
        let mtu_model = format_version == 3
            || timed.iter().any(|probe| probe.payload_bytes.is_some())
            || ports.iter().any(|port| {
                port.interface()
                    .is_some_and(|ip| ip.ipv4_mtu_bytes.is_some())
            });
        let mut probes = Vec::with_capacity(timed.len());
        let mut probe_names = Vec::with_capacity(timed.len());
        let mut probe_ids = HashSet::new();
        for probe in timed {
            require_name(&probe.id, "探测 ID")
                .map_err(|error| error.at(format!("/scenario/probes/{}/id", probes.len())))?;
            if !probe_ids.insert(probe.id.clone()) {
                return Err(InputError::new(format!("探测 ID {} 重复。", probe.id))
                    .code("duplicate_id")
                    .at(format!("/scenario/probes/{}/id", probes.len())));
            }
            let source =
                endpoint(&probe.source).map_err(|error| error.probe(&probe.id, "source"))?;
            let destination = endpoint(&probe.destination)
                .map_err(|error| error.probe(&probe.id, "destination"))?;
            if source == destination {
                return Err(InputError::new("探测需要两台不同的 IPv4 主机。".into())
                    .code("invalid_probe")
                    .probe(&probe.id, ""));
            }
            if legacy {
                let from = ports[source].interface().expect("host interface");
                let to = ports[destination].interface().expect("host interface");
                if !from.on_link(to.ipv4) || !to.on_link(from.ipv4) {
                    return Err(
                        InputError::new("版本 1 的探测两端须相互视为同网段。".into())
                            .code("unsupported_feature")
                            .probe(&probe.id, ""),
                    );
                }
            }
            if probe.at_ns >= probe.deadline_ns || probe.deadline_ns > end_ns || probe.ttl == 0 {
                return Err(InputError::new(format!(
                    "探测 {} 需要正 TTL，并满足发起时间 < 截止时间 ≤ 场景结束时间。",
                    probe.id
                ))
                .code("invalid_probe_timing")
                .probe(&probe.id, ""));
            }
            let payload_bytes = probe.payload_bytes.unwrap_or(DEFAULT_ECHO_PAYLOAD_BYTES);
            if payload_bytes > MAX_ECHO_PAYLOAD_BYTES {
                return Err(InputError::new(format!(
                    "探测 {} 的 ICMP Echo 载荷须在 0 至 {MAX_ECHO_PAYLOAD_BYTES} 字节之间。",
                    probe.id
                ))
                .code("invalid_payload")
                .probe(&probe.id, "payload_bytes"));
            }
            probes.push(ProbeConfig {
                id: probes.len(),
                source,
                destination,
                at_ns: probe.at_ns,
                deadline_ns: probe.deadline_ns,
                ttl: probe.ttl,
                payload_bytes,
                advertised_destinations: [false; 2],
            });
            probe_names.push(probe.id);
        }
        let mut physical_actions = Vec::with_capacity(action_specs.len());
        let mut link_times = HashMap::new();
        for (action_index, action) in action_specs.into_iter().enumerate() {
            let &link = link_ids.get(&action.link).ok_or_else(|| {
                InputError::new(format!("未知链路 {}。", action.link))
                    .code("unknown_reference")
                    .at(format!("/scenario/link_actions/{action_index}/link"))
            })?;
            if action.at_ns > end_ns
                || link_times
                    .insert((link, action.at_ns), action_index)
                    .is_some()
            {
                return Err(InputError::new(format!(
                    "链路 {} 的动作不得晚于场景结束，同一时刻只可声明一次。",
                    action.link
                ))
                .code("invalid_link_action")
                .at(format!("/scenario/link_actions/{action_index}")));
            }
            physical_actions.push((
                action.at_ns,
                LinkUpdate {
                    link,
                    state: action.state,
                },
            ));
        }
        let mut ordered: Vec<_> = physical_actions.iter().collect();
        ordered.sort_unstable_by_key(|(at_ns, update)| (update.link, *at_ns));
        let mut status = vec![LinkStatus::Up; links.len()];
        for &(at_ns, update) in ordered {
            if status[update.link] == update.state {
                return Err(InputError::new(format!(
                    "链路 {} 在 {at_ns} ns 已处于声明的状态。",
                    links[update.link].id
                ))
                .code("invalid_link_action")
                .at(format!(
                    "/scenario/link_actions/{}/state",
                    link_times[&(update.link, at_ns)]
                )));
            }
            status[update.link] = update.state;
        }
        let link_actions = batch_link_actions(physical_actions, &links, &protection);
        let routing = RoutingPlan::compile(
            routing_convergence_ns,
            link_state_specs,
            &devices,
            &ports,
            &port_ids,
            &links,
        )?;
        if let Some(plan) = &routing {
            plan.mark_advertised_destinations(&mut probes, &ports);
        }
        let mut warmup = Vec::with_capacity(warmup_specs.len());
        for (warmup_index, action) in warmup_specs.into_iter().enumerate() {
            let port = find_port(&port_ids, &action.port)
                .map_err(|error| error.at(format!("/scenario/warmup/{warmup_index}/port")))?;
            if action.at_ns > end_ns
                || !ports[port]
                    .interface()
                    .is_some_and(|i| i.can_resolve(action.neighbor))
            {
                return Err(InputError::new(format!(
                    "端口 {} 的预热需要可直接解析的邻居，且发起时间不得晚于场景结束。",
                    action.port
                ))
                .code("invalid_warmup")
                .at(format!("/scenario/warmup/{warmup_index}")));
            }
            warmup.push(Warmup {
                port,
                neighbor: action.neighbor,
                at_ns: action.at_ns,
            });
        }
        let coverage = import_source
            .as_ref()
            .map(|source| {
                crate::import_source::CoveragePlan::compile(source, &devices, &ports, &links)
            })
            .transpose()?;
        Ok(Self {
            format_version,
            id,
            revision,
            legacy,
            mtu_model,
            native_vlan_count,
            devices,
            ports,
            links,
            protection,
            routing,
            stp,
            probes,
            probe_names,
            link_actions,
            warmup,
            end_ns,
            arp_lifetime_ns,
            mac_lifetime_ns,
            trace_limit,
            addresses,
            forest,
            import_source,
            coverage,
        })
    }

    pub(crate) fn validate_workspace_labels(&self) -> Result<(), InputError> {
        let error = || {
            InputError::new(
                "后台实验的设备、端口、链路、探测和修订标识或名称须在 256 字节以内。".into(),
            )
            .code("label_too_long")
        };
        for (value, field) in [(&self.id, "/id"), (&self.revision, "/revision")] {
            if value.len() > 256 {
                return Err(error().at(field));
            }
        }
        for device in &self.devices {
            for (value, field) in [(&device.id, "id"), (&device.name, "name")] {
                if value.len() > 256 {
                    return Err(error().device(&device.id, field));
                }
            }
        }
        for port in &self.ports {
            for (value, field) in [(&port.id, "id"), (&port.name, "name")] {
                if value.len() > 256 {
                    return Err(error().port(&port.id, field));
                }
            }
        }
        for link in &self.links {
            if link.id.len() > 256 {
                return Err(error().link(&link.id, "id"));
            }
        }
        for probe in &self.probe_names {
            if probe.len() > 256 {
                return Err(error().probe(probe, "id"));
            }
        }
        Ok(())
    }

    pub(crate) fn interface(&self, index: PortIndex) -> Interface {
        self.ports[index].interface().expect("IP interface")
    }

    pub(crate) fn probe(&self, config: ProbeConfig) -> Probe {
        let endpoint = |port: PortIndex| {
            let device = &self.devices[self.ports[port].device];
            Endpoint {
                device_id: device.id.clone(),
                name: device.name.clone(),
                ipv4: self.interface(port).ipv4,
            }
        };
        Probe {
            source: endpoint(config.source),
            destination: endpoint(config.destination),
            deadline_ns: config.deadline_ns,
        }
    }

    pub(crate) fn port_reference(&self, index: PortIndex) -> PortReference {
        let port = &self.ports[index];
        let device = &self.devices[port.device];
        PortReference {
            device_id: device.id.clone(),
            device_name: device.name.clone(),
            port_id: port.id.clone(),
            port_name: port.name.clone(),
        }
    }

    pub(crate) fn on_required_branch(
        &self,
        stp_forest: Option<&ForwardingForest>,
        source: PortIndex,
        target: Ipv4Addr,
        from: PortIndex,
        stage: PortStage,
    ) -> bool {
        let Some(&target) = self.addresses.get(&target) else {
            return false;
        };
        let Some(attached) = self.ports[from].link else {
            return false;
        };
        if let Some(forest) = stp_forest
            && let Some(plan) = self.stp.as_ref()
            && let Some(slot) = plan.forest.slot(attached.link)
        {
            return plan
                .forest
                .on_required_branch(forest, slot, [source, target, from], stage);
        }
        let vertices =
            [source, target, from].map(|port| port_vertex(self.devices.len(), &self.ports, port));
        self.forest
            .on_required_branch(attached.link, vertices, stage)
    }
}

fn find_port(ports: &HashMap<String, PortIndex>, id: &str) -> Result<PortIndex, InputError> {
    ports
        .get(id)
        .copied()
        .ok_or_else(|| InputError::new(format!("未知端口 {id}。")).code("unknown_reference"))
}

fn interface(
    id: &str,
    ip: Ipv4Addr,
    length: u8,
    mac: &str,
    ipv4_mtu_bytes: Option<u16>,
) -> Result<Interface, InputError> {
    if length > 32 || !usable_ipv4(ip, length) {
        return Err(InputError::new(format!(
            "接口 {id} 的 IPv4 地址 {ip}/{length} 不能用于此模型。"
        ))
        .code("invalid_ipv4")
        .port(id, if length > 32 { "prefix_length" } else { "ipv4" }));
    }
    let mac = Mac::parse(mac).ok_or_else(|| {
        InputError::new(format!("接口 {id} 的 MAC 地址 {mac} 不是有效的单播地址。"))
            .code("invalid_mac")
            .port(id, "mac")
    })?;
    if ipv4_mtu_bytes.is_some_and(|mtu| mtu < MIN_IPV4_MTU_BYTES) {
        return Err(InputError::new(format!(
            "接口 {id} 的 IPv4 MTU 须在 {MIN_IPV4_MTU_BYTES} 至 {} 字节之间。",
            u16::MAX
        ))
        .code("invalid_mtu")
        .port(id, "ipv4_mtu_bytes"));
    }
    Ok(Interface {
        ipv4: ip,
        mac,
        ipv4_mtu_bytes,
        prefix_length: length,
    })
}

fn compile_routes(
    device: &str,
    device_ports: &[PortIndex],
    ports: &[Port],
    port_ids: &HashMap<String, PortIndex>,
    specs: Vec<RouteSpec>,
) -> Result<Vec<Route>, InputError> {
    let mut routes = Vec::with_capacity(device_ports.len() + specs.len());
    let mut prefixes = HashSet::new();
    for &egress in device_ports {
        let interface = ports[egress].interface().expect("router interface");
        let route = interface.connected_route(egress);
        let Route {
            network, length, ..
        } = route;
        if !prefixes.insert((network, length)) {
            return Err(InputError::new(format!(
                "路由器 {device} 的直连前缀 {network}/{length} 重复。"
            ))
            .code("duplicate_prefix")
            .device(device, "ports"));
        }
        routes.push(route);
    }
    prefixes.clear();
    for (route_index, spec) in specs.into_iter().enumerate() {
        let RouteSpec {
            network,
            prefix_length: length,
            egress,
            next_hop,
        } = spec;
        if length > 32 || (u32::from(network) & netmask(length)) != u32::from(network) {
            return Err(InputError::new(format!(
                "路由器 {device} 的静态前缀 {network}/{length} 不是规范网络地址。"
            ))
            .code("invalid_prefix")
            .device(
                device,
                &format!(
                    "routes/{route_index}/{}",
                    if length > 32 {
                        "prefix_length"
                    } else {
                        "network"
                    }
                ),
            ));
        }
        if !prefixes.insert((network, length)) {
            return Err(InputError::new(format!(
                "路由器 {device} 的静态前缀 {network}/{length} 重复。"
            ))
            .code("duplicate_prefix")
            .device(device, &format!("routes/{route_index}")));
        }
        let port = find_port(port_ids, &egress)
            .map_err(|error| error.device(device, &format!("routes/{route_index}/egress")))?;
        if ports[port].device != ports[device_ports[0]].device
            || !ports[port]
                .interface()
                .is_some_and(|i| i.can_resolve(next_hop))
        {
            return Err(InputError::new(format!(
                "路由器 {device} 的静态下一跳 {next_hop} 必须经本路由器的接口 {egress} 直连。"
            ))
            .code("invalid_next_hop")
            .device(device, &format!("routes/{route_index}/next_hop")));
        }
        routes.push(Route {
            network,
            length,
            egress: port,
            next_hop: Some(next_hop),
        });
    }
    // Stable sorting keeps connected routes ahead of equal-length static routes.
    routes.sort_by_key(|route| std::cmp::Reverse(route.length));
    Ok(routes)
}

fn require_name(value: &str, label: &str) -> Result<(), InputError> {
    if value.trim().is_empty() {
        return Err(InputError::new(format!("{label}不能为空。")).code("empty_value"));
    }
    Ok(())
}

fn validated_mode(
    port: &str,
    mut mode: SwitchMode,
    legacy: bool,
) -> Result<SwitchMode, InputError> {
    let vlans = match &mut mode {
        SwitchMode::Access { vlan } => std::slice::from_ref(vlan),
        SwitchMode::Trunk {
            allowed_vlans,
            native_vlan,
        } => {
            if let Some(vlan) = native_vlan {
                if legacy {
                    return Err(InputError::new(
                        "Trunk Native VLAN 需要 format_version 2 或 3。".into(),
                    )
                    .code("invalid_native_vlan")
                    .port(port, "mode/native_vlan"));
                }
                if !(1..=4094).contains(vlan) {
                    return Err(InputError::new(format!(
                        "端口 {port} 的 Native VLAN {vlan} 超出 1–4094 范围。"
                    ))
                    .code("vlan_out_of_range")
                    .port(port, "mode/native_vlan"));
                }
            }
            allowed_vlans.sort_unstable();
            allowed_vlans.dedup();
            allowed_vlans.as_slice()
        }
    };
    if let Some(vlan) = vlans.iter().find(|&&vlan| !(1..=4094).contains(&vlan)) {
        return Err(
            InputError::new(format!("端口 {port} 的 VLAN {vlan} 超出 1–4094 范围。"))
                .code("vlan_out_of_range")
                .port(
                    port,
                    if matches!(mode, SwitchMode::Access { .. }) {
                        "mode/vlan"
                    } else {
                        "mode/allowed_vlans"
                    },
                ),
        );
    }
    Ok(mode)
}

pub(crate) fn netmask(prefix_length: u8) -> u32 {
    if prefix_length == 0 {
        0
    } else {
        u32::MAX << (32 - prefix_length)
    }
}

fn usable_ipv4(ip: Ipv4Addr, prefix_length: u8) -> bool {
    let host_mask = !netmask(prefix_length);
    let host_bits = u32::from(ip) & host_mask;
    !(ip.is_unspecified()
        || ip.is_loopback()
        || ip.is_multicast()
        || ip.is_broadcast()
        || (prefix_length < 31 && (host_bits == 0 || host_bits == host_mask)))
}

fn port_vertex(device_count: usize, ports: &[Port], port: PortIndex) -> usize {
    match ports[port].kind {
        PortKind::Switch(_) => ports[port].device,
        PortKind::Ip(_) => device_count + port,
    }
}

fn compile_protection(
    specs: Vec<(LinkIndex, String)>,
    ids: &HashMap<String, LinkIndex>,
    ports: &[Port],
    links: &mut [Link],
) -> Result<Vec<LinkProtection>, InputError> {
    let mut pairs = Vec::with_capacity(specs.len());
    for (standby, primary_id) in specs {
        let standby_id = &links[standby].id;
        let &primary = ids.get(&primary_id).ok_or_else(|| {
            InputError::new(format!(
                "备用链路 {standby_id} 引用了未知主链路 {primary_id}。"
            ))
            .code("unknown_reference")
            .link(standby_id, "standby_for")
        })?;
        if primary == standby
            || links[primary].protection.is_some()
            || links[standby].protection.is_some()
        {
            return Err(InputError::new(format!(
                "链路 {primary_id} 与 {standby_id} 必须各自只属于一组主备。"
            ))
            .code("invalid_protection")
            .link(standby_id, "standby_for"));
        }
        let primary_ports = links[primary].ports;
        let standby_ports = links[standby].ports;
        let [a, b] = primary_ports.map(|port| ports[port].device);
        let endpoints = standby_ports.map(|port| ports[port].device);
        if !(endpoints == [a, b] || endpoints == [b, a])
            || primary_ports
                .into_iter()
                .chain(standby_ports)
                .any(|port| !matches!(ports[port].kind, PortKind::Switch(_)))
        {
            return Err(InputError::new(format!(
                "主链路 {primary_id} 与备用链路 {standby_id} 必须连接同一对交换机。"
            ))
            .code("invalid_protection")
            .link(standby_id, "standby_for"));
        }
        links[primary].protection = Some(pairs.len());
        links[standby].protection = Some(pairs.len());
        pairs.push(LinkProtection { primary, standby });
    }
    Ok(pairs)
}

fn batch_link_actions(
    actions: Vec<(u64, LinkUpdate)>,
    links: &[Link],
    protection: &[LinkProtection],
) -> Vec<LinkChange> {
    let mut batches: Vec<LinkChange> = Vec::with_capacity(actions.len());
    let mut by_pair_and_time = HashMap::new();
    for (at_ns, update) in actions {
        if let Some(pair) = links[update.link].protection {
            let index = *by_pair_and_time
                .entry((pair, at_ns))
                .or_insert(batches.len());
            if let Some(batch) = batches.get_mut(index) {
                // Keep both members in one event and put the primary first in its report.
                batch.second = Some(if update.link == protection[pair].primary {
                    std::mem::replace(&mut batch.first, update)
                } else {
                    update
                });
                continue;
            }
        }
        batches.push(LinkChange {
            at_ns,
            first: update,
            second: None,
        });
    }
    batches
}

fn forwarding_forest(
    device_count: usize,
    ports: &[Port],
    links: &[Link],
    protection: &[LinkProtection],
    active: impl Fn(LinkIndex) -> bool,
) -> Result<ForwardingForest, InputError> {
    let edges = links
        .iter()
        .enumerate()
        .filter(|&(index, link)| {
            active(index)
                && link
                    .protection
                    .is_none_or(|group| protection[group].standby != index)
        })
        .map(|(index, link)| {
            (
                index,
                link.ports
                    .map(|port| port_vertex(device_count, ports, port)),
            )
        });
    let adjacency = forest_adjacency(device_count + ports.len(), edges);
    let mut forest = index_forest(&adjacency, links.len(), |_| true).map_err(|link| {
        let [a, b] = links[link].ports;
        InputError::new(format!(
            "端口 {} 与 {} 的连接使二层拓扑形成环路。",
            ports[a].id, ports[b].id
        ))
        .code("layer2_cycle")
        .link(&links[link].id, "ports")
    })?;
    // Both physical members cross the same logical cut, in either direction.
    for pair in protection {
        forest.children[pair.standby] = forest.children[pair.primary];
    }
    Ok(forest)
}

fn forest_adjacency(
    vertex_count: usize,
    edges: impl Iterator<Item = (usize, [usize; 2])>,
) -> Vec<Vec<(usize, usize)>> {
    let mut adjacency = vec![Vec::new(); vertex_count];
    for (slot, [a, b]) in edges {
        adjacency[a].push((b, slot));
        adjacency[b].push((a, slot));
    }
    adjacency
}

fn index_forest(
    adjacency: &[Vec<(usize, usize)>],
    link_count: usize,
    active: impl Fn(usize) -> bool,
) -> Result<ForwardingForest, usize> {
    // Each IP interface is a leaf. Router ports never bridge two Ethernet segments.
    let mut vertices: Vec<_> = (0..adjacency.len()).map(|_| Vertex::default()).collect();
    let mut children = vec![None; link_count];
    let mut clock = 1;
    let mut stack = Vec::with_capacity(adjacency.len());
    for root in 0..vertices.len() {
        if vertices[root].entry != 0 {
            continue;
        }
        stack.push((root, None::<LinkIndex>, false));
        while let Some((vertex, incoming, leaving)) = stack.pop() {
            if leaving {
                vertices[vertex].end = clock;
                continue;
            }
            if let Some(link) = incoming {
                if vertices[vertex].entry != 0 {
                    return Err(link);
                }
                children[link] = Some(vertex);
            }
            vertices[vertex] = Vertex {
                component: root,
                entry: clock,
                end: 0,
            };
            clock += 1;
            stack.push((vertex, incoming, true));
            for &(peer, via) in &adjacency[vertex] {
                if Some(via) != incoming && active(via) {
                    stack.push((peer, Some(via), false));
                }
            }
        }
    }
    Ok(ForwardingForest { vertices, children })
}
