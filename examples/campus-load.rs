use std::{
    env,
    error::Error,
    hint::black_box,
    io,
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration, Instant},
};

use reliner::{
    AcceptedRun, DocumentId, Evidence, EvidenceFact, Outcome, ProbeReport, RunControl, RunLimits,
    RunMode, RunReport, RunStatus, RunSummary, ScenarioReport, TraceStatus, Workspace,
    WorkspaceConfig, WorkspaceError, run_lab, run_lab_with_limits,
};
use serde_json::{Value, json};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

const CAMPUS: &str = include_str!("campus-lab.json");
const DOCUMENTS: usize = 64;
const BACKGROUND_RUNS: usize = 4;
const CONTROL_ROUNDS: usize = 3;
const BASE_PROBES: usize = 7;
const BURST_SIZE: usize = 3;
const FIRST_BURST_NS: u64 = 22_500;
const BURST_PERIOD_NS: u64 = 6_000;
const PROBE_INTERVAL_NS: u64 = 500;
const BURST_DEADLINE_NS: u64 = 5_000;

fn trunk(id: &str, cost: u32) -> Value {
    json!({
        "id": id, "name": id, "stp_cost": cost,
        "mode": {"kind": "trunk", "allowed_vlans": [4094]}
    })
}

fn append_link(doc: &mut Value, id: String, left: String, right: String) {
    doc["links"].as_array_mut().unwrap().push(json!({
        "id": id, "ports": [left, right], "delay_ns": "100"
    }));
}

fn add_peer(router: &mut Value, port: &str, hop: usize, side: usize) {
    let mac = 2 * hop + side;
    let ip = format!("10.1.{}.{}", hop / 64, 4 * (hop % 64) + 1 + side);
    router["ports"].as_array_mut().unwrap().push(json!({
        "id": port, "name": port, "ipv4": ip, "prefix_length": 30,
        "mac": format!("02:ca:00:00:{:02x}:{:02x}", mac / 256, mac % 256)
    }));
    router["link_state"]["peers"]
        .as_array_mut()
        .unwrap()
        .push(json!({
            "port": port, "cost": 10
        }));
}

fn campus(scale: usize) -> Result<Value> {
    let mut doc: Value = serde_json::from_str(CAMPUS)?;
    doc["scenario"]["trace_limit"] = json!(0);
    let devices = doc["devices"].as_array().unwrap();
    let mut switch = devices
        .iter()
        .position(|device| device["id"] == "sw1")
        .unwrap();
    let mut router = devices
        .iter()
        .position(|device| device["id"] == "r2")
        .unwrap();
    for hop in 0..scale {
        let switch_id = format!("branch-sw-{hop:03}");
        let router_id = format!("branch-r-{hop:03}");
        let mut next_switch = json!({
            "kind": "switch", "id": switch_id, "name": switch_id, "ports": []
        });
        let mut next_router = json!({
            "kind": "router", "id": router_id, "name": router_id, "ports": [], "routes": [],
            "link_state": {"peers": [], "advertise": []}
        });

        // Two trunks per hop keep the branch redundant without extending the probe VLANs.
        for (path, cost) in [("main", 10), ("alternate", 20)] {
            let link = format!("{switch_id}-{path}");
            let left = format!("{link}-left");
            let right = format!("{link}-right");
            doc["devices"][switch]["ports"]
                .as_array_mut()
                .unwrap()
                .push(trunk(&left, cost));
            next_switch["ports"]
                .as_array_mut()
                .unwrap()
                .push(trunk(&right, cost));
            append_link(&mut doc, link, left, right);
        }
        let left = format!("{router_id}-left");
        let right = format!("{router_id}-right");
        add_peer(&mut doc["devices"][router], &left, hop, 0);
        add_peer(&mut next_router, &right, hop, 1);
        append_link(&mut doc, router_id, left, right);

        let devices = doc["devices"].as_array_mut().unwrap();
        switch = devices.len();
        router = switch + 1;
        devices.extend([next_switch, next_router]);
    }
    Ok(doc)
}

fn add_bursts(doc: &mut Value, bursts: usize) {
    if bursts == 0 {
        return;
    }
    let scenario = &mut doc["scenario"];
    let shift = (bursts - 1) as u64 * BURST_PERIOD_NS;
    let shift_time = |time: &mut Value| {
        let at_ns = time.as_str().unwrap().parse::<u64>().unwrap();
        *time = json!((at_ns + shift).to_string());
    };
    shift_time(&mut scenario["end_ns"]);
    for action in scenario["link_actions"].as_array_mut().unwrap() {
        if action["state"] == "up" {
            shift_time(&mut action["at_ns"]);
        }
    }
    let probes = scenario["probes"].as_array_mut().unwrap();
    assert_eq!(probes.len(), BASE_PROBES);
    assert!(probes.len() + bursts * BURST_SIZE <= usize::from(u16::MAX));
    let restored = probes
        .iter_mut()
        .find(|probe| probe["id"] == "restored")
        .unwrap();
    shift_time(&mut restored["at_ns"]);
    shift_time(&mut restored["deadline_ns"]);

    // The warmed 1 Gbit/s path serializes these 102-byte frames in 816 ns.
    // The second request hits the queue-less link busy, and the third clears it.
    for burst in 0..bursts {
        let start = FIRST_BURST_NS + burst as u64 * BURST_PERIOD_NS;
        let deadline = (start + BURST_DEADLINE_NS).to_string();
        for slot in 0..BURST_SIZE {
            let index = burst * BURST_SIZE + slot + 1;
            probes.push(json!({
                "id": format!("dense-{index:05}"), "source": "pc-a", "destination": "pc-b",
                "at_ns": (start + slot as u64 * PROBE_INTERVAL_NS).to_string(),
                "deadline_ns": deadline, "payload_bytes": 56
            }));
        }
    }
}

struct Workload {
    stp_round: usize,
    routing_event: usize,
    routers: usize,
    essential_facts_upper_bound: usize,
    bursts: usize,
}

impl Workload {
    fn new(scale: usize, dense: bool) -> Self {
        // The fixture adds two devices, six ports and three links per hop.
        let heap = |entries: usize| entries * (1 + entries.ilog2() as usize);
        let bridges = 3 + scale;
        let stp_links = 3 + 2 * scale;
        let routers = 3 + scale;
        let advertisements = 2;
        let routing_links = 3 + scale;
        Self {
            stp_round: 1
                + 4 * (bridges + 2)
                + 3 * (stp_links + 2)
                + 5 * bridges
                + 7 * stp_links
                + 2
                + 2 * heap(3 + scale),
            routing_event: 1
                + 2 * routers
                + 2 * routing_links
                // The physical triangle plus one chain gives a heap capacity of two or three.
                + 4 * (routing_links + 1)
                + 4 * advertisements
                + heap(advertisements),
            routers,
            essential_facts_upper_bound: CONTROL_ROUNDS * (1 + bridges + 3 * stp_links + routers)
                + 4,
            bursts: if dense { 1 + 4 * scale } else { 0 },
        }
    }

    fn total(&self) -> usize {
        let publications = CONTROL_ROUNDS * self.routers;
        let history_copies = (4..publications)
            .filter(|len| len.is_power_of_two())
            .sum::<usize>();
        CONTROL_ROUNDS * self.stp_round + publications * self.routing_event + history_copies
    }

    fn limits(&self) -> RunLimits {
        let defaults = WorkspaceConfig::default().run_limits;
        let added = self.bursts * BURST_SIZE;
        // Added probes use a fixed path and the same two advertised prefixes at every scale.
        let probe_work = 128;
        RunLimits {
            event_work: defaults
                .event_work
                .max(self.stp_round)
                .max(self.routing_event + CONTROL_ROUNDS * self.routers),
            total_work: defaults.total_work + self.total() + probe_work * added,
            essential_facts: defaults.essential_facts
                + self.essential_facts_upper_bound
                + 2 * added,
            ..defaults
        }
    }

    fn json(&self) -> Value {
        json!({
            "stp_round_work": self.stp_round, "routing_event_work": self.routing_event,
            "routers": self.routers, "scheduled_control_work": self.total(),
            "scheduled_control_and_link_facts": self.essential_facts_upper_bound,
            "scheduled_control_and_link_facts_is_upper_bound": true
        })
    }
}

#[derive(Default)]
struct Samples(Vec<Duration>);

impl Samples {
    fn summarize(mut self) -> Value {
        self.0.sort_unstable();
        let count = self.0.len();
        let rank = (count * 99).div_ceil(100);
        json!({
            "samples": count,
            "p99_ms": rank.checked_sub(1).map(|index| ms(self.0[index])),
            "max_ms": self.0.last().copied().map(ms)
        })
    }
}

fn ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

fn scenario(report: &RunReport) -> &ScenarioReport {
    match report {
        RunReport::Scenario(report) => report,
        _ => unreachable!("campus is a version 2 scenario"),
    }
}

fn assert_complete(report: &ScenarioReport, work: &Workload) {
    assert_eq!(report.stop_reason, None, "{}", report_end(report));
    assert_eq!(report.trace.status, TraceStatus::Disabled);
    assert_eq!(report.probes.len(), BASE_PROBES + work.bursts * BURST_SIZE);
    assert_eq!(report.stp_events.len(), CONTROL_ROUNDS);
    assert_eq!(report.routing_events.len(), CONTROL_ROUNDS * work.routers);
    assert!(report.evidence.iter().any(|fact| {
        fact.probe_id == "burst-reply"
            && fact.evidence.at_ns == 19_228
            && matches!(fact.evidence.fact, EvidenceFact::TransmitQueueFull { .. })
    }));
    if work.bursts == 0 {
        return;
    }
    let failover = report
        .routing_events
        .iter()
        .find(|event| event.router_id == "r1" && event.at_ns == 11_500)
        .unwrap();
    for burst in report.probes[BASE_PROBES..].chunks_exact(BURST_SIZE) {
        for probe in [&burst[0], &burst[2]] {
            assert!(
                matches!(probe.outcome, Outcome::Reachable { .. }),
                "{probe:?}"
            );
        }
        let drop = queue_full(report, &burst[1]);
        assert!(
            drop.is_some_and(|drop| {
                drop.port.port_id == "r1-r2" && drop.routing_event_id == Some(failover.event_id)
            }),
            "{:?}",
            burst[1]
        );
    }
}

fn queue_full<'a>(report: &'a ScenarioReport, probe: &ProbeReport) -> Option<&'a Evidence> {
    let Outcome::Timeout {
        cause: Some(id), ..
    } = probe.outcome
    else {
        return None;
    };
    report
        .evidence(id)
        .filter(|evidence| matches!(evidence.fact, EvidenceFact::TransmitQueueFull { .. }))
}

fn probe_counts(report: &ScenarioReport, probes: &[ProbeReport]) -> Value {
    let (mut reachable, mut timed_out, mut incomplete, mut queue_full_count) = (0, 0, 0, 0);
    for probe in probes {
        match probe.outcome {
            Outcome::Reachable { .. } => reachable += 1,
            Outcome::Timeout { .. } => {
                timed_out += 1;
                queue_full_count += usize::from(queue_full(report, probe).is_some());
            }
            Outcome::Incomplete { .. } => incomplete += 1,
            Outcome::Undetermined { .. } => {
                unreachable!("generated native campus has no import gaps")
            }
        }
    }
    json!({
        "total": probes.len(), "completed": reachable + timed_out, "incomplete": incomplete,
        "reachable": reachable, "timed_out": timed_out, "queue_full": queue_full_count
    })
}

fn evidence_counts(report: &ScenarioReport) -> Value {
    let (mut requests, mut replies, mut arp, mut other) = (0, 0, 0, 0);
    for fact in &report.evidence {
        match fact.evidence.fact {
            EvidenceFact::EchoRequestReceived => requests += 1,
            EvidenceFact::EchoReplyReceived => replies += 1,
            EvidenceFact::ArpResolved => arp += 1,
            _ => other += 1,
        }
    }
    let links = report
        .link_events
        .iter()
        .map(|event| 1 + usize::from(event.simultaneous.is_some()))
        .sum::<usize>();
    let routing = report.routing_events.len();
    let mut stp = 0;
    let mut previous: Option<reliner::StpEvent> = None;
    for event in report.stp_events.iter() {
        let (bridges, links) = if let Some(old) = &previous {
            (
                old.bridges
                    .iter()
                    .zip(&event.bridges)
                    .filter(|(a, b)| a != b)
                    .count(),
                old.links
                    .iter()
                    .zip(&event.links)
                    .filter(|(a, b)| a != b)
                    .count(),
            )
        } else {
            (event.bridges.len(), event.links.len())
        };
        stp += 1 + bridges + 3 * links;
        previous = Some(event);
    }
    json!({
        "echo_requests_received": requests, "echo_replies_received": replies, "arp_resolved": arp,
        "other_probe_evidence": other, "link": links, "routing": routing, "stp": stp,
        "retained": requests + replies + arp + other + links + routing + stp
    })
}

fn report_end(report: &ScenarioReport) -> Value {
    json!({
        "status": if report.stop_reason.is_some() { RunStatus::Incomplete } else { RunStatus::Finished },
        "stop_reason": report.stop_reason, "last_event_id": report.last_event_id,
        "last_event_ns": report.last_event_ns.to_string(),
        "probes": probe_counts(report, &report.probes), "essential_evidence": evidence_counts(report),
        "burst_probes": probe_counts(report, &report.probes[BASE_PROBES..])
    })
}

fn single(input: &str, iterations: usize, work: &Workload) -> Result<Value> {
    let limits = work.limits();
    let mut times = Samples(Vec::with_capacity(iterations));
    let start = Instant::now();
    let mut last = None;
    for iteration in 0..iterations {
        let measured = Instant::now();
        let report = black_box(run_lab_with_limits(black_box(input), limits)?);
        times.0.push(measured.elapsed());
        if iteration + 1 == iterations {
            last = Some(report);
        }
    }
    let elapsed = start.elapsed();
    let report = scenario(last.as_ref().unwrap());
    assert_complete(report, work);
    Ok(json!({
        "iterations": iterations, "elapsed_ms": ms(elapsed),
        "run": times.summarize(), "end": report_end(report), "limits": limits
    }))
}

fn wait_until(
    workspace: &Workspace,
    mut summary: RunSummary,
    wanted: impl Fn(RunStatus) -> bool,
) -> std::result::Result<RunSummary, WorkspaceError> {
    let start = Instant::now();
    while !summary.status.is_terminal() && !wanted(summary.status) {
        assert!(start.elapsed() < Duration::from_secs(30), "{summary:?}");
        summary =
            workspace.wait_for_update(summary.run.id, summary.update, Duration::from_millis(25))?;
    }
    Ok(summary)
}

fn compare_workspace(
    input: &str,
    limits: RunLimits,
    complete: Option<&Workload>,
) -> Result<(Value, Value)> {
    let config = WorkspaceConfig {
        run_limits: limits,
        ..WorkspaceConfig::default()
    };
    let workspace = Workspace::new(config)?;
    let doc = workspace.open_document(input.to_owned())?;
    let measured = Instant::now();
    let reference = run_lab_with_limits(input, limits)?;
    let standalone_ms = ms(measured.elapsed());
    let measured = Instant::now();
    let run = workspace.submit_run(doc.id, RunMode::Automatic)?;
    let summary = wait_until(
        &workspace,
        workspace.run_summary(run.id)?,
        RunStatus::is_terminal,
    )?;
    let workspace_report = workspace.run_report(run.id)?;
    let workspace_ms = ms(measured.elapsed());
    assert_eq!(*workspace_report, reference);
    let report = scenario(&workspace_report);
    if let Some(work) = complete {
        assert_complete(report, work);
    }
    let end = report_end(report);
    assert_eq!(end["status"], json!(summary.status));
    assert_eq!(summary.probe_counts, Some(report.probe_counts()));
    let stop_limit = match report.stop_reason {
        Some(reliner::StopReason::ResourceLimit { limit }) => Some(limits.maximum(limit)),
        _ => None,
    };
    assert_eq!(summary.stop_limit, stop_limit);
    assert_eq!(summary.last_event_id, report.last_event_id);
    assert_eq!(summary.last_event_ns, report.last_event_ns);
    assert_eq!(summary.stop_reason, report.stop_reason);
    Ok((
        end,
        json!({"workspace_ms": workspace_ms, "standalone_ms": standalone_ms}),
    ))
}

fn defaults(input: &str, work: &Workload) -> Result<Value> {
    let start = Instant::now();
    let standalone = run_lab(input)?;
    let cli_ms = ms(start.elapsed());
    let standalone = scenario(&standalone);
    let limits = WorkspaceConfig::default().run_limits;
    let (end, latency) = compare_workspace(input, limits, None)?;
    let mut result = json!({
        "workspace_end": end, "cli_end": report_end(standalone),
        "workspace_matches_standalone": true,
        "standalone_limits": standalone.settings.limits, "workspace_limits": limits,
        "cli_ms": cli_ms, "same_limits_latency": latency
    });
    if work.bursts != 0 {
        let limits = work.limits();
        let (end, latency) = compare_workspace(input, limits, Some(work))?;
        result["benchmark_comparison"] = json!({
            "end": end, "limits": limits, "latency": latency, "workspace_matches_standalone": true
        });
    }
    if end["stop_reason"]["resource_limit"]["limit"] == "event_work"
        && work.stp_round > limits.event_work
    {
        let raised = RunLimits {
            event_work: work.stp_round,
            ..limits
        };
        let (after, latency) = compare_workspace(input, raised, None)?;
        result["stp_event_limit_comparison"] =
            json!({"end": after, "limits": raised, "latency": latency});
    }
    result["elapsed_ms"] = json!(ms(start.elapsed()));
    Ok(result)
}

#[derive(Default)]
struct Pauses {
    times: Samples,
    attempted: usize,
    lost_races: usize,
    before_first_event: usize,
}

impl Pauses {
    fn issue(
        &mut self,
        workspace: &Workspace,
        summary: RunSummary,
    ) -> std::result::Result<RunSummary, WorkspaceError> {
        self.attempted += 1;
        let run = summary.run.id;
        let start = Instant::now();
        match workspace.control_run(run, RunControl::Pause) {
            Ok(()) => {}
            Err(WorkspaceError::InvalidControl { status, .. }) if status.is_terminal() => {
                self.lost_races += 1;
                return workspace.run_summary(run);
            }
            Err(error) => return Err(error),
        }
        let summary = wait_until(workspace, summary, |status| status == RunStatus::Paused)?;
        if summary.status == RunStatus::Paused {
            if summary.last_event_id.is_some() {
                self.times.0.push(start.elapsed());
            } else {
                self.before_first_event += 1;
            }
            workspace.control_run(run, RunControl::Resume)?;
        } else {
            self.lost_races += 1;
        }
        Ok(summary)
    }

    fn summarize(self) -> Value {
        json!({
            "attempted": self.attempted, "lost_races": self.lost_races,
            "before_first_event": self.before_first_event, "latency": self.times.summarize()
        })
    }
}

struct Background {
    during_measurement: usize,
    completed: usize,
    finished_without_pause_attempt: usize,
    last: RunSummary,
    pauses: Pauses,
}

fn run_background(
    workspace: &Workspace,
    document: DocumentId,
    reference: &RunReport,
    mut slots: Vec<Option<(AcceptedRun, bool)>>,
    done: &AtomicBool,
) -> std::result::Result<Background, WorkspaceError> {
    let mut during_measurement = 0;
    let mut completed = 0;
    let mut finished_without_pause_attempt = 0;
    let mut last = None;
    let mut pauses = Pauses::default();
    while slots.iter().any(Option::is_some) {
        for slot in &mut slots {
            let Some((run, attempted)) = slot else {
                continue;
            };
            let mut summary = workspace.run_summary(run.id)?;
            if !summary.status.is_terminal() && (summary.status != RunStatus::Running || *attempted)
            {
                summary =
                    workspace.wait_for_update(run.id, summary.update, Duration::from_millis(1))?;
            }
            if summary.status == RunStatus::Running && !*attempted && !done.load(Ordering::Acquire)
            {
                *attempted = true;
                summary = pauses.issue(workspace, summary)?;
            }
            if summary.status.is_terminal() {
                let status = if reference.is_incomplete() {
                    RunStatus::Incomplete
                } else {
                    RunStatus::Finished
                };
                assert_eq!(summary.status, status, "{summary:?}");
                assert_eq!(summary.probe_counts, Some(reference.probe_counts()));
                assert_eq!(workspace.run_report(run.id)?.as_ref(), reference);
                completed += 1;
                finished_without_pause_attempt += usize::from(!*attempted);
                last = Some(summary);
                *slot = if done.load(Ordering::Acquire) {
                    None
                } else {
                    during_measurement += 1;
                    Some((workspace.submit_run(document, RunMode::Automatic)?, false))
                };
            }
        }
    }
    Ok(Background {
        during_measurement,
        completed,
        finished_without_pause_attempt,
        last: last.expect("background pool completed runs"),
        pauses,
    })
}

fn mixed(input: String, iterations: usize, work: &Workload, default_limits: bool) -> Result<Value> {
    let config = WorkspaceConfig {
        run_limits: if default_limits {
            WorkspaceConfig::default().run_limits
        } else {
            work.limits()
        },
        ..WorkspaceConfig::default()
    };
    let workspace = Workspace::new(config)?;
    let reference = run_lab_with_limits(&input, config.run_limits)?;
    if !default_limits {
        assert_complete(scenario(&reference), work);
    }
    let mut expanded: Value = serde_json::from_str(&input)?;
    let loading = workspace.open_document(input)?;
    let loading_id = loading.id;
    let mut base = campus(0)?;
    let rated = base["links"]
        .as_array()
        .unwrap()
        .iter()
        .position(|link| link["id"] == "r1-r2")
        .unwrap();
    let mut documents = Vec::with_capacity(DOCUMENTS);
    documents.push((loading, None));
    for index in 1..DOCUMENTS {
        let draft = if index == 1 { &mut expanded } else { &mut base };
        draft["id"] = json!(format!("campus-ui-{index:02}"));
        let original = draft.to_string();
        let edits = (index % 4 == 1).then(|| {
            draft["links"][rated]["transmission"]["queue_frames"] = json!(1);
            let edited = draft.to_string();
            draft["links"][rated]["transmission"]["queue_frames"] = json!(0);
            [edited, original.clone()]
        });
        documents.push((workspace.open_document(original)?, edits));
    }
    let mut slots = Vec::with_capacity(BACKGROUND_RUNS);
    for _ in 0..BACKGROUND_RUNS {
        slots.push(Some((
            workspace.submit_run(loading_id, RunMode::Automatic)?,
            false,
        )));
    }
    let mut switches = Samples(Vec::with_capacity(iterations * DOCUMENTS));
    let mut large_edits = Samples(Vec::with_capacity(iterations));
    let mut small_edits = Samples(Vec::with_capacity(iterations * (DOCUMENTS / 4 - 1)));
    let done = AtomicBool::new(false);
    let (elapsed, drain, background) = thread::scope(|scope| -> Result<_> {
        let worker =
            scope.spawn(|| run_background(&workspace, loading_id, &reference, slots, &done));
        let start = Instant::now();
        let foreground = (|| -> Result<()> {
            for round in 0..iterations {
                for (index, (doc, edits)) in documents.iter_mut().enumerate() {
                    let measured = Instant::now();
                    black_box(workspace.document(doc.id)?);
                    switches.0.push(measured.elapsed());
                    if let Some(edits) = edits {
                        let replacement = edits[round % 2].clone();
                        let measured = Instant::now();
                        *doc = workspace.replace_experiment(doc.id, doc.revision, replacement)?;
                        let samples = if index == 1 {
                            &mut large_edits
                        } else {
                            &mut small_edits
                        };
                        samples.0.push(measured.elapsed());
                    }
                }
            }
            Ok(())
        })();
        let elapsed = start.elapsed();
        done.store(true, Ordering::Release);
        let drain = Instant::now();
        let background = worker.join().expect("background driver panicked")?;
        foreground?;
        Ok((elapsed, drain.elapsed(), background))
    })?;
    let report = workspace.run_report(background.last.run.id)?;
    let report = scenario(&report);
    if !default_limits {
        assert_complete(report, work);
    }
    Ok(json!({
        "iterations": iterations, "elapsed_ms": ms(elapsed), "drain_ms": ms(drain),
        "documents": DOCUMENTS, "document_bytes": workspace.document_usage().1,
        "switch": switches.summarize(),
        "config_large": large_edits.summarize(), "config_small": small_edits.summarize(),
        "end": report_end(report), "limits": config.run_limits, "stats": workspace.stats(),
        "background": {
            "workers": config.workers, "pool_size": BACKGROUND_RUNS,
            "completed_during_measurement": background.during_measurement,
            "completed": background.completed,
            "finished_without_pause_attempt": background.finished_without_pause_attempt,
            "pause": background.pauses.summarize()
        }
    }))
}

fn main() -> Result<()> {
    let args: Vec<_> = env::args().skip(1).collect();
    let usage = || {
        io::Error::other(
            "用法  campus-load <single|mixed|default|dense-single|dense-mixed|dense-mixed-default|dense-default> <0..256> [迭代数]",
        )
    };
    let [kind, scale, rest @ ..] = args.as_slice() else {
        return Err(usage().into());
    };
    let scale: usize = scale.parse()?;
    if scale > 256 || rest.len() > 1 {
        return Err(usage().into());
    }
    let dense = kind.strip_prefix("dense-");
    let mode = dense.unwrap_or(kind);
    let default_iterations = match mode {
        "single" => 100,
        "mixed" | "mixed-default" => 8192,
        "default" if rest.is_empty() => 1,
        _ => return Err(usage().into()),
    };
    let iterations = rest
        .first()
        .map(|value| value.parse::<usize>())
        .transpose()?
        .unwrap_or(default_iterations);
    if iterations == 0 {
        return Err(usage().into());
    }
    let work = Workload::new(scale, dense.is_some());
    let mut doc = campus(scale)?;
    add_bursts(&mut doc, work.bursts);
    let input = doc.to_string();
    let input_bytes = input.len();
    let result = match mode {
        "single" => single(&input, iterations, &work)?,
        "mixed" | "mixed-default" => mixed(input, iterations, &work, mode == "mixed-default")?,
        "default" => defaults(&input, &work)?,
        _ => unreachable!(),
    };
    println!(
        "{}",
        json!({
            "mode": kind, "scale": scale, "input_bytes": input_bytes,
            "control_work": work.json(), "bursts": work.bursts, "result": result
        })
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_settles_all_three_control_rounds_under_the_targeted_limits() {
        for scale in [101, 131, 256] {
            let work = Workload::new(scale, false);
            let doc = campus(scale).unwrap().to_string();
            let mut limits = WorkspaceConfig::default().run_limits;
            if scale == 256 {
                limits.event_work = work.stp_round;
            }
            let (end, _) = compare_workspace(&doc, limits, Some(&work)).unwrap();
            assert_eq!(end["probes"]["completed"], 7);
            let full = run_lab_with_limits(&doc, work.limits()).unwrap();
            let limited = run_lab_with_limits(&doc, limits).unwrap();
            let (full, limited) = (scenario(&full), scenario(&limited));
            assert_eq!(limited.stp_events, full.stp_events);
            assert_eq!(limited.routing_events, full.routing_events);
            assert_eq!(limited.link_events, full.link_events);
            assert_eq!(limited.evidence, full.evidence);
            assert_eq!(limited.probes, full.probes);
            assert_eq!(limited.last_event_id, full.last_event_id);
        }
    }

    #[test]
    fn workspace_default_settles_all_874_campus_probes_with_referenced_evidence() {
        let work = Workload::new(72, true);
        let mut doc = campus(72).unwrap();
        add_bursts(&mut doc, work.bursts);
        let (end, _) = compare_workspace(
            &doc.to_string(),
            WorkspaceConfig::default().run_limits,
            Some(&work),
        )
        .unwrap();
        assert_eq!(end["probes"]["completed"], 874);
        assert_eq!(end["essential_evidence"]["retained"], 2216);
    }

    #[test]
    fn dense_520_device_campus_reaches_the_evidence_boundary_with_default_total_work() {
        let work = Workload::new(256, true);
        let mut doc = campus(256).unwrap();
        add_bursts(&mut doc, work.bursts);
        let input = doc.to_string();
        let limits = RunLimits {
            event_work: work.stp_round,
            ..WorkspaceConfig::default().run_limits
        };
        for (essential_facts, completed, at_ns, event_id) in [
            (limits.essential_facts, 1059, "2129300", "18002"),
            (7735, 3081, "6177200", "39824"),
        ] {
            let (end, _) = compare_workspace(
                &input,
                RunLimits {
                    essential_facts,
                    ..limits
                },
                None,
            )
            .unwrap();
            assert_eq!(
                end["stop_reason"],
                json!({"resource_limit":{"limit":"essential_facts"}})
            );
            assert_eq!(end["essential_evidence"]["retained"], essential_facts);
            assert_eq!(end["probes"]["completed"], completed);
            assert_eq!(end["last_event_ns"], at_ns);
            assert_eq!(end["last_event_id"], event_id);
        }

        let enough = RunLimits {
            essential_facts: 7736,
            ..limits
        };
        let (end, _) = compare_workspace(&input, enough, Some(&work)).unwrap();
        assert_eq!(end["probes"]["completed"], 3082);
        assert_eq!(
            end["essential_evidence"]["retained"],
            enough.essential_facts
        );
        let full = run_lab_with_limits(&input, enough).unwrap();
        let limited = run_lab_with_limits(
            &input,
            RunLimits {
                essential_facts: enough.essential_facts - 1,
                ..limits
            },
        )
        .unwrap();
        let (full, limited) = (scenario(&full), scenario(&limited));
        assert_eq!(limited.evidence, full.evidence[..full.evidence.len() - 1]);
        assert_eq!(limited.stp_events, full.stp_events);
        assert_eq!(limited.routing_events, full.routing_events);
        assert_eq!(limited.link_events, full.link_events);
        assert_eq!(
            limited
                .probes
                .iter()
                .filter(|probe| matches!(probe.outcome, Outcome::Incomplete { .. }))
                .map(|probe| probe.id.as_str())
                .collect::<Vec<_>>(),
            ["restored"]
        );
    }
}
