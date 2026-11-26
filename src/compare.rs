use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock};

use serde::Serialize;

use crate::{AcceptedRun, Endpoint, EvidenceId, Outcome, Probe, RunReport, report::decimal};

#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct RunComparison {
    pub baseline: ComparedRun,
    pub candidate: ComparedRun,
    pub probes: Vec<ProbeComparison>,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct RunComparisonPage {
    pub baseline: ComparedRun,
    pub candidate: ComparedRun,
    pub page: crate::Page<ProbeComparison>,
}

pub(crate) struct ComparisonSnapshot {
    pub(crate) run: AcceptedRun,
    pub(crate) origin: crate::RunOrigin,
    pub(crate) report: Arc<RunReport>,
    pub(crate) index: Arc<OnceLock<Vec<usize>>>,
}

fn probe_id(report: &RunReport, index: usize) -> &str {
    match report {
        RunReport::Legacy(_) => "1",
        RunReport::Scenario(report) => &report.probes[index].id,
    }
}

impl ComparisonSnapshot {
    fn ordered(&self) -> &[usize] {
        self.index.get_or_init(|| {
            let len = match self.report.as_ref() {
                RunReport::Legacy(_) => 1,
                RunReport::Scenario(report) => report.probes.len(),
            };
            let mut index: Vec<_> = (0..len).collect();
            index.sort_unstable_by(|a, b| {
                probe_id(&self.report, *a).cmp(probe_id(&self.report, *b))
            });
            index
        })
    }

    fn description(&self) -> ComparedRun {
        let (topology_id, revision, model_version) = match self.report.as_ref() {
            RunReport::Legacy(report) => {
                (&report.topology_id, &report.revision, report.model_version)
            }
            RunReport::Scenario(report) => {
                (&report.topology_id, &report.revision, report.model_version)
            }
        };
        ComparedRun {
            run: self.run,
            topology_id: topology_id.clone(),
            revision: revision.clone(),
            model_version,
        }
    }

    fn probe(&self, index: usize) -> ComparedProbe {
        match self.report.as_ref() {
            RunReport::Legacy(report) => ComparedProbe::new(
                &report.probe,
                0,
                crate::topology::DEFAULT_TTL,
                None,
                report.outcome,
                None,
            ),
            RunReport::Scenario(report) => {
                let probe = &report.probes[index];
                ComparedProbe::new(
                    &probe.probe,
                    probe.at_ns,
                    probe.ttl,
                    probe.payload_bytes,
                    probe.outcome,
                    probe.request_evidence_id,
                )
            }
        }
    }
}

pub(crate) fn compare_page(
    before: ComparisonSnapshot,
    after: ComparisonSnapshot,
    offset: usize,
    count: usize,
) -> RunComparisonPage {
    let baseline = before.description();
    let candidate = after.description();
    let same_model = baseline.topology_id == candidate.topology_id
        && baseline.model_version == candidate.model_version;
    let left = before.ordered();
    let right = after.ordered();
    let (mut a, mut b, mut position) = (0, 0, 0);
    let mut entries = Vec::new();
    let count = count.min(128);
    while a < left.len() || b < right.len() {
        let old_id = left.get(a).map(|index| probe_id(&before.report, *index));
        let new_id = right.get(b).map(|index| probe_id(&after.report, *index));
        let (take_old, take_new) = match (old_id, new_id) {
            (Some(old), Some(new)) => (old <= new, new <= old),
            (Some(_), None) => (true, false),
            (None, Some(_)) => (false, true),
            (None, None) => unreachable!(),
        };
        if position >= offset {
            if entries.len() == count {
                break;
            }
            let old = take_old.then(|| before.probe(left[a]));
            let new = take_new.then(|| after.probe(right[b]));
            entries.push(ProbeComparison {
                probe_id: if take_old {
                    old_id.unwrap()
                } else {
                    new_id.unwrap()
                }
                .into(),
                same_probe_parameters: same_model
                    && old
                        .as_ref()
                        .zip(new.as_ref())
                        .is_some_and(|(a, b)| a.matches(b)),
                baseline: old,
                candidate: new,
            });
        }
        a += usize::from(take_old);
        b += usize::from(take_new);
        position += 1;
    }
    RunComparisonPage {
        baseline,
        candidate,
        page: crate::Page {
            next_offset: (!entries.is_empty() && (a < left.len() || b < right.len()))
                .then_some(position),
            entries,
        },
    }
}

#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct ComparedRun {
    pub run: AcceptedRun,
    pub topology_id: String,
    pub revision: String,
    pub model_version: u32,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct ProbeComparison {
    pub probe_id: String,
    pub baseline: Option<ComparedProbe>,
    pub candidate: Option<ComparedProbe>,
    pub same_probe_parameters: bool,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct ComparedProbe {
    #[serde(flatten)]
    pub probe: Probe,
    #[serde(serialize_with = "decimal")]
    pub at_ns: u64,
    pub ttl: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload_bytes: Option<u16>,
    pub outcome: Outcome,
    pub request_evidence_id: Option<EvidenceId>,
}

impl ComparedProbe {
    fn new(
        probe: &Probe,
        at_ns: u64,
        ttl: u8,
        payload_bytes: Option<u16>,
        outcome: Outcome,
        request_evidence_id: Option<EvidenceId>,
    ) -> Self {
        Self {
            probe: probe.clone(),
            at_ns,
            ttl,
            payload_bytes,
            outcome,
            request_evidence_id,
        }
    }

    fn matches(&self, other: &Self) -> bool {
        // Reports may omit the default Echo payload without changing probe parameters.
        const DEFAULT_ECHO_PAYLOAD_BYTES: u16 = 56;
        let same_endpoint =
            |a: &Endpoint, b: &Endpoint| a.device_id == b.device_id && a.ipv4 == b.ipv4;
        same_endpoint(&self.probe.source, &other.probe.source)
            && same_endpoint(&self.probe.destination, &other.probe.destination)
            && self.at_ns == other.at_ns
            && self.probe.deadline_ns == other.probe.deadline_ns
            && self.ttl == other.ttl
            && self.payload_bytes.unwrap_or(DEFAULT_ECHO_PAYLOAD_BYTES)
                == other.payload_bytes.unwrap_or(DEFAULT_ECHO_PAYLOAD_BYTES)
    }
}

fn describe(run: AcceptedRun, report: &RunReport) -> (ComparedRun, Vec<(&str, ComparedProbe)>) {
    let (topology, revision, model_version, probes) = match report {
        RunReport::Legacy(report) => (
            &report.topology_id,
            &report.revision,
            report.model_version,
            vec![(
                "1",
                ComparedProbe::new(
                    &report.probe,
                    0,
                    crate::topology::DEFAULT_TTL,
                    None,
                    report.outcome,
                    None,
                ),
            )],
        ),
        RunReport::Scenario(report) => (
            &report.topology_id,
            &report.revision,
            report.model_version,
            report
                .probes
                .iter()
                .map(|probe| {
                    (
                        probe.id.as_str(),
                        ComparedProbe::new(
                            &probe.probe,
                            probe.at_ns,
                            probe.ttl,
                            probe.payload_bytes,
                            probe.outcome,
                            probe.request_evidence_id,
                        ),
                    )
                })
                .collect(),
        ),
    };
    (
        ComparedRun {
            run,
            topology_id: topology.clone(),
            revision: revision.clone(),
            model_version,
        },
        probes,
    )
}

pub(crate) fn compare_runs(
    baseline: (AcceptedRun, &RunReport),
    candidate: (AcceptedRun, &RunReport),
) -> RunComparison {
    let (baseline, before) = describe(baseline.0, baseline.1);
    let (candidate, after) = describe(candidate.0, candidate.1);
    let same_model = baseline.topology_id == candidate.topology_id
        && baseline.model_version == candidate.model_version;
    let mut pairs: BTreeMap<_, (Option<_>, Option<_>)> = BTreeMap::new();
    for (id, probe) in before {
        pairs.entry(id).or_default().0 = Some(probe);
    }
    for (id, probe) in after {
        pairs.entry(id).or_default().1 = Some(probe);
    }
    let probes = pairs
        .into_iter()
        .map(|(id, (before, after))| ProbeComparison {
            probe_id: id.into(),
            same_probe_parameters: same_model
                && before
                    .as_ref()
                    .zip(after.as_ref())
                    .is_some_and(|(left, right)| left.matches(right)),
            baseline: before,
            candidate: after,
        })
        .collect();
    RunComparison {
        baseline,
        candidate,
        probes,
    }
}
