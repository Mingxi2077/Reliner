use std::{
    collections::{HashMap, HashSet},
    net::Ipv4Addr,
};

use serde::{Deserialize, Deserializer, Serialize, Serializer, de, ser::SerializeSeq};

pub(crate) fn decimal<S: Serializer>(value: &u64, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.collect_str(value)
}

pub(crate) fn read_decimal<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u64, D::Error> {
    let value = String::deserialize(deserializer)?;
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(de::Error::custom("64 位整数须为十进制整数字符串。"));
    }
    value.parse().map_err(de::Error::custom)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(transparent)]
pub struct EvidenceId(
    #[serde(serialize_with = "decimal", deserialize_with = "read_decimal")] pub u64,
);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(transparent)]
pub struct FrameId(#[serde(serialize_with = "decimal", deserialize_with = "read_decimal")] pub u64);

#[derive(Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Report {
    pub report_version: u32,
    pub model_version: u32,
    pub topology_id: String,
    pub revision: String,
    pub probe: Probe,
    pub outcome: Outcome,
    pub evidence: Vec<Evidence>,
}

impl Report {
    pub fn cause(&self) -> Option<&Evidence> {
        let Outcome::Timeout {
            cause: Some(id), ..
        } = self.outcome
        else {
            return None;
        };
        self.evidence.iter().find(|fact| fact.id == id)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct Probe {
    pub source: Endpoint,
    pub destination: Endpoint,
    #[serde(serialize_with = "decimal", deserialize_with = "read_decimal")]
    pub deadline_ns: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct Endpoint {
    pub device_id: String,
    pub name: String,
    pub ipv4: Ipv4Addr,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Outcome {
    Reachable {
        #[serde(serialize_with = "decimal", deserialize_with = "read_decimal")]
        at_ns: u64,
        evidence_id: EvidenceId,
    },
    Timeout {
        #[serde(serialize_with = "decimal", deserialize_with = "read_decimal")]
        at_ns: u64,
        waiting_for: WaitingFor,
        cause: Option<EvidenceId>,
    },
    Incomplete {
        #[serde(serialize_with = "decimal", deserialize_with = "read_decimal")]
        at_ns: u64,
        reason: StopReason,
    },
    Undetermined {
        modeled: ModeledOutcome,
        coverage: crate::CoverageRef,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ModeledOutcome {
    Reachable {
        #[serde(serialize_with = "decimal", deserialize_with = "read_decimal")]
        at_ns: u64,
        evidence_id: EvidenceId,
    },
    Timeout {
        #[serde(serialize_with = "decimal", deserialize_with = "read_decimal")]
        at_ns: u64,
        waiting_for: WaitingFor,
        cause: Option<EvidenceId>,
    },
    Incomplete {
        #[serde(serialize_with = "decimal", deserialize_with = "read_decimal")]
        at_ns: u64,
        reason: StopReason,
    },
}

impl Outcome {
    pub fn execution(self) -> ModeledOutcome {
        match self {
            Self::Reachable { at_ns, evidence_id } => {
                ModeledOutcome::Reachable { at_ns, evidence_id }
            }
            Self::Timeout {
                at_ns,
                waiting_for,
                cause,
            } => ModeledOutcome::Timeout {
                at_ns,
                waiting_for,
                cause,
            },
            Self::Incomplete { at_ns, reason } => ModeledOutcome::Incomplete { at_ns, reason },
            Self::Undetermined { modeled, .. } => modeled,
        }
    }
}

impl ModeledOutcome {
    pub fn as_outcome(self) -> Outcome {
        match self {
            Self::Reachable { at_ns, evidence_id } => Outcome::Reachable { at_ns, evidence_id },
            Self::Timeout {
                at_ns,
                waiting_for,
                cause,
            } => Outcome::Timeout {
                at_ns,
                waiting_for,
                cause,
            },
            Self::Incomplete { at_ns, reason } => Outcome::Incomplete { at_ns, reason },
        }
    }
    pub fn at_ns(self) -> u64 {
        match self {
            Self::Reachable { at_ns, .. }
            | Self::Timeout { at_ns, .. }
            | Self::Incomplete { at_ns, .. } => at_ns,
        }
    }
}

fn is_zero(value: &usize) -> bool {
    *value == 0
}
fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct ProbeCounts {
    pub completed: usize,
    pub reachable: usize,
    pub timed_out: usize,
    pub rejected_before_send: usize,
    pub incomplete: usize,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub undetermined: usize,
}

impl ProbeCounts {
    fn count(outcomes: impl IntoIterator<Item = Outcome>) -> Self {
        let mut counts = Self::default();
        for outcome in outcomes {
            if matches!(outcome, Outcome::Undetermined { .. }) {
                counts.undetermined += 1;
            }
            match outcome.execution() {
                ModeledOutcome::Incomplete { .. } => counts.incomplete += 1,
                _ => counts.completed += 1,
            }
            match outcome {
                Outcome::Reachable { .. } => counts.reachable += 1,
                Outcome::Timeout {
                    waiting_for: WaitingFor::ProbeDeadline,
                    ..
                } => counts.rejected_before_send += 1,
                Outcome::Timeout { .. } => counts.timed_out += 1,
                Outcome::Incomplete { .. } | Outcome::Undetermined { .. } => {}
            }
        }
        counts
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WaitingFor {
    ArpReply,
    EchoReply,
    ProbeDeadline,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    TimeOverflow,
    ResourceLimit { limit: ResourceLimit },
    Cancelled,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct Evidence {
    pub id: EvidenceId,
    #[serde(serialize_with = "decimal", deserialize_with = "read_decimal")]
    pub at_ns: u64,
    pub port: PortReference,
    pub frame: FrameEvidence,
    pub fact: EvidenceFact,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub protected_link: Option<ProtectedLinkEvidence>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stp_event_id: Option<EventId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub routing_event_id: Option<EventId>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub model_only: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct ProtectedLinkEvidence {
    pub link_id: String,
    pub selected_by: Option<EventId>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct PortReference {
    pub device_id: String,
    pub device_name: String,
    pub port_id: String,
    pub port_name: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct FrameEvidence {
    pub id: FrameId,
    pub parent_id: Option<FrameId>,
    pub kind: FrameKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FrameKind {
    ArpRequest,
    ArpReply,
    EchoRequest,
    EchoReply,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EvidenceFact {
    Ipv4AclDecision {
        event_id: EventId,
        direction: PortStage,
        source: Ipv4Addr,
        destination: Ipv4Addr,
        action: Ipv4AclAction,
        matched_rule: Option<Ipv4AclMatch>,
    },
    Discarded {
        stage: PortStage,
        reason: DropRule,
    },
    ArpResolved,
    EchoReplyReceived,
    EchoRequestReceived,
    Ipv4RouteMiss {
        destination: Ipv4Addr,
        request_evidence_id: Option<EvidenceId>,
    },
    DynamicRouteMiss {
        destination: Ipv4Addr,
        request_evidence_id: Option<EvidenceId>,
        reason: DynamicRouteMissReason,
        convergence_event_id: Option<EventId>,
    },
    Ipv4TtlExpired {
        received_ttl: u8,
    },
    Ipv4MtuExceeded {
        event_id: EventId,
        packet_kind: FrameKind,
        ipv4_total_bytes: u16,
        mtu_bytes: u16,
    },
    Ipv4NeighborNotUnicast {
        next_hop: Ipv4Addr,
    },
    InFlightLinkLoss {
        link_id: String,
        #[serde(serialize_with = "decimal", deserialize_with = "read_decimal")]
        planned_arrival_ns: u64,
        #[serde(serialize_with = "decimal", deserialize_with = "read_decimal")]
        sent_generation: u64,
        invalidated_by: EventId,
    },
    LinkDown {
        link_id: String,
        down_event_id: EventId,
    },
    TransmitQueueFull {
        event_id: EventId,
        link_id: String,
        #[serde(serialize_with = "decimal", deserialize_with = "read_decimal")]
        rate_bps: u64,
        queue_frames: u16,
        frame_bytes: u32,
    },
    InFlightProtectionLoss {
        link_id: String,
        #[serde(serialize_with = "decimal", deserialize_with = "read_decimal")]
        invalidated_at_ns: u64,
        #[serde(serialize_with = "decimal", deserialize_with = "read_decimal")]
        planned_arrival_ns: u64,
        #[serde(serialize_with = "decimal", deserialize_with = "read_decimal")]
        sent_generation: u64,
        invalidated_by: EventId,
    },
    StpAwaitingConvergence {
        link_id: String,
        up_event_id: EventId,
    },
    InFlightStpLoss {
        link_id: String,
        #[serde(serialize_with = "decimal", deserialize_with = "read_decimal")]
        invalidated_at_ns: u64,
        #[serde(serialize_with = "decimal", deserialize_with = "read_decimal")]
        planned_arrival_ns: u64,
        #[serde(serialize_with = "decimal", deserialize_with = "read_decimal")]
        sent_generation: u64,
        invalidated_by: EventId,
    },
    ProtectionUnavailable {
        primary_link_id: String,
        primary_down_event_id: EventId,
        standby_link_id: String,
        standby_down_event_id: EventId,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PortStage {
    Ingress,
    Egress,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Ipv4AclAction {
    Permit,
    Deny,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct Ipv4AclMatch {
    pub rule_number: usize,
    pub network: Ipv4Addr,
    pub prefix_length: u8,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "rule", rename_all = "snake_case")]
pub enum DropRule {
    TrunkVlanNotAllowed { vlan: u16, allowed_vlans: Vec<u16> },
    TrunkRequiresTag,
    UntaggedPortRejectsTag { vlan: u16 },
    AccessVlanMismatch { vlan: u16, access_vlan: u16 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceLimit {
    EventWork,
    TotalWork,
    PendingEvents,
    StateEntries,
    EssentialFacts,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(transparent)]
pub struct EventId(#[serde(serialize_with = "decimal", deserialize_with = "read_decimal")] pub u64);

#[derive(Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(untagged)]
#[allow(clippy::large_enum_variant)] // Keep the existing public variants and avoid an allocation on every run.
pub enum RunReport {
    Legacy(Report),
    Scenario(ScenarioReport),
}

impl RunReport {
    pub fn probe_counts(&self) -> ProbeCounts {
        match self {
            Self::Legacy(report) => ProbeCounts::count([report.outcome]),
            Self::Scenario(report) => report.probe_counts(),
        }
    }

    pub fn is_incomplete(&self) -> bool {
        match self {
            Self::Legacy(report) => matches!(report.outcome, Outcome::Incomplete { .. }),
            Self::Scenario(report) => report.stop_reason.is_some(),
        }
    }
}

#[derive(Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ScenarioReport {
    pub report_version: u32,
    pub model_version: u32,
    pub topology_id: String,
    pub revision: String,
    pub settings: ScenarioSettings,
    pub probes: Vec<ProbeReport>,
    pub evidence: Vec<ScenarioEvidence>,
    pub link_events: Vec<LinkEvent>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub routing_events: Vec<RoutingEvent>,
    #[serde(default, skip_serializing_if = "StpHistory::is_empty")]
    pub stp_events: StpHistory,
    pub trace: TraceReport,
    #[serde(serialize_with = "decimal", deserialize_with = "read_decimal")]
    pub last_event_ns: u64,
    pub last_event_id: Option<EventId>,
    pub stop_reason: Option<StopReason>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub import_source: Option<crate::ImportSource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coverage: Option<crate::ReportCoverage>,
}

impl ScenarioReport {
    pub fn probe_counts(&self) -> ProbeCounts {
        ProbeCounts::count(self.probes.iter().map(|probe| probe.outcome))
    }

    pub fn evidence(&self, id: EvidenceId) -> Option<&Evidence> {
        let offset = usize::try_from(id.0.checked_sub(1)?).ok()?;
        self.evidence.get(offset).map(|fact| &fact.evidence)
    }
}

#[derive(Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct ScenarioSettings {
    #[serde(serialize_with = "decimal", deserialize_with = "read_decimal")]
    pub end_ns: u64,
    #[serde(serialize_with = "decimal", deserialize_with = "read_decimal")]
    pub arp_lifetime_ns: u64,
    #[serde(serialize_with = "decimal", deserialize_with = "read_decimal")]
    pub mac_lifetime_ns: u64,
    pub default_reply_ttl: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ipv4_dont_fragment: Option<bool>,
    pub limits: crate::RunLimits,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub protected_links: Vec<ProtectedLinks>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub transmissions: Vec<LinkTransmissionSettings>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub routing: Option<Box<RoutingSettings>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stp: Option<StpSettings>,
}

#[derive(Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct StpSettings {
    #[serde(serialize_with = "decimal", deserialize_with = "read_decimal")]
    pub convergence_ns: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct StpEvent {
    pub event_id: EventId,
    #[serde(serialize_with = "decimal", deserialize_with = "read_decimal")]
    pub at_ns: u64,
    pub triggered_by_link_event_id: Option<EventId>,
    pub bridges: Vec<StpBridgeState>,
    pub links: Vec<StpLinkState>,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct StpEventSummary {
    pub event_id: EventId,
    #[serde(serialize_with = "decimal")]
    pub at_ns: u64,
    pub triggered_by_link_event_id: Option<EventId>,
    pub bridge_count: usize,
    pub link_count: usize,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct StpHistory {
    events: Vec<StpEventHeader>,
    rows: Option<Box<StpRows>>,
    row_growth_work: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct StpEventHeader {
    pub(crate) event_id: EventId,
    pub(crate) at_ns: u64,
    pub(crate) triggered_by_link_event_id: Option<EventId>,
}

#[derive(Debug, PartialEq, Eq)]
struct StpRow<T> {
    initial: T,
    changes: Vec<(usize, T)>,
}

impl<T> StpRow<T> {
    fn initial(rows: Vec<(usize, T)>) -> Vec<Self> {
        rows.into_iter()
            .map(|(_, initial)| Self {
                initial,
                changes: Vec::new(),
            })
            .collect()
    }

    fn append(&mut self, index: usize, state: T, growth_work: &mut usize) {
        *growth_work -= possible_history_copy(self.changes.len());
        reserve_history_append(&mut self.changes);
        self.changes.push((index, state));
        *growth_work += possible_history_copy(self.changes.len());
    }

    fn current(&self) -> &T {
        self.changes
            .last()
            .map_or(&self.initial, |(_, state)| state)
    }

    fn at(&self, index: usize) -> &T {
        let end = self
            .changes
            .partition_point(|(published, _)| *published <= index);
        end.checked_sub(1)
            .map_or(&self.initial, |at| &self.changes[at].1)
    }
}

#[derive(Debug, PartialEq, Eq)]
struct StpRows {
    bridges: Vec<StpRow<StpBridgeState>>,
    links: Vec<StpRow<StpLinkState>>,
}

impl StpHistory {
    pub fn summary_page(&self, offset: usize, count: usize) -> Page<StpEventSummary> {
        Page::indexed(self.events.len(), offset, count, |index| {
            let event = &self.events[index];
            let rows = self.rows.as_ref().expect("published STP rows");
            StpEventSummary {
                event_id: event.event_id,
                at_ns: event.at_ns,
                triggered_by_link_event_id: event.triggered_by_link_event_id,
                bridge_count: rows.bridges.len(),
                link_count: rows.links.len(),
            }
        })
    }

    pub fn bridge_page(
        &self,
        event_id: EventId,
        offset: usize,
        count: usize,
    ) -> Option<Page<StpBridgeState>> {
        let index = self
            .events
            .iter()
            .position(|event| event.event_id == event_id)?;
        let rows = self.rows.as_ref().expect("published STP rows");
        Some(Page::collect(&rows.bridges, offset, count, |row| {
            row.at(index).clone()
        }))
    }

    pub fn link_page(
        &self,
        event_id: EventId,
        offset: usize,
        count: usize,
    ) -> Option<Page<StpLinkState>> {
        let index = self
            .events
            .iter()
            .position(|event| event.event_id == event_id)?;
        let rows = self.rows.as_ref().expect("published STP rows");
        Some(Page::collect(&rows.links, offset, count, |row| {
            row.at(index).clone()
        }))
    }

    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// Reads a zero-based position in publication order.
    pub fn get(&self, index: usize) -> Option<StpEvent> {
        let event = self.events.get(index)?;
        let rows = self
            .rows
            .as_ref()
            .expect("the initial event defines all rows");
        Some(StpEvent {
            event_id: event.event_id,
            at_ns: event.at_ns,
            triggered_by_link_event_id: event.triggered_by_link_event_id,
            bridges: rows
                .bridges
                .iter()
                .map(|row| row.at(index).clone())
                .collect(),
            links: rows.links.iter().map(|row| row.at(index).clone()).collect(),
        })
    }

    /// Finds the referenced publication before expanding its rows.
    pub fn by_event_id(&self, event_id: EventId) -> Option<StpEvent> {
        let index = self
            .events
            .iter()
            .position(|event| event.event_id == event_id)?;
        self.get(index)
    }

    pub fn iter(&self) -> impl DoubleEndedIterator<Item = StpEvent> + ExactSizeIterator + '_ {
        (0..self.len()).map(|index| self.get(index).expect("existing STP event"))
    }

    pub(crate) fn row_growth_work(&self) -> usize {
        self.row_growth_work
    }

    pub(crate) fn current_bridge(&self, row: usize) -> Option<&StpBridgeState> {
        Some(self.rows.as_ref()?.bridges[row].current())
    }

    pub(crate) fn current_link(&self, row: usize) -> Option<&StpLinkState> {
        Some(self.rows.as_ref()?.links[row].current())
    }

    pub(crate) fn push(
        &mut self,
        event: StpEventHeader,
        bridges: Vec<(usize, StpBridgeState)>,
        links: Vec<(usize, StpLinkState)>,
    ) {
        let index = self.events.len();
        if let Some(rows) = self.rows.as_mut() {
            for (slot, state) in bridges {
                rows.bridges[slot].append(index, state, &mut self.row_growth_work);
            }
            for (slot, state) in links {
                rows.links[slot].append(index, state, &mut self.row_growth_work);
            }
        } else {
            self.rows = Some(Box::new(StpRows {
                bridges: StpRow::initial(bridges),
                links: StpRow::initial(links),
            }));
        }
        reserve_history_append(&mut self.events);
        self.events.push(event);
    }
}

const INITIAL_HISTORY_CAPACITY: usize = 4;

// Use logical capacities so metering is independent of the allocator's actual capacity.
pub(crate) fn possible_history_copy(len: usize) -> usize {
    if len >= INITIAL_HISTORY_CAPACITY && len.is_power_of_two() {
        len
    } else {
        0
    }
}

pub(crate) fn reserve_history_append<T>(entries: &mut Vec<T>) {
    entries.reserve_exact(if entries.is_empty() {
        INITIAL_HISTORY_CAPACITY
    } else {
        possible_history_copy(entries.len())
    });
}

impl Serialize for StpHistory {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut seq = serializer.serialize_seq(Some(self.len()))?;
        for event in self.iter() {
            seq.serialize_element(&event)?;
        }
        seq.end()
    }
}

impl<'de> Deserialize<'de> for StpHistory {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let events = Vec::<StpEvent>::deserialize(deserializer)?;
        let mut history = Self::default();
        let mut bridge_slots = HashMap::new();
        let mut link_slots = HashMap::new();
        let mut event_ids = HashSet::new();
        let mut previous_ns = 0;
        for (index, event) in events.into_iter().enumerate() {
            if event.event_id.0 == 0 || !event_ids.insert(event.event_id.0) {
                return Err(de::Error::custom("STP 发布事件 ID 须为非零且不重复。"));
            }
            if event.at_ns < previous_ns {
                return Err(de::Error::custom("STP 发布须按发生时间排列。"));
            }
            previous_ns = event.at_ns;
            if index == 0 {
                for (slot, bridge) in event.bridges.iter().enumerate() {
                    if bridge.bridge_id.is_empty()
                        || bridge_slots
                            .insert(bridge.bridge_id.clone(), slot)
                            .is_some()
                    {
                        return Err(de::Error::custom("STP 桥 ID 须为非空且不重复。"));
                    }
                }
                let mut port_ids = HashSet::new();
                for (slot, link) in event.links.iter().enumerate() {
                    if link.link_id.is_empty()
                        || link_slots.insert(link.link_id.clone(), slot).is_some()
                    {
                        return Err(de::Error::custom("STP 链路 ID 须为非空且不重复。"));
                    }
                    for port in &link.ports {
                        if port.port_id.is_empty() || !port_ids.insert(&port.port_id) {
                            return Err(de::Error::custom("STP 端口 ID 须为非空且不重复。"));
                        }
                    }
                }
            }
            if event.bridges.len() != bridge_slots.len() || event.links.len() != link_slots.len() {
                return Err(de::Error::custom("STP 发布的桥或链路集合改变。"));
            }
            let mut bridges = Vec::with_capacity(event.bridges.len());
            let mut seen = HashSet::new();
            for bridge in event.bridges {
                let slot = *bridge_slots
                    .get(&bridge.bridge_id)
                    .ok_or_else(|| <D::Error as de::Error>::custom("STP 发布含未知桥 ID。"))?;
                if !seen.insert(slot) {
                    return Err(de::Error::custom("STP 发布含重复桥 ID。"));
                }
                if index == 0 || history.current_bridge(slot) != Some(&bridge) {
                    bridges.push((slot, bridge));
                }
            }
            let mut links = Vec::with_capacity(event.links.len());
            seen.clear();
            for link in event.links {
                let slot = *link_slots
                    .get(&link.link_id)
                    .ok_or_else(|| <D::Error as de::Error>::custom("STP 发布含未知链路 ID。"))?;
                if !seen.insert(slot) {
                    return Err(de::Error::custom("STP 发布含重复链路 ID。"));
                }
                if let Some(previous) = history.current_link(slot)
                    && previous.ports.each_ref().map(|port| &port.port_id)
                        != link.ports.each_ref().map(|port| &port.port_id)
                {
                    return Err(de::Error::custom("STP 链路的端口槽位改变。"));
                }
                if index == 0 || history.current_link(slot) != Some(&link) {
                    links.push((slot, link));
                }
            }
            bridges.sort_unstable_by_key(|(slot, _)| *slot);
            links.sort_unstable_by_key(|(slot, _)| *slot);
            history.push(
                StpEventHeader {
                    event_id: event.event_id,
                    at_ns: event.at_ns,
                    triggered_by_link_event_id: event.triggered_by_link_event_id,
                },
                bridges,
                links,
            );
        }
        Ok(history)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct StpBridgeState {
    pub bridge_id: String,
    pub priority: u16,
    pub root_bridge_id: String,
    #[serde(serialize_with = "decimal", deserialize_with = "read_decimal")]
    pub root_path_cost: u64,
    pub root_port_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct StpLinkState {
    pub link_id: String,
    pub ports: [StpPortState; 2],
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct StpPortState {
    pub port_id: String,
    pub path_cost: u32,
    pub role: StpPortRole,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StpPortRole {
    Root,
    Designated,
    Alternate,
    Disabled,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct RoutingSettings {
    #[serde(serialize_with = "decimal", deserialize_with = "read_decimal")]
    pub convergence_ns: u64,
    pub advertisements: Vec<RoutingAdvertisement>,
    pub adjacency_issues: Vec<RoutingAdjacencyIssue>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct RoutingAdvertisement {
    pub router_id: String,
    pub port_id: String,
    pub network: Ipv4Addr,
    pub prefix_length: u8,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct RoutingAdjacencyIssue {
    pub link_id: String,
    pub port_id: String,
    pub peer_port_id: String,
    pub reason: RoutingAdjacencyProblem,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RoutingAdjacencyProblem {
    PeerDisabled,
    SubnetMismatch,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DynamicRouteMissReason {
    AwaitingInitialConvergence,
    NoAdvertisedPrefix,
    NoPathToAdvertisement,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct RoutingEvent {
    pub event_id: EventId,
    #[serde(serialize_with = "decimal", deserialize_with = "read_decimal")]
    pub at_ns: u64,
    pub router_id: String,
    pub triggered_by_link_event_id: Option<EventId>,
    pub installed_routes: usize,
}

#[derive(Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct LinkTransmissionSettings {
    pub link_id: String,
    #[serde(serialize_with = "decimal", deserialize_with = "read_decimal")]
    pub rate_bps: u64,
    pub queue_frames: u16,
}

#[derive(Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct ProbeReport {
    pub id: String,
    #[serde(flatten)]
    pub probe: Probe,
    #[serde(serialize_with = "decimal", deserialize_with = "read_decimal")]
    pub at_ns: u64,
    pub ttl: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload_bytes: Option<u16>,
    pub icmp: IcmpIdentity,
    pub outcome: Outcome,
    pub request_evidence_id: Option<EvidenceId>,
}

#[derive(Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct IcmpIdentity {
    pub identifier: u16,
    pub sequence: u16,
}

#[derive(Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct ScenarioEvidence {
    pub probe_id: String,
    #[serde(flatten)]
    pub evidence: Evidence,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkStatus {
    Up,
    Down,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct LinkEvent {
    pub event_id: EventId,
    pub link_id: String,
    #[serde(serialize_with = "decimal", deserialize_with = "read_decimal")]
    pub at_ns: u64,
    pub state: LinkStatus,
    #[serde(serialize_with = "decimal", deserialize_with = "read_decimal")]
    pub generation: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub simultaneous: Option<LinkMemberChange>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub protection: Option<ProtectionChange>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct LinkMemberChange {
    pub link_id: String,
    pub state: LinkStatus,
    #[serde(serialize_with = "decimal", deserialize_with = "read_decimal")]
    pub generation: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct ProtectionChange {
    #[serde(flatten)]
    pub links: ProtectedLinks,
    pub previous_active_link_id: Option<String>,
    pub active_link_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct ProtectedLinks {
    pub primary_link_id: String,
    pub standby_link_id: String,
}

#[derive(Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct TraceReport {
    pub status: TraceStatus,
    pub entries: Vec<TraceEntry>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TraceStatus {
    Disabled,
    Collected,
    Truncated,
    Evicted,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct TraceEntry {
    pub event_id: EventId,
    #[serde(serialize_with = "decimal", deserialize_with = "read_decimal")]
    pub at_ns: u64,
    pub event: TraceEvent,
    pub frame: Option<FrameEvidence>,
    pub port: Option<PortReference>,
    pub probe_id: Option<String>,
    pub learning: Option<TraceLearning>,
    pub route: Option<TraceRoute>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TraceEvent {
    LinkChange,
    RoutingConvergence,
    StpConvergence,
    TableExpiry,
    ProbeDeadline,
    TransmitStart,
    TransmitCancelled,
    FrameArrival,
    ProbeStart,
    NeighborWarmup,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TraceLearning {
    Arp { neighbor: Ipv4Addr },
    Mac { vlan: u16 },
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct TraceRoute {
    pub egress_port_id: String,
    pub destination: Ipv4Addr,
    pub next_hop: Ipv4Addr,
    pub network: Option<Ipv4Addr>,
    pub prefix_length: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ipv4_mtu: Option<TraceMtuCheck>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub learned: Option<TraceLearnedRoute>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct TraceLearnedRoute {
    pub origin_router_id: String,
    #[serde(serialize_with = "decimal", deserialize_with = "read_decimal")]
    pub cost: u64,
    pub convergence_event_id: EventId,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct TraceMtuCheck {
    pub packet_kind: FrameKind,
    pub ipv4_total_bytes: u16,
    pub mtu_bytes: u16,
    pub passed: bool,
}

#[derive(Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct Page<T> {
    pub entries: Vec<T>,
    pub next_offset: Option<usize>,
}

impl<T> Page<T> {
    pub(crate) fn collect<E>(
        entries: &[E],
        offset: usize,
        count: usize,
        project: impl FnMut(&E) -> T,
    ) -> Self {
        let mut project = project;
        Self::indexed(entries.len(), offset, count, |index| {
            project(&entries[index])
        })
    }

    pub(crate) fn indexed(
        len: usize,
        offset: usize,
        count: usize,
        project: impl FnMut(usize) -> T,
    ) -> Self {
        let start = offset.min(len);
        let end = (start + count.min(128)).min(len);
        Self {
            entries: (start..end).map(project).collect(),
            next_offset: (start < end && end < len).then_some(end),
        }
    }
}

#[derive(Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct EvidenceItem {
    pub probe_id: Option<String>,
    #[serde(flatten)]
    pub evidence: Evidence,
}

#[derive(Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct TracePage {
    pub status: Option<TraceStatus>,
    #[serde(flatten)]
    pub page: Page<TraceEntry>,
}
