use std::{
    env, io, thread,
    time::{Duration, Instant},
};

use reliner::{
    AcceptedRun, DocumentInfo, Outcome, RunControl, RunMode, RunReport, RunStatus, Workspace,
    WorkspaceConfig, WorkspaceError, run_lab_with_limits,
};
use serde::Serialize;
use serde_json::json;

const EXPECTED_PORT: &str = r#""id":"tail","mode":{"kind":"access","vlan":10}"#;
const EDITED_PORT: &str = r#""id":"tail","mode":{"kind":"access","vlan":20}"#;
const MEMORY_INTERVAL: Duration = Duration::from_secs(1);

fn topology(name: &str, count: usize, probes: usize) -> String {
    let mut devices = Vec::new();
    let mut ports = Vec::new();
    let mut links = Vec::new();
    for host in 0..count - 1 {
        let jack = if host == count - 2 {
            "tail".into()
        } else {
            format!("sw-{host}")
        };
        ports.push(json!({"id": jack, "name": jack, "mode": {"kind": "access", "vlan": 10}}));
        links.push(json!({"id": format!("link-{host}"), "ports": [jack, format!("eth-{host}")], "delay_ns": "10"}));
        devices.push(json!({
            "kind": "host", "id": format!("pc-{host}"), "name": format!("PC-{host}"),
            "ipv4": format!("10.0.{}.{host_part}", host / 254, host_part = host % 254 + 1),
            "prefix_length": 16, "mac": format!("02:00:00:00:{:02x}:{:02x}", host / 256, host % 256),
            "port": {"id": format!("eth-{host}"), "name": "eth0"}
        }));
    }
    devices.push(json!({"kind": "switch", "id": "sw", "name": "SW", "ports": ports}));
    let probes: Vec<_> = (0..probes).map(|probe| json!({
        "id": format!("ping-{probe}"), "source": "pc-0", "destination": "pc-1", "at_ns": "1000", "deadline_ns": "1500"
    })).collect();
    json!({
        "format_version": 2, "id": name, "revision": "1", "devices": devices, "links": links,
        "scenario": {"end_ns": "1500", "arp_lifetime_ns": "1000000", "mac_lifetime_ns": "1000000", "probes": probes}
    }).to_string()
}

enum Samples {
    Exact(Vec<Duration>),
    Bounded {
        bins: Box<[usize]>,
        count: usize,
        max: Duration,
    },
}

#[derive(Serialize)]
struct Latency {
    count: usize,
    p95_ms: f64,
    p99_ms: f64,
    max_ms: f64,
}

impl Samples {
    fn new(bounded: bool) -> Self {
        if bounded {
            Self::Bounded {
                bins: vec![0; 64 * 16].into_boxed_slice(),
                count: 0,
                max: Duration::ZERO,
            }
        } else {
            Self::Exact(Vec::new())
        }
    }

    fn add(&mut self, measured: Instant) {
        let elapsed = measured.elapsed();
        match self {
            Self::Exact(samples) => samples.push(elapsed),
            Self::Bounded { bins, count, max } => {
                // Keep five significant bits of the nanosecond count.
                let nanos = elapsed.as_nanos().min(u64::MAX as u128) as u64;
                let shift = (u64::BITS - nanos.leading_zeros()).saturating_sub(5);
                bins[shift as usize * 16 + (nanos >> shift) as usize] += 1;
                *count += 1;
                *max = (*max).max(elapsed);
            }
        }
    }

    fn count(&self) -> usize {
        match self {
            Self::Exact(samples) => samples.len(),
            Self::Bounded { count, .. } => *count,
        }
    }

    fn summarize(self) -> Latency {
        let count = self.count();
        let (p95, p99, max) = match self {
            Self::Exact(mut samples) => {
                samples.sort_unstable();
                let at = |rank: usize| {
                    samples
                        .get(rank.saturating_sub(1))
                        .copied()
                        .unwrap_or_default()
                };
                (at(count - count / 20), at(count - count / 100), at(count))
            }
            Self::Bounded { bins, max, .. } => {
                let at = |mut rank: usize| {
                    for (index, &frequency) in bins.iter().enumerate() {
                        if rank <= frequency {
                            let shift = (index / 16).saturating_sub(1);
                            let upper =
                                (((index - shift * 16) as u64) << shift) | ((1_u64 << shift) - 1);
                            return Duration::from_nanos(upper).min(max);
                        }
                        rank -= frequency;
                    }
                    unreachable!("rank exceeds recorded samples")
                };
                (at(count - count / 20), at(count - count / 100), max)
            }
        };
        let ms = |sample: Duration| sample.as_secs_f64() * 1000.0;
        Latency {
            count,
            p95_ms: ms(p95),
            p99_ms: ms(p99),
            max_ms: ms(max),
        }
    }
}

struct Mixed {
    workload: DocumentInfo,
    reference: RunReport,
    active: Vec<AcceptedRun>,
    held: AcceptedRun,
    outcomes_checked: usize,
    saw_overlap: bool,
    queue_full: usize,
    pausing: Option<(AcceptedRun, Instant)>,
    pauses: Samples,
}

impl Mixed {
    fn start(
        workspace: &Workspace,
        workload: DocumentInfo,
        input: &str,
        cfg: WorkspaceConfig,
        churn: bool,
    ) -> Self {
        let reference = run_lab_with_limits(input, cfg.run_limits).unwrap();
        let RunReport::Scenario(report) = &reference else {
            unreachable!()
        };
        assert!(
            report
                .probes
                .iter()
                .all(|p| matches!(p.outcome, Outcome::Reachable { .. }))
        );
        let held = workspace.submit_run(workload.id, RunMode::Paused).unwrap();
        assert_eq!(
            await_state(workspace, held, |s| s == RunStatus::Paused),
            RunStatus::Paused
        );
        Self {
            workload,
            reference,
            active: Vec::new(),
            held,
            outcomes_checked: 0,
            saw_overlap: false,
            queue_full: 0,
            pausing: None,
            pauses: Samples::new(churn),
        }
    }

    fn sample(&mut self, workspace: &Workspace, refill: bool, allow_pause: bool) {
        let stats = workspace.stats();
        self.saw_overlap |= stats.compiling > 0 && stats.running > 0 && stats.paused > 0;
        let mut pending = Vec::with_capacity(self.active.len());
        for run in self.active.drain(..) {
            let summary = workspace.run_summary(run.id).unwrap();
            if self
                .pausing
                .is_some_and(|(candidate, _)| candidate.id == run.id)
            {
                if summary.status == RunStatus::Paused {
                    self.pauses.add(self.pausing.take().unwrap().1);
                    workspace.control_run(run.id, RunControl::Resume).unwrap();
                } else if summary.status.is_terminal() {
                    self.pausing = None;
                }
            } else if self.pausing.is_none() && summary.status == RunStatus::Running && allow_pause
            {
                let start = Instant::now();
                match workspace.control_run(run.id, RunControl::Pause) {
                    Ok(()) => self.pausing = Some((run, start)),
                    Err(WorkspaceError::InvalidControl { status, .. }) if status.is_terminal() => {}
                    Err(error) => panic!("pause failed: {error}"),
                }
            }
            if summary.status.is_terminal() {
                assert_eq!(summary.status, RunStatus::Finished, "{summary:?}");
                assert_eq!(*workspace.run_report(run.id).unwrap(), self.reference);
                self.outcomes_checked += 1;
            } else {
                pending.push(run);
            }
        }
        self.active = pending;
        if refill {
            while self.active.len() < 24 {
                match workspace.submit_run(self.workload.id, RunMode::Automatic) {
                    Ok(run) => self.active.push(run),
                    Err(WorkspaceError::QueueFull) => {
                        self.queue_full += 1;
                        break;
                    }
                    Err(error) => panic!("load submission failed: {error}"),
                }
            }
        }
    }

    fn finish(&mut self, workspace: &Workspace) {
        let end = Instant::now() + Duration::from_secs(30);
        while !self.active.is_empty() {
            assert!(Instant::now() < end, "mixed runs did not drain");
            self.sample(workspace, false, false);
            thread::sleep(Duration::from_millis(1));
        }
        workspace
            .control_run(self.held.id, RunControl::Cancel)
            .unwrap();
        assert_eq!(
            await_state(workspace, self.held, RunStatus::is_terminal),
            RunStatus::Cancelled
        );
        assert!(self.saw_overlap && self.queue_full > 0 && self.outcomes_checked > 2);
        assert!(self.pauses.count() > 0, "no in-progress pause was observed");
    }
}

fn await_state(
    workspace: &Workspace,
    run: AcceptedRun,
    wanted: impl Fn(RunStatus) -> bool,
) -> RunStatus {
    let end = Instant::now() + Duration::from_secs(15);
    let mut summary = workspace.run_summary(run.id).unwrap();
    while !wanted(summary.status) {
        assert!(
            !summary.status.is_terminal() && Instant::now() < end,
            "{summary:?}"
        );
        summary = workspace
            .wait_for_update(run.id, summary.update, Duration::from_millis(25))
            .unwrap();
    }
    summary.status
}

#[cfg(windows)]
fn sample_memory(phase: &str, start: Instant) -> io::Result<()> {
    use std::{ffi::c_void, mem::size_of};

    #[derive(Default)]
    #[repr(C)]
    struct MemoryCounters {
        size: u32,
        page_fault_count: u32,
        peak_working_set: usize,
        working_set: usize,
        pool_and_pagefile: [usize; 6],
        private_usage: usize,
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentProcess() -> *mut c_void;
        fn K32GetProcessMemoryInfo(
            process: *mut c_void,
            counters: *mut MemoryCounters,
            size: u32,
        ) -> i32;
    }

    let size = size_of::<MemoryCounters>() as u32;
    let mut counters = MemoryCounters {
        size,
        ..MemoryCounters::default()
    };
    // The current process pseudo-handle needs no close; the buffer matches PROCESS_MEMORY_COUNTERS_EX.
    if unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, size) } == 0 {
        return Err(io::Error::last_os_error());
    }
    eprintln!(
        "{}",
        json!({
            "event": "workspace_memory", "phase": phase, "elapsed_ms": start.elapsed().as_millis(),
            "working_set_bytes": counters.working_set,
            "peak_working_set_bytes": counters.peak_working_set,
            "private_bytes": counters.private_usage,
        })
    );
    Ok(())
}

#[cfg(not(windows))]
fn sample_memory(_phase: &str, _start: Instant) -> io::Result<()> {
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = env::args().skip(1).collect();
    let [kind, count, seconds] = args.as_slice() else {
        return Err(
            io::Error::other("用法  workspace-load <pure|mixed|churn> <48|64> <秒数>").into(),
        );
    };
    if !matches!(kind.as_str(), "pure" | "mixed" | "churn") {
        return Err(io::Error::other("负载须为 pure、mixed 或 churn。").into());
    }
    let churn = kind == "churn";
    let count: usize = count.parse()?;
    let seconds: u64 = seconds.parse()?;
    if !matches!(count, 48 | 64) || seconds == 0 {
        return Err(io::Error::other("文档数须为 48 或 64，秒数须大于零。").into());
    }
    let cfg = WorkspaceConfig {
        resident_runs: 3,
        history_runs: 40,
        ..WorkspaceConfig::default()
    };
    let workspace = Workspace::new(cfg)?;
    let opening = Instant::now();
    let mut documents = Vec::new();
    for index in 0..count {
        let large = index < 4;
        let devices = if large { 256 } else { 32 };
        let probes = if index == 0 { 96 } else { 1 };
        documents.push(workspace.open_document(topology(
            &format!("lab-{index}"),
            devices,
            probes,
        ))?);
    }
    let open_ms = opening.elapsed().as_secs_f64() * 1000.0;
    let mut mixed = (kind != "pure").then(|| {
        let doc = workspace.document(documents[0].id).unwrap();
        Mixed::start(&workspace, doc.info, &doc.experiment, cfg, churn)
    });
    let mut switch = Samples::new(churn);
    let mut layout = Samples::new(churn);
    let mut config = Samples::new(churn);
    let mut reopen = churn.then(|| Samples::new(true));
    let mut round = 0;
    let start = Instant::now();
    let mut last_memory_sample = start;
    if churn {
        sample_memory("start", start)?;
    }
    while start.elapsed() < Duration::from_secs(seconds) {
        for (index, info) in documents.iter_mut().enumerate() {
            let measured = Instant::now();
            let doc = workspace.document(info.id)?;
            switch.add(measured);
            if index % 4 == 1 {
                let measured = Instant::now();
                *info = workspace.replace_presentation(
                    info.id,
                    info.revision,
                    Some(format!(r#"{{"pc-0":{{"x":{round},"y":{index}}}}}"#)),
                )?;
                layout.add(measured);
                let measured = Instant::now();
                let (old, new) = if round % 2 == 0 {
                    (EXPECTED_PORT, EDITED_PORT)
                } else {
                    (EDITED_PORT, EXPECTED_PORT)
                };
                let updated = doc.experiment.replacen(old, new, 1);
                assert_ne!(updated, doc.experiment.as_ref());
                *info = workspace.replace_experiment(info.id, info.revision, updated)?;
                config.add(measured);
            }
            if let Some(reopen) = &mut reopen
                && matches!(index, 1 | 5)
            {
                // Recycle one large and one small edited document, without keeping old snapshots alive.
                drop(doc);
                let input = workspace.document(info.id)?.experiment.to_string();
                let measured = Instant::now();
                workspace.close_document_at(info.id, info.revision, true)?;
                *info = workspace.open_document(input)?;
                reopen.add(measured);
            }
            if let Some(mixed) = &mut mixed
                && index % 8 == 0
            {
                let memory_due =
                    churn && cfg!(windows) && last_memory_sample.elapsed() >= MEMORY_INTERVAL;
                mixed.sample(&workspace, true, !memory_due);
                if memory_due && mixed.pausing.is_none() {
                    sample_memory("active", start)?;
                    last_memory_sample = Instant::now();
                }
            }
        }
        round += 1;
        thread::sleep(Duration::from_millis(1));
    }
    if let Some(mixed) = &mut mixed {
        mixed.finish(&workspace);
    }
    let stats = workspace.stats();
    assert_eq!((stats.resident, stats.queued, stats.input_bytes), (0, 0, 0));
    assert!(
        stats.retained <= cfg.history_runs
            && stats.retained_trace_entries <= cfg.history_trace_entries
    );
    if mixed.is_none() {
        assert_eq!((stats.compiles_started, stats.peak_resident), (0, 0));
    }
    let (open_documents, document_bytes) = workspace.document_usage();
    assert_eq!(open_documents, count);
    if churn {
        sample_memory("drained", start)?;
    }
    let mut report = json!({
        "load": kind, "documents": count, "workers": cfg.workers, "active_seconds": seconds,
        "open_ms": open_ms, "document_bytes": document_bytes, "rounds": round,
        "switch": switch.summarize(), "layout": layout.summarize(), "config": config.summarize(),
        "mixed": mixed.map(|mixed| json!({
            "overlap_observed": mixed.saw_overlap, "queue_full": mixed.queue_full,
            "outcomes_checked": mixed.outcomes_checked, "pause": mixed.pauses.summarize(),
        })),
        "stats": stats,
    });
    if let Some(reopen) = reopen {
        assert_eq!(reopen.count(), 2 * round);
        report["churn"] = json!({
            "close_reopen": reopen.summarize(),
            "percentiles": "histogram_upper_bound",
        });
    }
    println!("{report}");
    Ok(())
}
