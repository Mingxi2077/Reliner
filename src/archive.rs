use std::{
    collections::{HashMap, HashSet},
    error::Error,
    fmt,
    io::{self, Read, Write},
};

use serde::{Deserialize, Serialize, Serializer};
use serde_json::value::RawValue;

use crate::{
    EventId, Evidence, EvidenceFact, EvidenceId, EvidenceItem, FrameEvidence, FrameId,
    Ipv4AclAction, LinkEvent, LinkStatus, Outcome, Page, PortStage, ProbeCounts, RoutingEvent,
    RunReport, RunStatus, ScenarioReport, StopReason, StpEvent, TraceEntry, TraceEvent, TracePage,
    TraceReport, TraceStatus,
};

pub const MAX_ARCHIVE_BYTES: usize = 64 * 1024 * 1024;
const ARCHIVE_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ArchiveSource {
    pub run_id: String,
    pub document_revision: String,
    pub effective_revision: String,
}

#[derive(Debug)]
pub enum ArchiveError {
    Io(io::Error),
    Json(serde_json::Error),
    UnsupportedVersion(u32),
    UnsupportedReportVersion(u32),
    TooLarge,
    InvalidReport(String),
}

impl fmt::Display for ArchiveError {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => error.fmt(out),
            Self::Json(error) => error.fmt(out),
            Self::UnsupportedVersion(version) => {
                write!(out, "不支持归档版本 archive_version {version}。")
            }
            Self::UnsupportedReportVersion(version) => {
                write!(out, "不支持报告版本 report_version {version}。")
            }
            Self::TooLarge => out.write_str("报告归档超过 64 MiB 上限。"),
            Self::InvalidReport(message) => out.write_str(message),
        }
    }
}

impl Error for ArchiveError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Json(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for ArchiveError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for ArchiveError {
    fn from(error: serde_json::Error) -> Self {
        if error.is_io() {
            Self::Io(error.into())
        } else {
            Self::Json(error)
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct ArchivedRun {
    pub report: RunReport,
    pub trace: Option<TraceReport>,
    pub source: Option<ArchiveSource>,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct ArchiveSummary {
    pub report_version: u32,
    pub model_version: u32,
    pub topology_id: String,
    pub revision: String,
    pub status: RunStatus,
    pub probe_counts: ProbeCounts,
    pub stop_reason: Option<StopReason>,
    pub stop_limit: Option<usize>,
    #[serde(serialize_with = "optional_decimal")]
    pub last_event_ns: Option<u64>,
    pub last_event_id: Option<EventId>,
    pub trace_status: Option<TraceStatus>,
}

fn optional_decimal<S: Serializer>(value: &Option<u64>, serializer: S) -> Result<S::Ok, S::Error> {
    value.map(|value| value.to_string()).serialize(serializer)
}

impl ArchivedRun {
    pub fn summary(&self) -> ArchiveSummary {
        let (version, model, topology, revision, stop, limit, last_ns, last_id) = match &self.report
        {
            RunReport::Legacy(report) => (
                report.report_version,
                report.model_version,
                &report.topology_id,
                &report.revision,
                match report.outcome {
                    Outcome::Incomplete { reason, .. } => Some(reason),
                    _ => None,
                },
                None,
                None,
                None,
            ),
            RunReport::Scenario(report) => (
                report.report_version,
                report.model_version,
                &report.topology_id,
                &report.revision,
                report.stop_reason,
                match report.stop_reason {
                    Some(StopReason::ResourceLimit { limit }) => {
                        Some(report.settings.limits.maximum(limit))
                    }
                    _ => None,
                },
                report.last_event_id.map(|_| report.last_event_ns),
                report.last_event_id,
            ),
        };
        ArchiveSummary {
            report_version: version,
            model_version: model,
            topology_id: topology.clone(),
            revision: revision.clone(),
            status: match stop {
                None => RunStatus::Finished,
                Some(StopReason::Cancelled) => RunStatus::Cancelled,
                Some(_) => RunStatus::Incomplete,
            },
            probe_counts: self.report.probe_counts(),
            stop_reason: stop,
            stop_limit: limit,
            last_event_ns: last_ns,
            last_event_id: last_id,
            trace_status: self.trace_view().0,
        }
    }

    pub fn evidence_page(&self, offset: usize, count: usize) -> Page<EvidenceItem> {
        match &self.report {
            RunReport::Legacy(report) => {
                Page::collect(&report.evidence, offset, count, |fact| EvidenceItem {
                    probe_id: None,
                    evidence: fact.clone(),
                })
            }
            RunReport::Scenario(report) => {
                Page::collect(&report.evidence, offset, count, |fact| EvidenceItem {
                    probe_id: Some(fact.probe_id.clone()),
                    evidence: fact.evidence.clone(),
                })
            }
        }
    }

    pub fn link_event_page(&self, offset: usize, count: usize) -> Page<LinkEvent> {
        let events = match &self.report {
            RunReport::Legacy(_) => &[][..],
            RunReport::Scenario(report) => report.link_events.as_slice(),
        };
        Page::collect(events, offset, count, Clone::clone)
    }

    pub fn routing_event_page(&self, offset: usize, count: usize) -> Page<RoutingEvent> {
        let events = match &self.report {
            RunReport::Legacy(_) => &[][..],
            RunReport::Scenario(report) => report.routing_events.as_slice(),
        };
        Page::collect(events, offset, count, Clone::clone)
    }

    pub fn stp_event_page(&self, offset: usize, count: usize) -> Page<StpEvent> {
        match &self.report {
            RunReport::Legacy(_) => Page {
                entries: Vec::new(),
                next_offset: None,
            },
            RunReport::Scenario(report) => {
                Page::indexed(report.stp_events.len(), offset, count, |index| {
                    report
                        .stp_events
                        .get(index)
                        .expect("existing archived STP event")
                })
            }
        }
    }

    pub fn stp_event(&self, event_id: EventId) -> Option<StpEvent> {
        match &self.report {
            RunReport::Legacy(_) => None,
            RunReport::Scenario(report) => report.stp_events.by_event_id(event_id),
        }
    }

    pub fn trace_page(&self, offset: usize, count: usize) -> TracePage {
        let (status, entries) = self.trace_view();
        TracePage {
            status,
            page: Page::collect(entries, offset, count, Clone::clone),
        }
    }

    fn trace_view(&self) -> (Option<TraceStatus>, &[TraceEntry]) {
        if let Some(trace) = &self.trace {
            return (Some(trace.status), &trace.entries);
        }
        match &self.report {
            RunReport::Legacy(_) => (None, &[]),
            RunReport::Scenario(report) => (Some(report.trace.status), &report.trace.entries),
        }
    }
}

#[derive(Serialize)]
struct ArchiveOutput<'a> {
    archive_version: u32,
    report: &'a RunReport,
    trace: Option<&'a TraceReport>,
    source: Option<&'a ArchiveSource>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ArchiveInput<'a> {
    archive_version: u32,
    #[serde(borrow)]
    report: &'a RawValue,
    #[serde(borrow)]
    trace: Option<&'a RawValue>,
    #[serde(borrow)]
    source: Option<&'a RawValue>,
}

pub fn write_archive(
    report: &RunReport,
    trace: Option<&TraceReport>,
    source: Option<&ArchiveSource>,
    writer: impl Write,
) -> Result<(), ArchiveError> {
    if let Some(source) = source {
        validate_source(source)?;
    }
    validate_report(report, trace)?;
    let mut writer = LimitedWriter {
        inner: writer,
        written: 0,
        exceeded: false,
    };
    let result = serde_json::to_writer(
        &mut writer,
        &ArchiveOutput {
            archive_version: ARCHIVE_VERSION,
            report,
            trace,
            source,
        },
    );
    if writer.exceeded {
        return Err(ArchiveError::TooLarge);
    }
    result.map_err(ArchiveError::from)
}

pub fn read_archive(reader: impl Read) -> Result<ArchivedRun, ArchiveError> {
    let mut bytes = Vec::new();
    reader
        .take(MAX_ARCHIVE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_ARCHIVE_BYTES {
        return Err(ArchiveError::TooLarge);
    }
    let input: ArchiveInput<'_> = serde_json::from_slice(&bytes)?;
    if input.archive_version != ARCHIVE_VERSION {
        return Err(ArchiveError::UnsupportedVersion(input.archive_version));
    }
    let archived = ArchivedRun {
        report: read_report(input.report.get())?,
        trace: input
            .trace
            .map(|trace| serde_json::from_str(trace.get()))
            .transpose()?,
        source: input
            .source
            .map(|source| serde_json::from_str(source.get()))
            .transpose()?,
    };
    if let Some(source) = &archived.source {
        validate_source(source)?;
    }
    validate_report(&archived.report, archived.trace.as_ref())?;
    Ok(archived)
}

fn read_report(input: &str) -> Result<RunReport, ArchiveError> {
    #[derive(Deserialize)]
    struct Header {
        report_version: u32,
    }

    let header: Header = serde_json::from_str(input)?;
    // Decode the selected shape directly so field errors survive the untagged public enum.
    match header.report_version {
        1 => Ok(RunReport::Legacy(serde_json::from_str(input)?)),
        2..=9 => Ok(RunReport::Scenario(serde_json::from_str(input)?)),
        version => Err(ArchiveError::UnsupportedReportVersion(version)),
    }
}

struct LimitedWriter<W> {
    inner: W,
    written: usize,
    exceeded: bool,
}

impl<W: Write> Write for LimitedWriter<W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > MAX_ARCHIVE_BYTES - self.written {
            self.exceeded = true;
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "报告归档超过 64 MiB 上限。",
            ));
        }
        let count = self.inner.write(bytes)?;
        self.written += count;
        Ok(count)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

fn invalid(message: impl Into<String>) -> ArchiveError {
    ArchiveError::InvalidReport(message.into())
}

fn nonzero(id: u64, path: &str) -> Result<(), ArchiveError> {
    if id == 0 {
        Err(invalid(format!("{path} 的 ID 不得为 0。")))
    } else {
        Ok(())
    }
}

fn validate_source(source: &ArchiveSource) -> Result<(), ArchiveError> {
    for (field, value) in [
        ("run_id", &source.run_id),
        ("document_revision", &source.document_revision),
        ("effective_revision", &source.effective_revision),
    ] {
        if value.is_empty()
            || !value.bytes().all(|byte| byte.is_ascii_digit())
            || value.parse::<u64>().is_err()
        {
            return Err(invalid(format!(
                "/source/{field} 须为 u64 范围内的十进制整数字符串。"
            )));
        }
    }
    Ok(())
}

fn outcome_time(outcome: Outcome) -> u64 {
    outcome.execution().at_ns()
}

struct References<'a> {
    evidence: HashMap<u64, (&'a str, &'a Evidence)>,
    frame_acls: HashMap<(&'a str, u64), &'a Evidence>,
    links: HashMap<u64, &'a LinkEvent>,
    routes: HashMap<u64, &'a RoutingEvent>,
    trees: HashMap<u64, u64>,
    probes: HashMap<&'a str, &'a crate::Probe>,
    events: HashSet<u64>,
}

impl<'a> References<'a> {
    fn new() -> Self {
        Self {
            evidence: HashMap::new(),
            frame_acls: HashMap::new(),
            links: HashMap::new(),
            routes: HashMap::new(),
            trees: HashMap::new(),
            probes: HashMap::new(),
            events: HashSet::new(),
        }
    }

    fn add_event(
        &mut self,
        id: EventId,
        at_ns: u64,
        report: &ScenarioReport,
        path: &str,
    ) -> Result<(), ArchiveError> {
        nonzero(id.0, path)?;
        if !self.events.insert(id.0) {
            return Err(invalid(format!("{path} 重复使用事件 ID {}。", id.0)));
        }
        if report.last_event_id.is_none() || at_ns > report.last_event_ns {
            return Err(invalid(format!("{path} 晚于报告的最后完整事件。")));
        }
        Ok(())
    }

    fn evidence(
        &self,
        id: EvidenceId,
        probe: &str,
        path: &str,
    ) -> Result<&'a Evidence, ArchiveError> {
        match self.evidence.get(&id.0) {
            Some((owner, evidence)) if *owner == probe => Ok(*evidence),
            _ => Err(invalid(format!(
                "{path} 引用的证据 {} 不存在或不属于探测 {probe}。",
                id.0
            ))),
        }
    }

    fn request(
        &self,
        id: EvidenceId,
        probe: &str,
        at_ns: u64,
        path: &str,
    ) -> Result<(), ArchiveError> {
        let evidence = self.evidence(id, probe, path)?;
        if !matches!(evidence.fact, EvidenceFact::EchoRequestReceived) {
            return Err(invalid(format!(
                "{path} 引用的证据 {} 不是请求送达事实。",
                id.0
            )));
        }
        if evidence.at_ns > at_ns {
            return Err(invalid(format!(
                "{path} 引用的请求送达证据 {} 晚于引用它的事实。",
                id.0
            )));
        }
        Ok(())
    }

    fn link(&self, id: EventId, at_ns: u64, path: &str) -> Result<&'a LinkEvent, ArchiveError> {
        let event = self
            .links
            .get(&id.0)
            .ok_or_else(|| invalid(format!("{path} 引用的链路事件 {} 不存在。", id.0)))?;
        if event.at_ns > at_ns {
            return Err(invalid(format!(
                "{path} 引用了尚未发生的链路事件 {}。",
                id.0
            )));
        }
        Ok(*event)
    }

    fn route(&self, id: EventId, at_ns: u64, path: &str) -> Result<(), ArchiveError> {
        let event = self
            .routes
            .get(&id.0)
            .ok_or_else(|| invalid(format!("{path} 引用的路由事件 {} 不存在。", id.0)))?;
        if event.at_ns > at_ns {
            return Err(invalid(format!(
                "{path} 引用了尚未发生的路由事件 {}。",
                id.0
            )));
        }
        Ok(())
    }

    fn tree(&self, id: EventId, at_ns: u64, path: &str) -> Result<(), ArchiveError> {
        let time = self
            .trees
            .get(&id.0)
            .ok_or_else(|| invalid(format!("{path} 引用的 STP 事件 {} 不存在。", id.0)))?;
        if *time > at_ns {
            return Err(invalid(format!(
                "{path} 引用了尚未发生的 STP 事件 {}。",
                id.0
            )));
        }
        Ok(())
    }

    fn outcome(&self, outcome: Outcome, probe: &str, path: &str) -> Result<(), ArchiveError> {
        match outcome.execution().as_outcome() {
            Outcome::Reachable { at_ns, evidence_id } => {
                let evidence = self.evidence(evidence_id, probe, &format!("{path}/evidence_id"))?;
                if evidence.at_ns != at_ns
                    || !matches!(evidence.fact, EvidenceFact::EchoReplyReceived)
                {
                    return Err(invalid(format!(
                        "{path}/evidence_id 不是该时刻的匹配回包事实。"
                    )));
                }
            }
            Outcome::Timeout {
                at_ns,
                cause: Some(id),
                ..
            } => {
                let evidence = self.evidence(id, probe, &format!("{path}/cause"))?;
                if matches!(
                    evidence.fact,
                    EvidenceFact::Ipv4AclDecision {
                        action: crate::Ipv4AclAction::Permit,
                        ..
                    }
                ) {
                    return Err(invalid(format!("{path}/cause 引用了 ACL 放行事实。")));
                }
                if evidence.at_ns > at_ns {
                    return Err(invalid(format!("{path}/cause 晚于探测结案。")));
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn denied_at(&self, probe: &str, frame: FrameId) -> Option<PortStage> {
        match self.frame_acls.get(&(probe, frame.0))?.fact {
            EvidenceFact::Ipv4AclDecision {
                direction,
                action: Ipv4AclAction::Deny,
                ..
            } => Some(direction),
            _ => None,
        }
    }

    fn check_acl_parent(
        &self,
        probe: &str,
        frame: FrameEvidence,
        path: &str,
    ) -> Result<(), ArchiveError> {
        // Every provable chain from a denied frame includes its first retained edge.
        if frame
            .parent_id
            .is_some_and(|id| self.denied_at(probe, id).is_some())
        {
            return Err(invalid(format!(
                "{path}/frame/parent_id 引用了已被 ACL 拒绝的帧。"
            )));
        }
        Ok(())
    }

    fn fact(
        &self,
        evidence: &Evidence,
        probe: &str,
        version: u32,
        path: &str,
    ) -> Result<(), ArchiveError> {
        nonzero(evidence.frame.id.0, &format!("{path}/frame/id"))?;
        // Frame parents need not have their own retained evidence or optional trace entry.
        if let Some(parent) = evidence.frame.parent_id {
            nonzero(parent.0, &format!("{path}/frame/parent_id"))?;
        }
        self.check_acl_parent(probe, evidence.frame, path)?;
        if !matches!(evidence.fact, EvidenceFact::Ipv4AclDecision { .. })
            && self.denied_at(probe, evidence.frame.id).is_some()
        {
            return Err(invalid(format!("{path}/fact 与该帧的 ACL 拒绝判定矛盾。")));
        }
        if let Some(id) = evidence.stp_event_id {
            self.tree(id, evidence.at_ns, &format!("{path}/stp_event_id"))?;
        }
        if let Some(id) = evidence.routing_event_id {
            self.route(id, evidence.at_ns, &format!("{path}/routing_event_id"))?;
        }
        if let Some(link) = &evidence.protected_link
            && let Some(id) = link.selected_by
        {
            let event = self.link(
                id,
                evidence.at_ns,
                &format!("{path}/protected_link/selected_by"),
            )?;
            if event
                .protection
                .as_ref()
                .and_then(|change| change.active_link_id.as_deref())
                != Some(link.link_id.as_str())
            {
                return Err(invalid(format!(
                    "{path}/protected_link/selected_by 没有选中对应链路。"
                )));
            }
        }
        let at = evidence.at_ns;
        let path = format!("{path}/fact");
        match &evidence.fact {
            EvidenceFact::Ipv4AclDecision {
                event_id,
                source,
                destination,
                action,
                matched_rule,
                ..
            } => {
                if version < 9 || evidence.frame.id.0 != event_id.0 {
                    return Err(invalid(format!(
                        "{path}/event_id 须属于版本 9 的当前 IPv4 到达帧。"
                    )));
                }
                let probe = self.probes[probe];
                let endpoints = (probe.source.ipv4, probe.destination.ipv4);
                let valid_packet = match evidence.frame.kind {
                    crate::FrameKind::EchoRequest => (*source, *destination) == endpoints,
                    crate::FrameKind::EchoReply => (*destination, *source) == endpoints,
                    _ => false,
                };
                if !valid_packet {
                    return Err(invalid(format!(
                        "{path}/source 与该探测的包方向或地址不符。"
                    )));
                }
                let valid_rule =
                    matched_rule
                        .as_ref()
                        .map_or(*action == crate::Ipv4AclAction::Deny, |rule| {
                            (1..=crate::topology::MAX_ACL_RULES).contains(&rule.rule_number)
                                && rule.prefix_length <= 32
                                && u32::from(*source) & crate::topology::netmask(rule.prefix_length)
                                    == u32::from(rule.network)
                        });
                if !valid_rule {
                    return Err(invalid(format!(
                        "{path}/matched_rule 与源前缀或隐式拒绝不符。"
                    )));
                }
            }
            EvidenceFact::Ipv4RouteMiss {
                request_evidence_id,
                ..
            }
            | EvidenceFact::DynamicRouteMiss {
                request_evidence_id,
                ..
            } => {
                if let Some(id) = request_evidence_id {
                    self.request(*id, probe, at, &format!("{path}/request_evidence_id"))?;
                }
                if let EvidenceFact::DynamicRouteMiss {
                    convergence_event_id: Some(id),
                    ..
                } = &evidence.fact
                {
                    self.route(*id, at, &format!("{path}/convergence_event_id"))?;
                }
            }
            EvidenceFact::InFlightLinkLoss {
                link_id,
                invalidated_by,
                ..
            } => {
                let event = self.link(*invalidated_by, at, &format!("{path}/invalidated_by"))?;
                check_link_member(event, link_id, LinkStatus::Down, &path)?;
            }
            EvidenceFact::LinkDown {
                link_id,
                down_event_id,
            } => {
                let event = self.link(*down_event_id, at, &format!("{path}/down_event_id"))?;
                check_link_member(event, link_id, LinkStatus::Down, &path)?;
            }
            EvidenceFact::InFlightProtectionLoss { invalidated_by, .. } => {
                let event = self.link(*invalidated_by, at, &format!("{path}/invalidated_by"))?;
                if event.protection.is_none() {
                    return Err(invalid(format!("{path}/invalidated_by 不是主备选择事件。")));
                }
            }
            EvidenceFact::StpAwaitingConvergence {
                link_id,
                up_event_id,
            } => {
                let event = self.link(*up_event_id, at, &format!("{path}/up_event_id"))?;
                check_link_member(event, link_id, LinkStatus::Up, &path)?;
            }
            EvidenceFact::InFlightStpLoss { invalidated_by, .. } => {
                self.tree(*invalidated_by, at, &format!("{path}/invalidated_by"))?;
            }
            EvidenceFact::ProtectionUnavailable {
                primary_link_id,
                primary_down_event_id,
                standby_link_id,
                standby_down_event_id,
            } => {
                let primary = self.link(
                    *primary_down_event_id,
                    at,
                    &format!("{path}/primary_down_event_id"),
                )?;
                check_link_member(primary, primary_link_id, LinkStatus::Down, &path)?;
                let standby = self.link(
                    *standby_down_event_id,
                    at,
                    &format!("{path}/standby_down_event_id"),
                )?;
                check_link_member(standby, standby_link_id, LinkStatus::Down, &path)?;
            }
            // These identify ordinary execution events, not required control-event collections.
            EvidenceFact::Ipv4MtuExceeded { event_id, .. }
            | EvidenceFact::TransmitQueueFull { event_id, .. } => {
                nonzero(event_id.0, &format!("{path}/event_id"))?;
            }
            _ => {}
        }
        Ok(())
    }
}

fn check_link_member(
    event: &LinkEvent,
    link: &str,
    state: LinkStatus,
    path: &str,
) -> Result<(), ArchiveError> {
    if (event.link_id == link && event.state == state)
        || event
            .simultaneous
            .as_ref()
            .is_some_and(|member| member.link_id == link && member.state == state)
    {
        Ok(())
    } else {
        Err(invalid(format!(
            "{path} 引用的事件 {} 没有使链路 {link} 进入 {state:?}。",
            event.event_id.0
        )))
    }
}

fn validate_report(report: &RunReport, trace: Option<&TraceReport>) -> Result<(), ArchiveError> {
    let mut refs = References::new();
    match report {
        RunReport::Legacy(report) => {
            if report.report_version != 1 {
                return Err(ArchiveError::UnsupportedReportVersion(
                    report.report_version,
                ));
            }
            if matches!(report.outcome, Outcome::Undetermined { .. }) {
                return Err(invalid("/report/outcome 的未判定状态不适用于报告版本 1。"));
            }
            if report.evidence.iter().any(|fact| fact.model_only) {
                return Err(invalid("/report/evidence/model_only 不适用于报告版本 1。"));
            }
            if matches!(report.outcome, Outcome::Timeout { at_ns, .. } if at_ns != report.probe.deadline_ns)
                || matches!(report.outcome, Outcome::Reachable { at_ns, .. } if at_ns >= report.probe.deadline_ns)
            {
                return Err(invalid("/report/outcome 与探测截止时刻不一致。"));
            }
            refs.probes.insert("1", &report.probe);
            for (index, evidence) in report.evidence.iter().enumerate() {
                add_evidence(&mut refs, index, "1", evidence)?;
            }
            refs.outcome(report.outcome, "1", "/report/outcome")?;
            for (index, evidence) in report.evidence.iter().enumerate() {
                refs.fact(evidence, "1", 1, &format!("/report/evidence/{index}"))?;
            }
        }
        RunReport::Scenario(report) => {
            if !(2..=9).contains(&report.report_version) {
                return Err(ArchiveError::UnsupportedReportVersion(
                    report.report_version,
                ));
            }
            crate::import_archive::validate_import_coverage(report).map_err(invalid)?;
            if let Some(id) = report.last_event_id {
                nonzero(id.0, "/report/last_event_id")?;
            }
            if report.last_event_ns > report.settings.end_ns
                || (report.last_event_id.is_none() && report.last_event_ns != 0)
            {
                return Err(invalid("/report/last_event_ns 不符合场景或最后完整事件。"));
            }
            for probe in &report.probes {
                if probe.id.is_empty() || refs.probes.insert(&probe.id, &probe.probe).is_some() {
                    return Err(invalid(format!(
                        "/report/probes 包含空或重复的探测 ID {}。",
                        probe.id
                    )));
                }
            }
            for (index, fact) in report.evidence.iter().enumerate() {
                if !refs.probes.contains_key(fact.probe_id.as_str()) {
                    return Err(invalid(format!(
                        "/report/evidence/{index}/probe_id 引用了未知探测 {}。",
                        fact.probe_id
                    )));
                }
                if report.last_event_id.is_none() || fact.evidence.at_ns > report.last_event_ns {
                    return Err(invalid(format!(
                        "/report/evidence/{index} 晚于最后完整事件。"
                    )));
                }
                add_evidence(&mut refs, index, &fact.probe_id, &fact.evidence)?;
            }
            for (index, event) in report.link_events.iter().enumerate() {
                refs.add_event(
                    event.event_id,
                    event.at_ns,
                    report,
                    &format!("/report/link_events/{index}/event_id"),
                )?;
                refs.links.insert(event.event_id.0, event);
            }
            for (index, event) in report.routing_events.iter().enumerate() {
                refs.add_event(
                    event.event_id,
                    event.at_ns,
                    report,
                    &format!("/report/routing_events/{index}/event_id"),
                )?;
                refs.routes.insert(event.event_id.0, event);
            }
            for (index, event) in report.stp_events.iter().enumerate() {
                refs.add_event(
                    event.event_id,
                    event.at_ns,
                    report,
                    &format!("/report/stp_events/{index}/event_id"),
                )?;
                refs.trees.insert(event.event_id.0, event.at_ns);
                validate_tree(&event, index)?;
            }
            for (index, event) in report.routing_events.iter().enumerate() {
                if let Some(id) = event.triggered_by_link_event_id {
                    refs.link(
                        id,
                        event.at_ns,
                        &format!("/report/routing_events/{index}/triggered_by_link_event_id"),
                    )?;
                }
            }
            for (index, event) in report.stp_events.iter().enumerate() {
                if let Some(id) = event.triggered_by_link_event_id {
                    refs.link(
                        id,
                        event.at_ns,
                        &format!("/report/stp_events/{index}/triggered_by_link_event_id"),
                    )?;
                }
            }
            for (index, probe) in report.probes.iter().enumerate() {
                let execution = probe.outcome.execution().as_outcome();
                let at_ns = outcome_time(execution);
                if !matches!(execution, Outcome::Incomplete { .. })
                    && (report.last_event_id.is_none()
                        || at_ns < probe.at_ns
                        || at_ns > report.last_event_ns)
                {
                    return Err(invalid(format!(
                        "/report/probes/{index}/outcome 不在探测发起与最后完整事件之间。"
                    )));
                }
                if matches!(execution, Outcome::Timeout { .. }) && at_ns != probe.probe.deadline_ns
                {
                    return Err(invalid(format!(
                        "/report/probes/{index}/outcome 与探测截止时刻不一致。"
                    )));
                }
                if matches!(execution, Outcome::Reachable { .. })
                    && at_ns >= probe.probe.deadline_ns
                {
                    return Err(invalid(format!(
                        "/report/probes/{index}/outcome 未在探测截止前完成。"
                    )));
                }
                if let Outcome::Incomplete { at_ns, reason } = execution
                    && (report.stop_reason != Some(reason) || at_ns != report.last_event_ns)
                {
                    return Err(invalid(format!(
                        "/report/probes/{index}/outcome 与 /report/stop_reason 或最后完整事件不一致。"
                    )));
                }
                refs.outcome(
                    probe.outcome,
                    &probe.id,
                    &format!("/report/probes/{index}/outcome"),
                )?;
                if let Some(id) = probe.request_evidence_id {
                    refs.request(
                        id,
                        &probe.id,
                        outcome_time(probe.outcome),
                        &format!("/report/probes/{index}/request_evidence_id"),
                    )?;
                }
            }
            for (index, fact) in report.evidence.iter().enumerate() {
                refs.fact(
                    &fact.evidence,
                    &fact.probe_id,
                    report.report_version,
                    &format!("/report/evidence/{index}"),
                )?;
            }
            validate_trace(&refs, &report.trace, Some(report), "/report/trace")?;
            if let Some(trace) = trace {
                if !report.trace.entries.is_empty() && trace != &report.trace {
                    return Err(invalid("/trace 与 /report/trace 的已留存轨迹不一致。"));
                }
                if (trace.status == TraceStatus::Disabled)
                    != (report.trace.status == TraceStatus::Disabled)
                {
                    return Err(invalid("/trace 与 /report/trace 的轨迹采集设置不一致。"));
                }
            }
        }
    }
    if let Some(trace) = trace {
        let scenario = match report {
            RunReport::Legacy(_) => None,
            RunReport::Scenario(report) => Some(report),
        };
        validate_trace(&refs, trace, scenario, "/trace")?;
    }
    Ok(())
}

fn add_evidence<'a>(
    refs: &mut References<'a>,
    index: usize,
    probe: &'a str,
    evidence: &'a Evidence,
) -> Result<(), ArchiveError> {
    // ScenarioReport::evidence resolves IDs by their one-based publication position.
    if evidence.id.0 != index as u64 + 1 {
        return Err(invalid(format!(
            "/report/evidence/{index}/id 应为 {}，实际为 {}。",
            index + 1,
            evidence.id.0
        )));
    }
    if let EvidenceFact::Ipv4AclDecision { direction, .. } = evidence.fact
        && let Some(previous) = refs
            .frame_acls
            .insert((probe, evidence.frame.id.0), evidence)
    {
        let same_arrival = evidence.at_ns == previous.at_ns
            && evidence.frame == previous.frame
            && evidence.port.device_id == previous.port.device_id;
        let ingress_permitted = matches!(
            previous.fact,
            EvidenceFact::Ipv4AclDecision {
                direction: PortStage::Ingress,
                action: Ipv4AclAction::Permit,
                ..
            }
        );
        if !same_arrival || !ingress_permitted || direction != PortStage::Egress {
            return Err(invalid(format!(
                "/report/evidence/{index}/fact 与同一帧先前的 ACL 判定矛盾。"
            )));
        }
    }
    refs.evidence.insert(evidence.id.0, (probe, evidence));
    Ok(())
}

fn validate_tree(event: &StpEvent, index: usize) -> Result<(), ArchiveError> {
    let bridges: HashSet<_> = event
        .bridges
        .iter()
        .map(|bridge| bridge.bridge_id.as_str())
        .collect();
    let ports: HashSet<_> = event
        .links
        .iter()
        .flat_map(|link| link.ports.iter().map(|port| port.port_id.as_str()))
        .collect();
    for bridge in &event.bridges {
        if !bridges.contains(bridge.root_bridge_id.as_str()) {
            return Err(invalid(format!(
                "/report/stp_events/{index}/bridges 的 root_bridge_id {} 不存在。",
                bridge.root_bridge_id
            )));
        }
        if let Some(port) = &bridge.root_port_id
            && !ports.contains(port.as_str())
        {
            return Err(invalid(format!(
                "/report/stp_events/{index}/bridges 的 root_port_id {port} 不存在。"
            )));
        }
    }
    Ok(())
}

fn validate_trace(
    refs: &References<'_>,
    trace: &TraceReport,
    report: Option<&ScenarioReport>,
    path: &str,
) -> Result<(), ArchiveError> {
    if !trace.entries.is_empty()
        && matches!(trace.status, TraceStatus::Disabled | TraceStatus::Evicted)
    {
        return Err(invalid(format!("{path} 声明关闭或已淘汰，却仍包含轨迹。")));
    }
    let mut ids = HashSet::new();
    let mut previous_ns = 0;
    for (index, entry) in trace.entries.iter().enumerate() {
        let path = format!("{path}/entries/{index}");
        nonzero(entry.event_id.0, &format!("{path}/event_id"))?;
        if !ids.insert(entry.event_id.0) || entry.at_ns < previous_ns {
            return Err(invalid(format!("{path} 的事件重复或发生时间倒退。")));
        }
        previous_ns = entry.at_ns;
        if report.is_some_and(|report| {
            report.last_event_id.is_none() || entry.at_ns > report.last_event_ns
        }) {
            return Err(invalid(format!("{path} 晚于最后完整事件。")));
        }
        if let Some(probe) = &entry.probe_id
            && !refs.probes.contains_key(probe.as_str())
        {
            return Err(invalid(format!("{path}/probe_id 引用了未知探测 {probe}。")));
        }
        if let (Some(probe), Some(frame)) = (&entry.probe_id, entry.frame) {
            refs.check_acl_parent(probe, frame, &path)?;
            if let Some(stage) = refs.denied_at(probe, frame.id)
                && let Some(route) = &entry.route
                && (stage == PortStage::Ingress || route.ipv4_mtu.is_some())
            {
                return Err(invalid(format!(
                    "{path}/route 包含该帧被 ACL 拒绝后才会执行的检查。"
                )));
            }
        }
        let control_at = match entry.event {
            TraceEvent::LinkChange => Some(
                refs.link(entry.event_id, entry.at_ns, &format!("{path}/event_id"))?
                    .at_ns,
            ),
            TraceEvent::RoutingConvergence => {
                refs.route(entry.event_id, entry.at_ns, &format!("{path}/event_id"))?;
                Some(refs.routes[&entry.event_id.0].at_ns)
            }
            TraceEvent::StpConvergence => {
                refs.tree(entry.event_id, entry.at_ns, &format!("{path}/event_id"))?;
                Some(refs.trees[&entry.event_id.0])
            }
            _ => None,
        };
        if control_at.is_some_and(|at_ns| at_ns != entry.at_ns) {
            return Err(invalid(format!(
                "{path}/event_id 与控制事件的发生时刻不一致。"
            )));
        }
        if let Some(learned) = entry
            .route
            .as_ref()
            .and_then(|route| route.learned.as_ref())
        {
            refs.route(
                learned.convergence_event_id,
                entry.at_ns,
                &format!("{path}/route/learned/convergence_event_id"),
            )?;
            if refs.routes[&learned.convergence_event_id.0].router_id
                != entry
                    .port
                    .as_ref()
                    .map_or("", |port| port.device_id.as_str())
            {
                return Err(invalid(format!(
                    "{path}/route/learned/convergence_event_id 不属于查表设备。"
                )));
            }
        }
    }
    Ok(())
}
