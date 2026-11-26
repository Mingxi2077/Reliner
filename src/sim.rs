use std::{
    cmp::{Ordering, Reverse},
    collections::{BTreeMap, BinaryHeap, HashMap, HashSet},
    net::Ipv4Addr,
    num::NonZeroU64,
};

use crate::dynamic::{LearnedRoute, RoutingPlan};
use crate::report::{possible_history_copy, reserve_history_append};
use crate::topology::{
    DEFAULT_TTL, DeviceIndex, DeviceKind, ECHO_HEADERS_BYTES, ForwardingForest, Interface,
    LinkChange, LinkIndex, LinkProtection, Mac, PortIndex, PortKind, ProbeConfig, ProbeIndex,
    SwitchMode, Topology, Transmission,
};
use crate::{
    DropRule, DynamicRouteMissReason, EventId, Evidence, EvidenceFact, EvidenceId, FrameEvidence,
    FrameId, FrameKind, IcmpIdentity, LinkEvent, LinkMemberChange, LinkStatus,
    LinkTransmissionSettings, Outcome, PortStage, ProbeReport, ProtectedLinkEvidence,
    ProtectedLinks, ProtectionChange, Report, ResourceLimit, RoutingEvent, RunLimits, RunReport,
    ScenarioEvidence, ScenarioReport, ScenarioSettings, StopReason, StpBridgeState, StpEventHeader,
    StpHistory, StpLinkState, StpSettings, TraceEntry, TraceEvent, TraceLearnedRoute,
    TraceLearning, TraceMtuCheck, TraceReport, TraceRoute, TraceStatus, WaitingFor,
};

pub(crate) fn run(topology: Topology, limits: RunLimits) -> RunReport {
    let mut simulation = Simulation::new(topology, limits);
    while simulation.next_work().is_some() {
        simulation.step();
    }
    simulation.report()
}

type Neighbor = (PortIndex, Ipv4Addr);
type MacKey = (DeviceIndex, u16, Mac);

#[derive(Clone, Copy)]
enum Packet {
    Arp {
        sender: Ipv4Addr,
        target: Ipv4Addr,
        reply: bool,
        probe: Option<ProbeIndex>,
    },
    Ip(IpPacket),
}

impl Packet {
    fn kind(self) -> FrameKind {
        match self {
            Self::Arp { reply: false, .. } => FrameKind::ArpRequest,
            Self::Arp { reply: true, .. } => FrameKind::ArpReply,
            Self::Ip(IpPacket { reply: false, .. }) => FrameKind::EchoRequest,
            Self::Ip(IpPacket { reply: true, .. }) => FrameKind::EchoReply,
        }
    }

    fn probe(self) -> Option<ProbeIndex> {
        match self {
            Self::Arp { probe, .. } => probe,
            Self::Ip(ip) => Some(ip.probe),
        }
    }
}

#[derive(Clone, Copy)]
struct IpPacket {
    source: Ipv4Addr,
    destination: Ipv4Addr,
    ttl: u8,
    ipv4_total_bytes: u16,
    probe: ProbeIndex,
    reply: bool,
}

#[derive(Clone, Copy)]
struct WireFrame {
    source_mac: Mac,
    destination_mac: Mac,
    packet: Packet,
    tag: Option<u16>,
    origin: PortIndex,
    neighbor: Ipv4Addr,
    parent_id: Option<FrameId>,
    stp_generation: u64,
    required_at_send: Option<bool>,
    routing_event: Option<NonZeroU64>,
}

impl WireFrame {
    fn mac_bytes(self) -> u32 {
        let payload = match self.packet {
            Packet::Arp { .. } => 28,
            Packet::Ip(ip) => u32::from(ip.ipv4_total_bytes),
        };
        (14 + 4 * u32::from(self.tag.is_some()) + payload + 4).max(64)
    }

    fn evidence(self, id: FrameId) -> FrameEvidence {
        FrameEvidence {
            id,
            parent_id: self.parent_id,
            kind: self.packet.kind(),
        }
    }
}

#[derive(Clone, Copy)]
struct Arrival {
    port: PortIndex,
    wire: WireFrame,
    link: LinkIndex,
    generation: u64,
}

struct ScheduledFrame {
    arrival: Arrival,
    arrives_at_ns: u64,
    transmit_at_ns: Option<u64>,
}

#[derive(Clone, Copy)]
enum TableKey {
    Mac(MacKey),
    Arp(Neighbor),
}

#[derive(Clone, Copy)]
enum Action {
    Link(usize),
    ConvergeRoute(usize),
    ConvergeStp,
    Expire {
        key: TableKey,
        generation: EventId,
    },
    Deadline(ProbeIndex),
    Transmit {
        arrival: Arrival,
        arrives_at_ns: u64,
        arrival_order: u64,
    },
    Arrival(Arrival),
    Start(ProbeIndex),
    Warm(Neighbor),
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Phase {
    LinkChange,
    Control,
    Timer,
    Transmit,
    Arrival,
    Start,
}

impl Action {
    fn phase(self) -> Phase {
        match self {
            Self::Link(_) => Phase::LinkChange,
            Self::ConvergeRoute(_) | Self::ConvergeStp => Phase::Control,
            Self::Transmit { .. } => Phase::Transmit,
            Self::Expire { .. } | Self::Deadline(_) => Phase::Timer,
            Self::Arrival(_) => Phase::Arrival,
            Self::Start(_) | Self::Warm(_) => Phase::Start,
        }
    }
}

#[derive(Clone, Copy)]
struct Event {
    key: (u64, Phase, u64),
    action: Action,
}

impl Ord for Event {
    fn cmp(&self, other: &Self) -> Ordering {
        self.key.cmp(&other.key)
    }
}

impl PartialOrd for Event {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Eq for Event {}

impl PartialEq for Event {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key
    }
}

#[derive(Default, Clone, Copy)]
struct LinkState {
    generation: u64,
    down_event: Option<EventId>,
}

#[derive(Clone, Copy, Default)]
struct Transmitter {
    busy_until_ns: u64,
    waiting: u16,
}

#[derive(Clone, Copy)]
enum LinkInvalidation {
    PhysicalDown(EventId),
    ForwardingChanged(Point),
    SpanningTreeChanged(Point),
}

#[derive(Default)]
struct InFlight {
    remaining: usize,
    invalidated_by: Option<LinkInvalidation>,
}

struct LearnedMac {
    port: PortIndex,
    link_generation: u64,
    stp_generation: u64,
}

struct Entry<T> {
    value: T,
    generation: EventId,
}

#[derive(Clone, Copy)]
enum Learning {
    Mac { key: MacKey, port: PortIndex },
    Arp { key: Neighbor, mac: Mac },
}

#[derive(Clone, Copy)]
struct PendingPacket {
    packet: IpPacket,
    parent_id: Option<FrameId>,
    routing_event: Option<NonZeroU64>,
}

struct ProbeState {
    outcome: Option<Outcome>,
    waiting_for: WaitingFor,
    neighbor: Option<Neighbor>,
    request: Option<EvidenceId>,
    cause: Option<EvidenceId>,
}

impl Default for ProbeState {
    fn default() -> Self {
        Self {
            outcome: None,
            waiting_for: WaitingFor::ArpReply,
            neighbor: None,
            request: None,
            cause: None,
        }
    }
}

struct Observation {
    probe: ProbeIndex,
    port: PortIndex,
    frame: FrameEvidence,
    fact: EvidenceFact,
    blocking: bool,
    routing_event: Option<NonZeroU64>,
}

#[derive(Clone, Copy)]
struct Point {
    at_ns: u64,
    id: EventId,
}

impl Point {
    fn frame(self) -> FrameId {
        FrameId(self.id.0)
    }
}

#[derive(Clone, Copy)]
struct ControlRound {
    at_ns: u64,
    trigger: Option<EventId>,
}

struct RoutingTable {
    event_id: EventId,
    routes: Vec<LearnedRoute>,
}

struct StpTree {
    active: Vec<bool>,
    forest: ForwardingForest,
    event_id: EventId,
}

impl StpTree {
    fn entries(&self) -> usize {
        self.active.len() + self.forest.entries()
    }
}

struct StpUpdate {
    tree: StpTree,
    event: StpEventHeader,
    bridges: Vec<(usize, StpBridgeState)>,
    links: Vec<(usize, StpLinkState)>,
}

impl StpUpdate {
    fn facts(&self) -> usize {
        1 + self.bridges.len() + 3 * self.links.len()
    }
}

#[derive(Clone, Copy)]
struct RouteChoice {
    key: Neighbor,
    destination: Ipv4Addr,
    prefix: Option<(Ipv4Addr, u8)>,
    ipv4_mtu: Option<TraceMtuCheck>,
    learned: Option<(LearnedRoute, EventId)>,
}

#[derive(Default)]
struct Effects {
    learning: Option<Learning>,
    expiry: Option<(u64, Action)>,
    frames: Vec<ScheduledFrame>,
    transmitters: HashMap<PortIndex, Transmitter>,
    observations: Vec<Observation>,
    pending: Option<(Neighbor, PendingPacket)>,
    resolved: Option<Neighbor>,
    request_sent: Vec<ProbeIndex>,
    route: Option<RouteChoice>,
    routing_round: Option<ControlRound>,
    routing_table: Option<Vec<LearnedRoute>>,
    stp_round: Option<ControlRound>,
    stp_update: Option<StpUpdate>,
}

impl Effects {
    fn observe(
        &mut self,
        probe: ProbeIndex,
        port: PortIndex,
        frame: FrameEvidence,
        fact: EvidenceFact,
        blocking: bool,
    ) {
        self.observations.push(Observation {
            probe,
            port,
            frame,
            fact,
            blocking,
            routing_event: None,
        });
    }
}

pub(crate) struct Simulation {
    topology: Topology,
    limits: RunLimits,
    queue: BinaryHeap<Reverse<Event>>,
    sequence: u64,
    links: Vec<LinkState>,
    transmitters: HashMap<PortIndex, Transmitter>,
    protection_selections: Vec<Option<EventId>>,
    forwarding_flood_work: HashMap<DeviceIndex, usize>,
    in_flight: HashMap<(LinkIndex, u64), InFlight>,
    mac: HashMap<MacKey, Entry<LearnedMac>>,
    arp: HashMap<Neighbor, Entry<Mac>>,
    pending: HashMap<Neighbor, BTreeMap<ProbeIndex, PendingPacket>>,
    pending_count: usize,
    probes: Vec<ProbeState>,
    evidence: Vec<(ProbeIndex, Evidence)>,
    link_events: Vec<LinkEvent>,
    link_facts: usize,
    routing_round: Option<ControlRound>,
    routing_tables: Vec<Option<RoutingTable>>,
    routing_entries: usize,
    routing_events: Vec<RoutingEvent>,
    stp_round: Option<ControlRound>,
    stp_tree: Option<StpTree>,
    stp_recovered: Vec<Option<EventId>>,
    stp_mac_generation: u64,
    stp_events: StpHistory,
    stp_facts: usize,
    trace: TraceReport,
    trace_limit: usize,
    work: usize,
    last: Option<Point>,
    stop: Option<StopReason>,
}

impl Simulation {
    pub(crate) fn new(topology: Topology, limits: RunLimits) -> Self {
        let requested_trace = topology.trace_limit;
        let trace_limit = requested_trace.min(limits.trace_entries);
        let nodes = topology.routing.as_ref().map_or(0, |plan| plan.nodes.len());
        let initial_routing_round = topology.routing.as_ref().map(|plan| ControlRound {
            at_ns: plan.convergence_ns,
            trigger: None,
        });
        let stp_links = topology.stp.as_ref().map_or(0, |plan| plan.link_count());
        let initial_stp_round = topology.stp.as_ref().map(|plan| ControlRound {
            at_ns: plan.convergence_ns,
            trigger: None,
        });
        let mut sim = Self {
            links: vec![LinkState::default(); topology.links.len()],
            transmitters: HashMap::new(),
            protection_selections: vec![None; topology.protection.len()],
            forwarding_flood_work: HashMap::new(),
            probes: (0..topology.probes.len())
                .map(|_| ProbeState::default())
                .collect(),
            topology,
            limits,
            queue: BinaryHeap::new(),
            sequence: 0,
            in_flight: HashMap::new(),
            mac: HashMap::new(),
            arp: HashMap::new(),
            pending: HashMap::new(),
            pending_count: 0,
            evidence: Vec::new(),
            link_events: Vec::new(),
            link_facts: 0,
            routing_round: None,
            routing_tables: (0..nodes).map(|_| None).collect(),
            routing_entries: 0,
            routing_events: Vec::new(),
            stp_round: None,
            stp_tree: None,
            stp_recovered: vec![None; stp_links],
            stp_mac_generation: 0,
            stp_events: StpHistory::default(),
            stp_facts: 0,
            trace: TraceReport {
                status: if requested_trace > 0 {
                    TraceStatus::Collected
                } else {
                    TraceStatus::Disabled
                },
                entries: Vec::new(),
            },
            trace_limit,
            work: 0,
            last: None,
            stop: None,
        };
        let declared = sim.probes.len() * 2
            + sim.topology.link_actions.len()
            + sim.topology.warmup.len()
            + sim.routing_event_count(initial_routing_round)
            + sim.stp_event_count(initial_stp_round);
        if declared > limits.pending_events {
            sim.exhaust(ResourceLimit::PendingEvents);
            return sim;
        }
        for index in 0..sim.topology.protection.len() {
            sim.adjust_forwarding_flood_work(sim.topology.protection[index].standby, false);
        }
        for index in 0..sim.topology.link_actions.len() {
            sim.schedule(sim.topology.link_actions[index].at_ns, Action::Link(index));
        }
        for index in 0..sim.topology.warmup.len() {
            let warmup = &sim.topology.warmup[index];
            sim.schedule(warmup.at_ns, Action::Warm((warmup.port, warmup.neighbor)));
        }
        for index in 0..sim.probes.len() {
            let probe = sim.topology.probes[index];
            sim.schedule(probe.at_ns, Action::Start(index));
            sim.schedule(probe.deadline_ns, Action::Deadline(index));
        }
        if let Some(round) = initial_routing_round {
            sim.schedule_routing_round(round);
        }
        if let Some(round) = initial_stp_round {
            for slot in 0..stp_links {
                let link = sim.topology.stp.as_ref().unwrap().link_at(slot);
                sim.adjust_forwarding_flood_work(link, false);
            }
            sim.schedule_stp_round(round);
        }
        sim
    }

    fn schedule(&mut self, at_ns: u64, action: Action) {
        self.sequence += 1;
        self.schedule_reserved(at_ns, action, self.sequence);
    }

    fn schedule_reserved(&mut self, at_ns: u64, action: Action, order: u64) {
        self.queue.push(Reverse(Event {
            key: (at_ns, action.phase(), order),
            action,
        }));
    }

    fn routing_event_count(&self, round: Option<ControlRound>) -> usize {
        round
            .filter(|round| round.at_ns <= self.topology.end_ns)
            .map_or(0, |_| self.routing_tables.len())
    }

    fn schedule_routing_round(&mut self, round: ControlRound) {
        self.routing_round = Some(round);
        for node in 0..self.routing_event_count(Some(round)) {
            self.schedule(round.at_ns, Action::ConvergeRoute(node));
        }
    }

    fn stp_event_count(&self, round: Option<ControlRound>) -> usize {
        usize::from(round.is_some_and(|round| round.at_ns <= self.topology.end_ns))
    }

    fn schedule_stp_round(&mut self, round: ControlRound) {
        self.stp_round = Some(round);
        if self.stp_event_count(Some(round)) != 0 {
            self.schedule(round.at_ns, Action::ConvergeStp);
        }
    }

    fn control_due(&self, at_ns: u64, current: Option<ControlRound>, delay: u64) -> Option<u64> {
        // Changes on the boundary belong to the already scheduled control phase.
        (current.is_none_or(|round| round.at_ns < at_ns) && delay <= self.topology.end_ns - at_ns)
            .then(|| at_ns + delay)
    }

    fn stp_due(&self, at_ns: u64, change: LinkChange) -> Option<u64> {
        let plan = self.topology.stp.as_ref()?;
        self.control_due(at_ns, self.stp_round, plan.convergence_ns)
            .filter(|_| {
                change
                    .updates()
                    .any(|update| plan.slot(update.link).is_some())
            })
    }

    fn routing_due(&self, at_ns: u64, change: LinkChange) -> Option<u64> {
        let plan = self.topology.routing.as_ref()?;
        self.control_due(at_ns, self.routing_round, plan.convergence_ns)
            .filter(|_| change.updates().any(|update| plan.watches(update.link)))
    }

    fn stage_link_change(&self, point: Point, change: LinkChange) -> Effects {
        let round = |at_ns| ControlRound {
            at_ns,
            trigger: Some(point.id),
        };
        Effects {
            routing_round: self.routing_due(point.at_ns, change).map(round),
            stp_round: self.stp_due(point.at_ns, change).map(round),
            ..Effects::default()
        }
    }

    fn stage_stp(&self, point: Point) -> StpUpdate {
        let plan = self.topology.stp.as_ref().expect("spanning tree plan");
        let snapshot = plan.select(&self.topology, &self.stp_events, |link| {
            self.links[link].down_event.is_none()
        });
        let forest = plan.forest.build(&snapshot.active);
        StpUpdate {
            tree: StpTree {
                active: snapshot.active,
                forest,
                event_id: point.id,
            },
            event: StpEventHeader {
                event_id: point.id,
                at_ns: point.at_ns,
                triggered_by_link_event_id: self.stp_round.expect("scheduled STP round").trigger,
            },
            bridges: snapshot.bridges,
            links: snapshot.links,
        }
    }

    fn exhaust(&mut self, limit: ResourceLimit) {
        self.stop = Some(StopReason::ResourceLimit { limit });
    }

    pub(crate) fn cancel(&mut self) {
        self.stop = Some(StopReason::Cancelled);
    }

    pub(crate) fn last_event(&self) -> Option<(u64, EventId)> {
        self.last.map(|p| (p.at_ns, p.id))
    }

    pub(crate) fn next_work(&self) -> Option<usize> {
        if self.stop.is_some() || (self.topology.legacy && self.probes[0].outcome.is_some()) {
            return None;
        }
        let event = self.queue.peek()?.0;
        if event.key.0 > self.topology.end_ns {
            return None;
        }
        Some(
            1 + match event.action {
                Action::Arrival(arrival) if self.invalidated(arrival).is_none() => {
                    let port = &self.topology.ports[arrival.port];
                    let device = &self.topology.devices[port.device];
                    match &port.kind {
                        PortKind::Switch(mode) => {
                            let vlan = match mode {
                                SwitchMode::Access { vlan } => Some(*vlan),
                                SwitchMode::Trunk { native_vlan, .. } => {
                                    arrival.wire.tag.or(*native_vlan)
                                }
                            };
                            let known = vlan.and_then(|vlan| {
                                self.known_mac_port((
                                    port.device,
                                    vlan,
                                    arrival.wire.destination_mac,
                                ))
                            });
                            let egress_work = match known {
                                Some(egress) => {
                                    let target = &self.topology.ports[egress];
                                    let skipped = self.topology.stp.as_ref().is_some_and(|plan| {
                                        egress == arrival.port
                                            || target
                                                .link
                                                .and_then(|link| plan.slot(link.link))
                                                .is_some_and(|slot| {
                                                    self.stp_recovered[slot].is_some()
                                                })
                                    });
                                    if skipped { 1 } else { target.filter_work() }
                                }
                                None => {
                                    let flood = self
                                        .forwarding_flood_work
                                        .get(&port.device)
                                        .copied()
                                        .unwrap_or(device.flood_work);
                                    flood
                                        - if self.topology.stp.is_some() {
                                            port.vlan_work()
                                        } else {
                                            0
                                        }
                                }
                            };
                            port.filter_work() + egress_work
                        }
                        PortKind::Ip(_) => match arrival.wire.packet {
                            Packet::Arp { sender, .. } => {
                                2 + 2 * self
                                    .pending
                                    .get(&(arrival.port, sender))
                                    .map_or(0, BTreeMap::len)
                            }
                            Packet::Ip(_) => match &device.kind {
                                DeviceKind::Router { routes } => {
                                    let dynamic = self
                                        .dynamic_table(port.device)
                                        .flatten()
                                        .map_or(0, |table| table.routes.len() + 1);
                                    // Charge before selecting the route or evaluating either list.
                                    2 + routes.len()
                                        + dynamic
                                        + port.acl_work(PortStage::Ingress)
                                        + device.max_egress_acl_work
                                }
                                _ => 2,
                            },
                        },
                    }
                }
                Action::Start(_) | Action::Warm(_) => 2,
                Action::Link(index) => {
                    let change = self.topology.link_actions[index];
                    let protected = self.topology.links[change.first.link].protection.is_some();
                    // A role change can adjust both endpoints of both members.
                    let routing_events = self
                        .routing_due(event.key.0, change)
                        .map_or(0, |_| self.routing_tables.len());
                    let stp_event = self.stp_due(event.key.0, change).is_some();
                    let stp_gate_work = self.topology.stp.as_ref().map_or(0, |plan| {
                        2 * change
                            .updates()
                            .filter(|update| {
                                plan.slot(update.link)
                                    .is_some_and(|slot| self.stp_forwards(slot))
                            })
                            .count()
                    });
                    change.update_count() - 1
                        + 4 * usize::from(protected)
                        + routing_events
                        + usize::from(stp_event)
                        + stp_gate_work
                }
                Action::ConvergeRoute(_) => {
                    self.topology.routing.as_ref().expect("routing plan").work
                        + possible_history_copy(self.routing_events.len())
                }
                Action::ConvergeStp => {
                    self.topology.stp.as_ref().expect("spanning tree plan").work
                        + self.stp_events.append_work()
                }
                _ => 0,
            },
        )
    }

    pub(crate) fn step(&mut self) {
        let Some(cost) = self.next_work() else { return };
        if cost > self.limits.event_work {
            self.exhaust(ResourceLimit::EventWork);
            return;
        }
        if cost > self.limits.total_work - self.work {
            self.exhaust(ResourceLimit::TotalWork);
            return;
        }
        let Reverse(event) = *self.queue.peek().expect("runnable event");
        let point = Point {
            at_ns: event.key.0,
            id: EventId(event.key.2),
        };
        let effects = match event.action {
            Action::Arrival(arrival) => self.receive(point, arrival),
            Action::Start(index) => self.start(point, self.topology.probes[index]),
            Action::Warm(key) => self.warm(point, key),
            Action::Link(index) => {
                Ok(self.stage_link_change(point, self.topology.link_actions[index]))
            }
            Action::ConvergeRoute(node) => {
                let plan = self.topology.routing.as_ref().expect("routing plan");
                Ok(Effects {
                    routing_table: Some(
                        plan.routes(node, |link| self.links[link].down_event.is_none()),
                    ),
                    ..Effects::default()
                })
            }
            Action::ConvergeStp => Ok(Effects {
                stp_update: Some(self.stage_stp(point)),
                ..Effects::default()
            }),
            _ => Ok(Effects::default()),
        };
        let mut effects = match effects {
            Ok(effects) => effects,
            Err(reason) => {
                self.stop = Some(reason);
                return;
            }
        };
        let queued_replacement = usize::from(matches!(event.action, Action::Transmit { .. }));
        if self.queue.len() - 1
            + effects.frames.len()
            + usize::from(effects.expiry.is_some())
            + queued_replacement
            + self.routing_event_count(effects.routing_round)
            + self.stp_event_count(effects.stp_round)
            > self.limits.pending_events
        {
            self.exhaust(ResourceLimit::PendingEvents);
            return;
        }
        let new_entries = match effects.learning {
            Some(Learning::Mac { key, .. }) => usize::from(!self.mac.contains_key(&key)),
            Some(Learning::Arp { key, .. }) => usize::from(!self.arp.contains_key(&key)),
            None => 0,
        } + usize::from(effects.pending.is_some())
            + effects
                .transmitters
                .keys()
                .filter(|port| !self.transmitters.contains_key(port))
                .count();
        let routing_entries = match event.action {
            Action::ConvergeRoute(node) => {
                let old = self.routing_tables[node]
                    .as_ref()
                    .map_or(0, |table| table.routes.len());
                let new = effects.routing_table.as_ref().expect("staged routes").len();
                self.routing_entries - old + new
            }
            _ => self.routing_entries,
        };
        let stp_entries = effects
            .stp_update
            .as_ref()
            .map(|update| &update.tree)
            .or(self.stp_tree.as_ref())
            .map_or(0, StpTree::entries);
        if self.arp.len()
            + self.mac.len()
            + self.pending_count
            + self.transmitters.len()
            + routing_entries
            + stp_entries
            + new_entries
            > self.limits.state_entries
        {
            self.exhaust(ResourceLimit::StateEntries);
            return;
        }
        self.retain_observations(&mut effects);
        let link_facts = match event.action {
            Action::Link(index) => self.topology.link_actions[index].update_count(),
            _ => 0,
        };
        let facts = effects.observations.len()
            + link_facts
            + usize::from(effects.routing_table.is_some())
            + effects.stp_update.as_ref().map_or(0, StpUpdate::facts);
        if self.evidence.len()
            + self.link_facts
            + self.routing_events.len()
            + self.stp_facts
            + facts
            > self.limits.essential_facts
        {
            self.exhaust(ResourceLimit::EssentialFacts);
            return;
        }
        self.queue.pop();
        self.trace_event(point, event.action, &effects);
        self.commit(point, event.action, effects);
        self.work += cost;
        self.last = Some(point);
    }

    fn retain_observations(&self, effects: &mut Effects) {
        if self.topology.legacy || effects.observations.is_empty() {
            return;
        }
        let mut first_blocked = None;
        let mut other_blocked = None;
        effects.observations.retain(|observation| {
            if !observation.blocking {
                return true;
            }
            let probe = observation.probe;
            let state = &self.probes[probe];
            // ARP resolution clears that neighbor's provisional cause before new observations commit.
            if state.cause.is_some_and(|id| {
                state.neighbor.is_none()
                    || state.neighbor != effects.resolved
                    || !is_arp_cause(&self.evidence, id)
            }) {
                return false;
            }
            match first_blocked {
                None => {
                    first_blocked = Some(probe);
                    true
                }
                Some(first) => {
                    probe != first && other_blocked.get_or_insert_with(HashSet::new).insert(probe)
                }
            }
        });
    }

    fn start(&self, point: Point, probe: ProbeConfig) -> Result<Effects, StopReason> {
        let packet = IpPacket {
            source: self.topology.interface(probe.source).ipv4,
            destination: self.topology.interface(probe.destination).ipv4,
            ttl: probe.ttl,
            ipv4_total_bytes: ECHO_HEADERS_BYTES + probe.payload_bytes,
            probe: probe.id,
            reply: false,
        };
        let mut effects = Effects::default();
        self.route(point, probe.source, None, packet, &mut effects)?;
        Ok(effects)
    }

    fn warm(&self, point: Point, (port, ip): Neighbor) -> Result<Effects, StopReason> {
        let mut effects = Effects::default();
        self.send(
            point,
            port,
            self.arp_frame((port, ip), false, Mac::BROADCAST, None, None),
            &mut effects,
        )?;
        Ok(effects)
    }

    fn active_link(&self, pair: LinkProtection) -> Option<LinkIndex> {
        [pair.primary, pair.standby]
            .into_iter()
            .find(|&link| self.links[link].down_event.is_none())
    }

    fn adjust_forwarding_flood_work(&mut self, link: LinkIndex, active: bool) {
        // Every port is still inspected. Only a forwarding member runs its VLAN check.
        for index in self.topology.links[link].ports {
            let port = &self.topology.ports[index];
            let work = self
                .forwarding_flood_work
                .entry(port.device)
                .or_insert(self.topology.devices[port.device].flood_work);
            if active {
                *work += port.vlan_work();
            } else {
                *work -= port.vlan_work();
            }
        }
    }

    fn stp_forwards(&self, slot: usize) -> bool {
        self.stp_tree.as_ref().is_some_and(|tree| tree.active[slot])
    }

    fn required_branch(&self, port: PortIndex, wire: WireFrame, stage: PortStage) -> bool {
        let forest = self.stp_tree.as_ref().map(|tree| &tree.forest);
        self.topology
            .on_required_branch(forest, wire.origin, wire.neighbor, port, stage)
    }

    fn known_mac_port(&self, key: MacKey) -> Option<PortIndex> {
        let learned = &self.mac.get(&key)?.value;
        if learned.stp_generation != self.stp_mac_generation {
            return None;
        }
        let link = self.topology.ports[learned.port]
            .link
            .expect("learned on a link")
            .link;
        let valid = self.topology.links[link].protection.is_none()
            || learned.link_generation == self.links[link].generation;
        valid.then_some(learned.port)
    }

    fn invalidated(&self, arrival: Arrival) -> Option<LinkInvalidation> {
        self.in_flight[&(arrival.link, arrival.generation)].invalidated_by
    }

    fn receive(&self, point: Point, arrival: Arrival) -> Result<Effects, StopReason> {
        let mut effects = Effects::default();
        if let Some(loss) = self.invalidated(arrival) {
            let fact = || {
                let link_id = self.topology.links[arrival.link].id.clone();
                match loss {
                    LinkInvalidation::PhysicalDown(event) => EvidenceFact::InFlightLinkLoss {
                        link_id,
                        planned_arrival_ns: point.at_ns,
                        sent_generation: arrival.generation,
                        invalidated_by: event,
                    },
                    LinkInvalidation::SpanningTreeChanged(invalidation) => {
                        EvidenceFact::InFlightStpLoss {
                            link_id,
                            invalidated_at_ns: invalidation.at_ns,
                            planned_arrival_ns: point.at_ns,
                            sent_generation: arrival.generation,
                            invalidated_by: invalidation.id,
                        }
                    }
                    LinkInvalidation::ForwardingChanged(invalidation) => {
                        EvidenceFact::InFlightProtectionLoss {
                            link_id,
                            invalidated_at_ns: invalidation.at_ns,
                            planned_arrival_ns: point.at_ns,
                            sent_generation: arrival.generation,
                            invalidated_by: invalidation.id,
                        }
                    }
                }
            };
            self.observe_drop(
                arrival.port,
                arrival.wire,
                point.frame(),
                PortStage::Ingress,
                fact,
                &mut effects,
            );
            return Ok(effects);
        }
        match &self.topology.ports[arrival.port].kind {
            PortKind::Switch(mode) => self.receive_switch(point, arrival, mode, &mut effects)?,
            PortKind::Ip(interface) => {
                self.receive_interface(point, arrival, *interface, &mut effects)?
            }
        }
        Ok(effects)
    }

    fn receive_switch(
        &self,
        point: Point,
        arrival: Arrival,
        mode: &SwitchMode,
        effects: &mut Effects,
    ) -> Result<(), StopReason> {
        let frame = arrival.wire;
        let vlan = match ingress_vlan(mode, frame.tag) {
            Ok(vlan) => vlan,
            Err(reject) => {
                self.observe_drop(
                    arrival.port,
                    frame,
                    point.frame(),
                    PortStage::Ingress,
                    || reject.fact(PortStage::Ingress),
                    effects,
                );
                return Ok(());
            }
        };
        let device = self.topology.ports[arrival.port].device;
        if frame.stp_generation == self.stp_mac_generation {
            self.learn(
                point,
                Learning::Mac {
                    key: (device, vlan, frame.source_mac),
                    port: arrival.port,
                },
                effects,
            )?;
        }
        let known = self.known_mac_port((device, vlan, frame.destination_mac));
        let egress_ports = match &known {
            Some(port) => std::slice::from_ref(port),
            None => self.topology.devices[device].ports.as_slice(),
        };
        for &egress in egress_ports {
            let port = &self.topology.ports[egress];
            let Some(attachment) = port.link else {
                continue;
            };
            if egress == arrival.port {
                continue;
            }
            if let Some(slot) = self
                .topology
                .stp
                .as_ref()
                .and_then(|plan| plan.slot(attachment.link))
            {
                if !self.stp_forwards(slot) {
                    continue;
                }
                if let Some(up_event_id) = self.stp_recovered[slot] {
                    self.observe_drop(
                        egress,
                        frame,
                        point.frame(),
                        PortStage::Egress,
                        || EvidenceFact::StpAwaitingConvergence {
                            link_id: self.topology.links[attachment.link].id.clone(),
                            up_event_id,
                        },
                        effects,
                    );
                    continue;
                }
            }
            if let Some(group) = self.topology.links[attachment.link].protection {
                let pair = self.topology.protection[group];
                match self.active_link(pair) {
                    Some(active) if active == attachment.link => {}
                    None if attachment.link == pair.primary => {
                        self.observe_drop(
                            egress,
                            frame,
                            point.frame(),
                            PortStage::Egress,
                            || self.protection_unavailable(pair),
                            effects,
                        );
                        continue;
                    }
                    _ => continue,
                }
            }
            let PortKind::Switch(mode) = &port.kind else {
                unreachable!("switch port")
            };
            match egress_tag(mode, vlan) {
                Ok(tag) => self.send(
                    point,
                    egress,
                    WireFrame {
                        tag,
                        parent_id: Some(point.frame()),
                        ..frame
                    },
                    effects,
                )?,
                Err(reject) => self.observe_drop(
                    egress,
                    frame,
                    point.frame(),
                    PortStage::Egress,
                    || reject.fact(PortStage::Egress),
                    effects,
                ),
            }
        }
        Ok(())
    }

    fn receive_interface(
        &self,
        point: Point,
        arrival: Arrival,
        interface: Interface,
        effects: &mut Effects,
    ) -> Result<(), StopReason> {
        let wire = arrival.wire;
        if let Some(vlan) = wire.tag {
            self.observe_drop(
                arrival.port,
                wire,
                point.frame(),
                PortStage::Ingress,
                || EvidenceFact::Discarded {
                    stage: PortStage::Ingress,
                    reason: DropRule::UntaggedPortRejectsTag { vlan },
                },
                effects,
            );
            return Ok(());
        }
        match wire.packet {
            Packet::Arp { .. } => self.receive_arp(point, arrival, interface, effects),
            Packet::Ip(ip) if wire.destination_mac == interface.mac => {
                self.receive_ip(point, arrival, ip, effects)
            }
            _ => Ok(()),
        }
    }

    fn receive_arp(
        &self,
        point: Point,
        arrival: Arrival,
        interface: Interface,
        effects: &mut Effects,
    ) -> Result<(), StopReason> {
        let wire = arrival.wire;
        let Packet::Arp {
            sender,
            target,
            reply,
            probe,
        } = wire.packet
        else {
            unreachable!("ARP handler")
        };
        let matching_mac = wire.destination_mac == interface.mac
            || (!reply && wire.destination_mac == Mac::BROADCAST);
        if target != interface.ipv4
            || !matching_mac
            || (!self.topology.legacy && !interface.can_resolve(sender))
        {
            return Ok(());
        }
        let key = (arrival.port, sender);
        self.learn(
            point,
            Learning::Arp {
                key,
                mac: wire.source_mac,
            },
            effects,
        )?;
        if !reply {
            let mut response =
                self.arp_frame(key, true, wire.source_mac, probe, Some(point.frame()));
            response.routing_event = wire.routing_event;
            self.send(point, arrival.port, response, effects)?;
        }
        if let Some(waiting) = self.pending.get(&key) {
            effects.resolved = Some(key);
            for (&probe, pending) in waiting {
                if self.probes[probe].outcome.is_some() {
                    continue;
                }
                if !pending.packet.reply && self.topology.probes[probe].source == arrival.port {
                    effects.observe(
                        probe,
                        arrival.port,
                        wire.evidence(point.frame()),
                        EvidenceFact::ArpResolved,
                        false,
                    );
                }
                let resume = PendingPacket {
                    parent_id: pending.parent_id.or(Some(point.frame())),
                    ..*pending
                };
                self.send_ip(point, key, resume, wire.source_mac, effects)?;
            }
        }
        Ok(())
    }

    fn receive_ip(
        &self,
        point: Point,
        arrival: Arrival,
        mut ip: IpPacket,
        effects: &mut Effects,
    ) -> Result<(), StopReason> {
        let port = &self.topology.ports[arrival.port];
        let packet_frame = arrival.wire.evidence(point.frame());
        match &self.topology.devices[port.device].kind {
            DeviceKind::Host { .. } => {
                if ip.destination != self.topology.interface(arrival.port).ipv4 {
                    return Ok(());
                }
                let config = self.topology.probes[ip.probe];
                let (sender, receiver) = if ip.reply {
                    (config.destination, config.source)
                } else {
                    (config.source, config.destination)
                };
                if arrival.port != receiver || ip.source != self.topology.interface(sender).ipv4 {
                    return Ok(());
                }
                if self.probes[ip.probe].outcome.is_none() {
                    let fact = if ip.reply {
                        Some(EvidenceFact::EchoReplyReceived)
                    } else if self.topology.legacy {
                        None
                    } else {
                        Some(EvidenceFact::EchoRequestReceived)
                    };
                    if let Some(fact) = fact {
                        effects.observe(ip.probe, arrival.port, packet_frame, fact, false);
                    }
                }
                if ip.reply {
                    return Ok(());
                }
                ip = IpPacket {
                    source: ip.destination,
                    destination: ip.source,
                    ttl: DEFAULT_TTL,
                    reply: true,
                    ..ip
                };
            }
            DeviceKind::Router { .. } => {
                if self.acl_denies(
                    point,
                    arrival.port,
                    PortStage::Ingress,
                    ip,
                    packet_frame,
                    effects,
                ) {
                    return Ok(());
                }
                if ip.ttl <= 1 {
                    self.observe_ip_loss(
                        arrival.port,
                        ip,
                        packet_frame,
                        EvidenceFact::Ipv4TtlExpired {
                            received_ttl: ip.ttl,
                        },
                        effects,
                    );
                    return Ok(());
                }
                ip.ttl -= 1;
            }
            DeviceKind::Switch => unreachable!("IP port"),
        }
        self.route(point, arrival.port, Some(packet_frame), ip, effects)
    }

    fn dynamic_table(&self, device: DeviceIndex) -> Option<Option<&RoutingTable>> {
        let node = self.topology.routing.as_ref()?.node(device)?;
        // A participating router can receive traffic before its first publication.
        Some(self.routing_tables[node].as_ref())
    }

    fn route(
        &self,
        point: Point,
        from: PortIndex,
        incoming: Option<FrameEvidence>,
        packet: IpPacket,
        effects: &mut Effects,
    ) -> Result<(), StopReason> {
        let from_interface = self.topology.interface(from);
        let device = self.topology.ports[from].device;
        let routing = self.dynamic_table(device);
        let mut learned = None;
        let decision = match &self.topology.devices[device].kind {
            DeviceKind::Host { gateway } => {
                let next = if from_interface.on_link(packet.destination) {
                    Some(packet.destination)
                } else {
                    *gateway
                };
                next.map(|next| ((from, next), None))
            }
            DeviceKind::Router { routes } => {
                let configured = routes.iter().copied().find(|route| {
                    route.matches(packet.destination)
                        && (routing.is_none()
                            || route.next_hop.is_some()
                            || self.topology.ports[route.egress]
                                .link
                                .is_some_and(|link| self.links[link.link].down_event.is_none()))
                });
                learned = routing.flatten().and_then(|table| {
                    table
                        .routes
                        .iter()
                        .copied()
                        .find(|entry| entry.route.matches(packet.destination))
                        .filter(|entry| {
                            configured.is_none_or(|route| entry.route.length > route.length)
                        })
                        .map(|entry| (entry, table.event_id))
                });
                learned
                    .map(|(entry, _)| entry.route)
                    .or(configured)
                    .map(|route| {
                        (
                            (route.egress, route.next_hop.unwrap_or(packet.destination)),
                            Some((route.network, route.length)),
                        )
                    })
            }
            DeviceKind::Switch => unreachable!("IP source"),
        };
        let packet_kind = Packet::Ip(packet).kind();
        let frame = incoming
            .filter(|frame| frame.kind == packet_kind)
            .unwrap_or(FrameEvidence {
                id: point.frame(),
                parent_id: incoming.map(|frame| frame.id),
                kind: packet_kind,
            });
        let Some((key, prefix)) = decision else {
            self.observe_ip_loss(
                from,
                packet,
                frame,
                if let Some(table) = routing {
                    let reason = if table.is_none() {
                        DynamicRouteMissReason::AwaitingInitialConvergence
                    } else if self.topology.probes[packet.probe].advertised_destinations
                        [usize::from(packet.reply)]
                    {
                        DynamicRouteMissReason::NoPathToAdvertisement
                    } else {
                        DynamicRouteMissReason::NoAdvertisedPrefix
                    };
                    EvidenceFact::DynamicRouteMiss {
                        destination: packet.destination,
                        request_evidence_id: None,
                        reason,
                        convergence_event_id: table.map(|table| table.event_id),
                    }
                } else {
                    EvidenceFact::Ipv4RouteMiss {
                        destination: packet.destination,
                        request_evidence_id: None,
                    }
                },
                effects,
            );
            return Ok(());
        };
        effects.route = Some(RouteChoice {
            key,
            destination: packet.destination,
            prefix,
            ipv4_mtu: None,
            learned,
        });
        if self.acl_denies(point, key.0, PortStage::Egress, packet, frame, effects) {
            return Ok(());
        }
        let egress = self.topology.interface(key.0);
        if !self.topology.legacy && !egress.can_resolve(key.1) {
            self.observe_ip_loss(
                from,
                packet,
                frame,
                EvidenceFact::Ipv4NeighborNotUnicast { next_hop: key.1 },
                effects,
            );
            return Ok(());
        }
        if let Some(mtu_bytes) = egress.ipv4_mtu_bytes {
            let ipv4_total_bytes = packet.ipv4_total_bytes;
            let passed = ipv4_total_bytes <= mtu_bytes;
            effects.route.as_mut().expect("selected route").ipv4_mtu = Some(TraceMtuCheck {
                packet_kind,
                ipv4_total_bytes,
                mtu_bytes,
                passed,
            });
            if !passed {
                // An MTU decision happens before creating an outgoing frame.
                self.observe_ip_loss(
                    key.0,
                    packet,
                    incoming.unwrap_or(frame),
                    EvidenceFact::Ipv4MtuExceeded {
                        event_id: point.id,
                        packet_kind,
                        ipv4_total_bytes,
                        mtu_bytes,
                    },
                    effects,
                );
                return Ok(());
            }
        }
        let pending = PendingPacket {
            packet,
            parent_id: incoming.map(|frame| frame.id),
            routing_event: learned.and_then(|(_, id)| NonZeroU64::new(id.0)),
        };
        if let Some(entry) = self.arp.get(&key) {
            self.send_ip(point, key, pending, entry.value, effects)?;
        } else {
            if self.probes[packet.probe].outcome.is_none() {
                effects.pending = Some((key, pending));
            }
            let mut request = self.arp_frame(
                key,
                false,
                Mac::BROADCAST,
                Some(packet.probe),
                pending.parent_id,
            );
            request.routing_event = pending.routing_event;
            self.send(point, key.0, request, effects)?;
        }
        Ok(())
    }

    fn acl_denies(
        &self,
        point: Point,
        port: PortIndex,
        direction: PortStage,
        packet: IpPacket,
        frame: FrameEvidence,
        effects: &mut Effects,
    ) -> bool {
        let Some((action, matched_rule)) = self.topology.ports[port]
            .ipv4_acl
            .as_ref()
            .and_then(|acl| acl.decide(direction, packet.source))
        else {
            return false;
        };
        let denied = action == crate::Ipv4AclAction::Deny;
        if self.probes[packet.probe].outcome.is_none() {
            effects.observe(
                packet.probe,
                port,
                frame,
                EvidenceFact::Ipv4AclDecision {
                    event_id: point.id,
                    direction,
                    source: packet.source,
                    destination: packet.destination,
                    action,
                    matched_rule,
                },
                denied,
            );
        }
        denied
    }

    fn observe_ip_loss(
        &self,
        port: PortIndex,
        packet: IpPacket,
        frame: FrameEvidence,
        fact: EvidenceFact,
        effects: &mut Effects,
    ) {
        if self.probes[packet.probe].outcome.is_none() {
            effects.observe(packet.probe, port, frame, fact, true);
        }
    }

    fn send_ip(
        &self,
        point: Point,
        (port, neighbor): Neighbor,
        pending: PendingPacket,
        mac: Mac,
        effects: &mut Effects,
    ) -> Result<(), StopReason> {
        let packet = pending.packet;
        if !packet.reply && self.topology.probes[packet.probe].source == port {
            effects.request_sent.push(packet.probe);
        }
        let wire = WireFrame {
            source_mac: self.topology.interface(port).mac,
            destination_mac: mac,
            packet: Packet::Ip(packet),
            tag: None,
            origin: port,
            neighbor,
            parent_id: pending.parent_id,
            stp_generation: self.stp_mac_generation,
            required_at_send: None,
            routing_event: pending.routing_event,
        };
        self.send(point, port, wire, effects)
    }

    fn arp_frame(
        &self,
        (port, neighbor): Neighbor,
        reply: bool,
        destination_mac: Mac,
        probe: Option<ProbeIndex>,
        parent_id: Option<FrameId>,
    ) -> WireFrame {
        let interface = self.topology.interface(port);
        WireFrame {
            source_mac: interface.mac,
            destination_mac,
            packet: Packet::Arp {
                sender: interface.ipv4,
                target: neighbor,
                reply,
                probe,
            },
            tag: None,
            origin: port,
            neighbor,
            parent_id,
            stp_generation: self.stp_mac_generation,
            required_at_send: None,
            routing_event: None,
        }
    }

    fn send(
        &self,
        point: Point,
        port: PortIndex,
        mut wire: WireFrame,
        effects: &mut Effects,
    ) -> Result<(), StopReason> {
        let Some(attachment) = self.topology.ports[port].link else {
            return Ok(());
        };
        let state = self.links[attachment.link];
        let link = &self.topology.links[attachment.link];
        if let Some(down_event_id) = state.down_event {
            self.observe_drop(
                port,
                wire,
                point.frame(),
                PortStage::Egress,
                || EvidenceFact::LinkDown {
                    link_id: link.id.clone(),
                    down_event_id,
                },
                effects,
            );
            return Ok(());
        }
        if self.stp_tree.is_some() {
            wire.required_at_send = Some(
                wire.required_at_send.unwrap_or(true)
                    && self.required_branch(port, wire, PortStage::Egress),
            );
        }
        let arrival = Arrival {
            port: attachment.peer,
            wire,
            link: attachment.link,
            generation: state.generation,
        };
        if let Some(config) = link.transmission {
            self.transmit(point, port, arrival, config, effects)?;
        } else {
            let at_ns = point
                .at_ns
                .checked_add(link.delay_ns)
                .ok_or(StopReason::TimeOverflow)?;
            if at_ns <= self.topology.end_ns {
                effects.frames.push(ScheduledFrame {
                    arrival,
                    arrives_at_ns: at_ns,
                    transmit_at_ns: None,
                });
            }
        }
        Ok(())
    }

    fn transmit(
        &self,
        point: Point,
        port: PortIndex,
        arrival: Arrival,
        config: Transmission,
        effects: &mut Effects,
    ) -> Result<(), StopReason> {
        let wire = arrival.wire;
        let frame_bytes = wire.mac_bytes();
        let transmitter = effects
            .transmitters
            .entry(port)
            .or_insert_with(|| self.transmitters.get(&port).copied().unwrap_or_default());
        let link = &self.topology.links[arrival.link];
        let waiting = transmitter.busy_until_ns > point.at_ns;
        if waiting && transmitter.waiting == config.queue_frames {
            self.observe_drop(
                port,
                wire,
                point.frame(),
                PortStage::Egress,
                || EvidenceFact::TransmitQueueFull {
                    event_id: point.id,
                    link_id: link.id.clone(),
                    rate_bps: config.rate_bps.get(),
                    queue_frames: config.queue_frames,
                    frame_bytes,
                },
                effects,
            );
            return Ok(());
        }
        let start = point.at_ns.max(transmitter.busy_until_ns);
        let duration = (u64::from(frame_bytes) * 8_000_000_000).div_ceil(config.rate_bps.get());
        let finish = start
            .checked_add(duration)
            .ok_or(StopReason::TimeOverflow)?;
        let arrives_at_ns = finish
            .checked_add(link.delay_ns)
            .ok_or(StopReason::TimeOverflow)?;
        transmitter.busy_until_ns = finish;
        transmitter.waiting += u16::from(waiting);
        effects.frames.push(ScheduledFrame {
            arrival,
            arrives_at_ns,
            transmit_at_ns: waiting.then_some(start),
        });
        Ok(())
    }

    fn observe_drop(
        &self,
        port: PortIndex,
        wire: WireFrame,
        id: FrameId,
        stage: PortStage,
        fact: impl FnOnce() -> EvidenceFact,
        effects: &mut Effects,
    ) {
        let Some(index) = wire.packet.probe() else {
            return;
        };
        if self.probes[index].outcome.is_some() {
            return;
        }
        let waiting = match wire.packet {
            Packet::Arp {
                sender,
                target,
                reply,
                ..
            } => {
                let key = if reply {
                    self.topology
                        .addresses
                        .get(&target)
                        .map(|&port| (port, sender))
                } else {
                    Some((wire.origin, target))
                };
                key.is_some_and(|key| {
                    effects.resolved != Some(key)
                        && (self.probes[index].neighbor == Some(key)
                            || effects.pending.is_some_and(|(pending, packet)| {
                                pending == key && packet.packet.probe == index
                            }))
                })
            }
            Packet::Ip(_) => true,
        };
        let required = match stage {
            PortStage::Ingress => wire
                .required_at_send
                .unwrap_or_else(|| self.required_branch(port, wire, stage)),
            PortStage::Egress => {
                wire.required_at_send.unwrap_or(true) && self.required_branch(port, wire, stage)
            }
        };
        let blocking = waiting && required;
        if blocking || self.topology.legacy {
            effects.observations.push(Observation {
                probe: index,
                port,
                frame: wire.evidence(id),
                fact: fact(),
                blocking,
                routing_event: wire.routing_event,
            });
        }
    }

    fn learn(
        &self,
        point: Point,
        learned: Learning,
        effects: &mut Effects,
    ) -> Result<(), StopReason> {
        let (key, lifetime) = match learned {
            Learning::Mac { key, .. } => (TableKey::Mac(key), self.topology.mac_lifetime_ns),
            Learning::Arp { key, .. } => (TableKey::Arp(key), self.topology.arp_lifetime_ns),
        };
        effects.learning = Some(learned);
        if let Some(lifetime) = lifetime {
            let at_ns = point
                .at_ns
                .checked_add(lifetime)
                .ok_or(StopReason::TimeOverflow)?;
            if at_ns <= self.topology.end_ns {
                effects.expiry = Some((
                    at_ns,
                    Action::Expire {
                        key,
                        generation: point.id,
                    },
                ));
            }
        }
        Ok(())
    }

    fn protected_links(&self, pair: LinkProtection) -> ProtectedLinks {
        ProtectedLinks {
            primary_link_id: self.topology.links[pair.primary].id.clone(),
            standby_link_id: self.topology.links[pair.standby].id.clone(),
        }
    }

    fn protection_unavailable(&self, pair: LinkProtection) -> EvidenceFact {
        EvidenceFact::ProtectionUnavailable {
            primary_link_id: self.topology.links[pair.primary].id.clone(),
            primary_down_event_id: self.links[pair.primary]
                .down_event
                .expect("primary is down"),
            standby_link_id: self.topology.links[pair.standby].id.clone(),
            standby_down_event_id: self.links[pair.standby]
                .down_event
                .expect("standby is down"),
        }
    }

    fn invalidate_link(&mut self, link: LinkIndex, loss: LinkInvalidation) {
        let state = &mut self.links[link];
        if let Some(frames) = self.in_flight.get_mut(&(link, state.generation)) {
            frames.invalidated_by = Some(loss);
        }
        state.generation += 1;
        if self.topology.links[link].transmission.is_some() {
            for port in self.topology.links[link].ports {
                self.transmitters.remove(&port);
            }
        }
    }

    fn commit_link_change(&mut self, point: Point, change: LinkChange) {
        let group = self.topology.links[change.first.link].protection;
        let previous = group.and_then(|group| self.active_link(self.topology.protection[group]));
        for update in change.updates() {
            if let Some(slot) = self
                .topology
                .stp
                .as_ref()
                .and_then(|plan| plan.slot(update.link))
                && self.stp_forwards(slot)
            {
                let held = self.stp_recovered[slot].is_some();
                let recovering = update.state == LinkStatus::Up;
                self.stp_recovered[slot] = recovering.then_some(point.id);
                if held != recovering {
                    self.adjust_forwarding_flood_work(update.link, !recovering);
                }
            }
            self.links[update.link].down_event = match update.state {
                LinkStatus::Down => {
                    self.invalidate_link(update.link, LinkInvalidation::PhysicalDown(point.id));
                    Some(point.id)
                }
                LinkStatus::Up => None,
            };
        }
        let protection = group.map(|group| {
            let pair = self.topology.protection[group];
            let active = self.active_link(pair);
            if previous != active {
                if let Some(previous) = previous {
                    self.adjust_forwarding_flood_work(previous, false);
                    if self.links[previous].down_event.is_none() {
                        self.invalidate_link(previous, LinkInvalidation::ForwardingChanged(point));
                    }
                }
                if let Some(active) = active {
                    self.adjust_forwarding_flood_work(active, true);
                }
                self.protection_selections[group] = Some(point.id);
            }
            let id = |link: LinkIndex| self.topology.links[link].id.clone();
            ProtectionChange {
                links: self.protected_links(pair),
                previous_active_link_id: previous.map(id),
                active_link_id: active.map(id),
            }
        });
        let first = change.first;
        self.link_events.push(LinkEvent {
            event_id: point.id,
            link_id: self.topology.links[first.link].id.clone(),
            at_ns: point.at_ns,
            state: first.state,
            generation: self.links[first.link].generation,
            simultaneous: change.second.map(|update| LinkMemberChange {
                link_id: self.topology.links[update.link].id.clone(),
                state: update.state,
                generation: self.links[update.link].generation,
            }),
            protection,
        });
        self.link_facts += change.update_count();
    }

    fn commit_stp(&mut self, point: Point, update: StpUpdate) {
        for (slot, &active) in update.tree.active.iter().enumerate() {
            let previous = self.stp_forwards(slot);
            let held = self.stp_recovered[slot].take().is_some();
            let link = self.topology.stp.as_ref().unwrap().link_at(slot);
            if (previous && !held) != active {
                self.adjust_forwarding_flood_work(link, active);
            }
            if previous == active {
                continue;
            }
            if !active && self.links[link].down_event.is_none() {
                self.invalidate_link(link, LinkInvalidation::SpanningTreeChanged(point));
            }
            // A port that remains forwarding can now lead away from a learned destination.
            self.stp_mac_generation = point.id.0;
        }
        self.stp_facts += update.facts();
        self.stp_tree = Some(update.tree);
        self.stp_events
            .push(update.event, update.bridges, update.links);
    }

    fn commit(&mut self, point: Point, action: Action, mut effects: Effects) {
        match action {
            Action::Link(index) => {
                self.commit_link_change(point, self.topology.link_actions[index]);
                if let Some(round) = effects.routing_round {
                    self.schedule_routing_round(round);
                }
                if let Some(round) = effects.stp_round {
                    self.schedule_stp_round(round);
                }
            }
            Action::ConvergeStp => self.commit_stp(
                point,
                effects.stp_update.take().expect("staged spanning tree"),
            ),
            Action::ConvergeRoute(node) => {
                let plan = self.topology.routing.as_ref().expect("routing plan");
                let routes = effects.routing_table.take().expect("staged routing table");
                reserve_history_append(&mut self.routing_events);
                self.routing_events.push(RoutingEvent {
                    event_id: point.id,
                    at_ns: point.at_ns,
                    router_id: self.topology.devices[plan.nodes[node].device].id.clone(),
                    triggered_by_link_event_id: self
                        .routing_round
                        .expect("scheduled routing round")
                        .trigger,
                    installed_routes: routes.len(),
                });
                self.routing_entries += routes.len();
                let previous = self.routing_tables[node].replace(RoutingTable {
                    event_id: point.id,
                    routes,
                });
                self.routing_entries -= previous.map_or(0, |table| table.routes.len());
            }
            Action::Deadline(probe) if self.probes[probe].outcome.is_none() => {
                let state = &mut self.probes[probe];
                state.outcome = Some(Outcome::Timeout {
                    at_ns: point.at_ns,
                    waiting_for: state.waiting_for,
                    cause: state.cause,
                });
                if let Some(key) = state.neighbor.take() {
                    let entries = self.pending.get_mut(&key).expect("pending probe neighbor");
                    entries.remove(&probe);
                    self.pending_count -= 1;
                    if entries.is_empty() {
                        self.pending.remove(&key);
                    }
                }
            }
            Action::Transmit {
                arrival,
                arrives_at_ns,
                arrival_order,
            } => {
                if self.invalidated(arrival).is_none() {
                    let port = self.topology.ports[arrival.port]
                        .link
                        .expect("attached")
                        .peer;
                    self.transmitters
                        .get_mut(&port)
                        .expect("queued transmitter")
                        .waiting -= 1;
                }
                // An invalid generation still reaches the original loss observation point.
                self.schedule_reserved(arrives_at_ns, Action::Arrival(arrival), arrival_order);
            }
            Action::Arrival(arrival) => {
                let key = (arrival.link, arrival.generation);
                let frames = self
                    .in_flight
                    .get_mut(&key)
                    .expect("arrivals retain a link generation");
                frames.remaining -= 1;
                if frames.remaining == 0 {
                    self.in_flight.remove(&key);
                }
            }
            Action::Expire { key, generation } => match key {
                TableKey::Mac(key) => expire(&mut self.mac, key, generation),
                TableKey::Arp(key) => expire(&mut self.arp, key, generation),
            },
            _ => {}
        }
        if let Some((at_ns, action)) = effects.expiry {
            self.schedule(at_ns, action);
        }
        match effects.learning {
            Some(Learning::Mac { key, port }) => {
                let link = self.topology.ports[port]
                    .link
                    .expect("learned on a link")
                    .link;
                self.mac.insert(
                    key,
                    Entry {
                        value: LearnedMac {
                            port,
                            link_generation: self.links[link].generation,
                            stp_generation: self.stp_mac_generation,
                        },
                        generation: point.id,
                    },
                );
            }
            Some(Learning::Arp { key, mac }) => {
                self.arp.insert(
                    key,
                    Entry {
                        value: mac,
                        generation: point.id,
                    },
                );
            }
            None => {}
        }
        if let Some(key) = effects.resolved {
            let resolved = self
                .pending
                .remove(&key)
                .expect("resolution has waiting packets");
            self.pending_count -= resolved.len();
            for (probe, _) in resolved {
                let state = &mut self.probes[probe];
                state.neighbor = None;
                if state
                    .cause
                    .is_some_and(|id| is_arp_cause(&self.evidence, id))
                {
                    state.cause = None;
                }
            }
        }
        if let Some((key, pending)) = effects.pending {
            let probe = pending.packet.probe;
            self.probes[probe].neighbor = Some(key);
            let previous = self.pending.entry(key).or_default().insert(probe, pending);
            debug_assert!(previous.is_none(), "one live IP packet per probe");
            self.pending_count += 1;
        }
        for probe in effects.request_sent {
            self.probes[probe].waiting_for = WaitingFor::EchoReply;
        }
        self.transmitters.extend(effects.transmitters);
        for frame in effects.frames {
            let arrival = frame.arrival;
            self.in_flight
                .entry((arrival.link, arrival.generation))
                .or_default()
                .remaining += 1;
            if let Some(at_ns) = frame.transmit_at_ns {
                // Arrival ties follow admission order even for a frame still in the FIFO.
                self.sequence += 1;
                self.schedule(
                    at_ns,
                    Action::Transmit {
                        arrival,
                        arrives_at_ns: frame.arrives_at_ns,
                        arrival_order: self.sequence,
                    },
                );
            } else {
                self.schedule(frame.arrives_at_ns, Action::Arrival(arrival));
            }
        }
        for mut observation in effects.observations {
            let state = &mut self.probes[observation.probe];
            let id = EvidenceId(self.evidence.len() as u64 + 1);
            match &mut observation.fact {
                EvidenceFact::EchoRequestReceived => state.request = Some(id),
                EvidenceFact::EchoReplyReceived => {
                    state.outcome = Some(Outcome::Reachable {
                        at_ns: point.at_ns,
                        evidence_id: id,
                    })
                }
                EvidenceFact::Ipv4RouteMiss {
                    request_evidence_id,
                    ..
                }
                | EvidenceFact::DynamicRouteMiss {
                    request_evidence_id,
                    ..
                } => *request_evidence_id = state.request,
                EvidenceFact::Ipv4MtuExceeded { .. }
                    if state.waiting_for == WaitingFor::ArpReply =>
                {
                    state.waiting_for = WaitingFor::ProbeDeadline;
                }
                _ => {}
            }
            if observation.blocking && state.cause.is_none() {
                state.cause = Some(id);
            }
            let protected_link = match observation.fact {
                EvidenceFact::Discarded { .. } => self.topology.ports[observation.port]
                    .link
                    .and_then(|attachment| {
                        let link = &self.topology.links[attachment.link];
                        link.protection.map(|group| ProtectedLinkEvidence {
                            link_id: link.id.clone(),
                            selected_by: self.protection_selections[group],
                        })
                    }),
                _ => None,
            };
            let stp_event_id = match observation.fact {
                EvidenceFact::Discarded { .. }
                | EvidenceFact::TransmitQueueFull { .. }
                | EvidenceFact::StpAwaitingConvergence { .. } => self.topology.ports
                    [observation.port]
                    .link
                    .and_then(|attachment| {
                        self.topology.stp.as_ref()?.slot(attachment.link)?;
                        self.stp_tree.as_ref().map(|tree| tree.event_id)
                    }),
                _ => None,
            };
            self.evidence.push((
                observation.probe,
                Evidence {
                    id,
                    at_ns: point.at_ns,
                    port: self.topology.port_reference(observation.port),
                    frame: observation.frame,
                    fact: observation.fact,
                    protected_link,
                    stp_event_id,
                    routing_event_id: observation.routing_event.map(|id| EventId(id.get())),
                    model_only: false,
                },
            ));
        }
    }

    fn trace_event(&mut self, point: Point, action: Action, effects: &Effects) {
        if self.trace.status == TraceStatus::Disabled {
            return;
        }
        if self.trace.entries.len() == self.trace_limit {
            self.trace.status = TraceStatus::Truncated;
            return;
        }
        let (event, frame, port, probe) = match action {
            Action::Link(_) => (TraceEvent::LinkChange, None, None, None),
            Action::ConvergeRoute(_) => (TraceEvent::RoutingConvergence, None, None, None),
            Action::ConvergeStp => (TraceEvent::StpConvergence, None, None, None),
            Action::Expire { .. } => (TraceEvent::TableExpiry, None, None, None),
            Action::Deadline(probe) => (TraceEvent::ProbeDeadline, None, None, Some(probe)),
            Action::Transmit { arrival, .. } => (
                if self.invalidated(arrival).is_none() {
                    TraceEvent::TransmitStart
                } else {
                    TraceEvent::TransmitCancelled
                },
                None,
                self.topology.ports[arrival.port].link.map(|link| link.peer),
                arrival.wire.packet.probe(),
            ),
            Action::Arrival(arrival) => (
                TraceEvent::FrameArrival,
                Some(arrival.wire.evidence(point.frame())),
                Some(arrival.port),
                arrival.wire.packet.probe(),
            ),
            Action::Start(probe) => (
                TraceEvent::ProbeStart,
                None,
                Some(self.topology.probes[probe].source),
                Some(probe),
            ),
            Action::Warm((port, _)) => (TraceEvent::NeighborWarmup, None, Some(port), None),
        };
        let (network, prefix_length) = effects.route.and_then(|route| route.prefix).unzip();
        self.trace.entries.push(TraceEntry {
            event_id: point.id,
            at_ns: point.at_ns,
            event,
            frame,
            port: port.map(|port| self.topology.port_reference(port)),
            probe_id: probe.map(|probe| self.topology.probe_names[probe].clone()),
            learning: effects.learning.map(|fact| match fact {
                Learning::Mac {
                    key: (_, vlan, _), ..
                } => TraceLearning::Mac { vlan },
                Learning::Arp {
                    key: (_, neighbor), ..
                } => TraceLearning::Arp { neighbor },
            }),
            route: effects.route.map(|route| TraceRoute {
                egress_port_id: self.topology.ports[route.key.0].id.clone(),
                destination: route.destination,
                next_hop: route.key.1,
                network,
                prefix_length,
                ipv4_mtu: route.ipv4_mtu,
                learned: route
                    .learned
                    .map(|(entry, convergence_event_id)| TraceLearnedRoute {
                        origin_router_id: self.topology.devices[entry.origin].id.clone(),
                        cost: entry.cost,
                        convergence_event_id,
                    }),
            }),
        });
    }

    fn outcome(&self, probe: ProbeIndex) -> Outcome {
        self.probes[probe]
            .outcome
            .unwrap_or_else(|| Outcome::Incomplete {
                at_ns: self.last.map_or(0, |point| point.at_ns),
                reason: self.stop.expect("unterminated probe needs a stop reason"),
            })
    }

    fn report_outcome(&self, probe: ProbeIndex) -> Outcome {
        let modeled = self.outcome(probe);
        let config = self.topology.probes[probe];
        self.topology
            .coverage
            .as_ref()
            .and_then(|coverage| {
                coverage.reference(
                    self.topology.ports[config.source].device,
                    self.topology.ports[config.destination].device,
                )
            })
            .map_or(modeled, |coverage| Outcome::Undetermined {
                modeled: modeled.execution(),
                coverage,
            })
    }

    pub(crate) fn report(self) -> RunReport {
        if self.topology.legacy {
            let outcome = self.outcome(0);
            let probe = self.topology.probe(self.topology.probes[0]);
            return RunReport::Legacy(Report {
                report_version: 1,
                model_version: 1,
                topology_id: self.topology.id,
                revision: self.topology.revision,
                probe,
                outcome,
                evidence: self.evidence.into_iter().map(|(_, fact)| fact).collect(),
            });
        }
        let transmissions: Vec<_> = self
            .topology
            .links
            .iter()
            .filter_map(|link| {
                link.transmission.map(|config| LinkTransmissionSettings {
                    link_id: link.id.clone(),
                    rate_bps: config.rate_bps.get(),
                    queue_frames: config.queue_frames,
                })
            })
            .collect();
        let probes = self
            .topology
            .probes
            .iter()
            .map(|&probe| ProbeReport {
                id: self.topology.probe_names[probe.id].clone(),
                probe: self.topology.probe(probe),
                at_ns: probe.at_ns,
                ttl: probe.ttl,
                payload_bytes: (self.topology.mtu_model || !transmissions.is_empty())
                    .then_some(probe.payload_bytes),
                icmp: IcmpIdentity {
                    identifier: 1,
                    sequence: (probe.id + 1) as u16,
                },
                outcome: self.report_outcome(probe.id),
                request_evidence_id: self.probes[probe.id].request,
            })
            .collect();
        let protected_links: Vec<_> = self
            .topology
            .protection
            .iter()
            .map(|&pair| self.protected_links(pair))
            .collect();
        let has_acls = self.topology.has_ipv4_acls();
        let evidence = self
            .evidence
            .into_iter()
            .map(|(probe, mut evidence)| {
                let config = self.topology.probes[probe];
                evidence.model_only = self.topology.coverage.as_ref().is_some_and(|plan| {
                    plan.reference(
                        self.topology.ports[config.source].device,
                        self.topology.ports[config.destination].device,
                    )
                    .is_some()
                });
                ScenarioEvidence {
                    probe_id: self.topology.probe_names[probe].clone(),
                    evidence,
                }
            })
            .collect();
        let routing = self
            .topology
            .routing
            .map(RoutingPlan::into_settings)
            .map(Box::new);
        let stp = self.topology.stp.as_ref().map(|plan| StpSettings {
            convergence_ns: plan.convergence_ns,
        });
        let version = if has_acls {
            9
        } else if self.topology.format_version == 3 {
            8
        } else if stp.is_some() {
            7
        } else if routing.is_some() {
            6
        } else if !transmissions.is_empty() {
            5
        } else if self.topology.mtu_model {
            4
        } else if protected_links.is_empty() {
            2
        } else {
            3
        };
        RunReport::Scenario(ScenarioReport {
            report_version: version,
            model_version: if has_acls {
                13
            } else if self.topology.native_vlan_count != 0 {
                12
            } else if self.topology.format_version == 3 {
                11
            } else if routing.is_some() {
                10
            } else if stp.is_some() {
                8
            } else {
                version
            },
            topology_id: self.topology.id,
            revision: self.topology.revision,
            settings: ScenarioSettings {
                end_ns: self.topology.end_ns,
                arp_lifetime_ns: self
                    .topology
                    .arp_lifetime_ns
                    .expect("scenario ARP lifetime"),
                mac_lifetime_ns: self
                    .topology
                    .mac_lifetime_ns
                    .expect("scenario MAC lifetime"),
                default_reply_ttl: DEFAULT_TTL,
                ipv4_dont_fragment: self.topology.mtu_model.then_some(true),
                limits: self.limits,
                protected_links,
                transmissions,
                routing,
                stp,
            },
            probes,
            evidence,
            link_events: self.link_events,
            routing_events: self.routing_events,
            stp_events: self.stp_events,
            trace: self.trace,
            last_event_ns: self.last.map_or(0, |point| point.at_ns),
            last_event_id: self.last.map(|point| point.id),
            stop_reason: self.stop,
            import_source: self.topology.import_source,
            coverage: self.topology.coverage.map(|plan| plan.report),
        })
    }
}

fn is_arp_cause(evidence: &[(ProbeIndex, Evidence)], id: EvidenceId) -> bool {
    matches!(
        evidence[(id.0 - 1) as usize].1.frame.kind,
        FrameKind::ArpRequest | FrameKind::ArpReply
    )
}

fn expire<K: Eq + std::hash::Hash, V>(
    table: &mut HashMap<K, Entry<V>>,
    key: K,
    generation: EventId,
) {
    if table
        .get(&key)
        .is_some_and(|entry| entry.generation == generation)
    {
        table.remove(&key);
    }
}

enum Rejection<'a> {
    TrunkVlanNotAllowed { vlan: u16, allowed: &'a [u16] },
    TrunkRequiresTag,
    TaggedAccess { vlan: u16 },
    AccessVlanMismatch { vlan: u16, access_vlan: u16 },
}

impl Rejection<'_> {
    fn fact(self, stage: PortStage) -> EvidenceFact {
        let reason = match self {
            Self::TrunkVlanNotAllowed { vlan, allowed } => DropRule::TrunkVlanNotAllowed {
                vlan,
                allowed_vlans: allowed.to_vec(),
            },
            Self::TrunkRequiresTag => DropRule::TrunkRequiresTag,
            Self::TaggedAccess { vlan } => DropRule::UntaggedPortRejectsTag { vlan },
            Self::AccessVlanMismatch { vlan, access_vlan } => {
                DropRule::AccessVlanMismatch { vlan, access_vlan }
            }
        };
        EvidenceFact::Discarded { stage, reason }
    }
}

fn ingress_vlan(mode: &SwitchMode, tag: Option<u16>) -> Result<u16, Rejection<'_>> {
    match (mode, tag) {
        (SwitchMode::Access { vlan }, None) => Ok(*vlan),
        (SwitchMode::Access { .. }, Some(vlan)) => Err(Rejection::TaggedAccess { vlan }),
        (
            SwitchMode::Trunk {
                allowed_vlans,
                native_vlan,
            },
            tag,
        ) => {
            let vlan = tag.or(*native_vlan).ok_or(Rejection::TrunkRequiresTag)?;
            require_trunk_vlan(allowed_vlans, vlan)?;
            Ok(vlan)
        }
    }
}

fn egress_tag(mode: &SwitchMode, vlan: u16) -> Result<Option<u16>, Rejection<'_>> {
    match mode {
        SwitchMode::Access { vlan: access_vlan } if *access_vlan == vlan => Ok(None),
        SwitchMode::Access { vlan: access_vlan } => Err(Rejection::AccessVlanMismatch {
            vlan,
            access_vlan: *access_vlan,
        }),
        SwitchMode::Trunk {
            allowed_vlans,
            native_vlan,
        } => {
            require_trunk_vlan(allowed_vlans, vlan)?;
            Ok((*native_vlan != Some(vlan)).then_some(vlan))
        }
    }
}

fn require_trunk_vlan(allowed: &[u16], vlan: u16) -> Result<(), Rejection<'_>> {
    if allowed.binary_search(&vlan).is_err() {
        return Err(Rejection::TrunkVlanNotAllowed { vlan, allowed });
    }
    Ok(())
}
