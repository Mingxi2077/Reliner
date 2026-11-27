use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Condvar, Mutex, OnceLock},
    thread::{self, JoinHandle},
    time::Duration,
};

use serde::Serialize;

use crate::{
    DocumentId, EventId, ProbeCounts, Revision, RunReport, StopReason, TraceReport, TraceStatus,
    WorkspaceConfig, WorkspaceError, report::decimal, sim::Simulation,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct RunId(#[serde(serialize_with = "decimal")] pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct AcceptedRun {
    pub id: RunId,
    pub document_id: DocumentId,
    pub effective_revision: Revision,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RunOrigin {
    pub document_id: DocumentId,
    pub document_revision: Revision,
    pub effective_revision: Revision,
    pub topology_id: String,
    pub native_revision: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunMode {
    Automatic,
    Paused,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunControl {
    Pause,
    Resume,
    Step,
    Cancel,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Queued,
    Compiling,
    Ready,
    Running,
    Paused,
    Finished,
    Incomplete,
    Cancelled,
    Failed,
}

impl RunStatus {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Finished | Self::Incomplete | Self::Cancelled | Self::Failed
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RunSummary {
    #[serde(flatten)]
    pub run: AcceptedRun,
    pub origin: RunOrigin,
    pub status: RunStatus,
    #[serde(serialize_with = "decimal")]
    pub update: u64,
    #[serde(serialize_with = "decimal")]
    pub last_event_ns: u64,
    pub last_event_id: Option<EventId>,
    pub stop_reason: Option<StopReason>,
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_error: Option<crate::InputError>,
    pub trace_status: Option<TraceStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub probe_counts: Option<ProbeCounts>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_limit: Option<usize>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct SchedulerStats {
    pub queued: usize,
    pub compiling: usize,
    pub compiling_peak: usize,
    #[serde(serialize_with = "decimal")]
    pub compiles_started: u64,
    pub ready: usize,
    pub running: usize,
    pub paused: usize,
    pub resident: usize,
    pub peak_resident: usize,
    pub retained: usize,
    pub retained_trace_entries: usize,
    pub input_bytes: usize,
    pub peak_input_bytes: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Intent {
    Run,
    Pause,
    Step,
    Cancel,
}

struct Job {
    summary: RunSummary,
    intent: Intent,
    input: Option<String>,
    simulation: Option<Box<Simulation>>,
    report: Option<Arc<RunReport>>,
    trace: Option<Arc<TraceReport>>,
    comparison_index: Arc<OnceLock<Vec<usize>>>,
}

pub(crate) struct RunRead {
    pub(crate) id: RunId,
    pub(crate) origin: RunOrigin,
    pub(crate) report: Option<Arc<RunReport>>,
    pub(crate) trace: Option<Arc<TraceReport>>,
    pub(crate) trace_status: Option<TraceStatus>,
}

impl Job {
    fn status(&mut self, status: RunStatus) {
        self.summary.status = status;
        self.summary.update += 1;
    }

    fn progress(&mut self, simulation: &Simulation) {
        if let Some((at_ns, id)) = simulation.last_event() {
            self.summary.last_event_ns = at_ns;
            self.summary.last_event_id = Some(id);
        }
    }
}

#[derive(Default)]
struct State {
    next_id: u64,
    jobs: HashMap<RunId, Job>,
    queue: VecDeque<RunId>,
    ready: VecDeque<RunId>,
    history: VecDeque<RunId>,
    prefer_ready: bool,
    closing: bool,
    input_bytes: usize,
    peak_input_bytes: usize,
    resident: usize,
    peak_resident: usize,
    compiling: usize,
    compiling_peak: usize,
    compiles_started: u64,
    trace_entries: usize,
}

impl State {
    fn job(&self, id: RunId) -> Result<&Job, WorkspaceError> {
        self.jobs.get(&id).ok_or(WorkspaceError::UnknownRun(id))
    }

    fn job_mut(&mut self, id: RunId) -> &mut Job {
        self.jobs
            .get_mut(&id)
            .expect("live run belongs to the scheduler")
    }

    fn prune(&mut self, config: WorkspaceConfig) {
        while self.history.len() > config.history_runs {
            let id = self.history.pop_front().expect("retained summary");
            let job = self.jobs.remove(&id).expect("retained run");
            self.trace_entries -= job.trace.map_or(0, |trace| trace.entries.len());
        }
        for &id in &self.history {
            if self.trace_entries <= config.history_trace_entries {
                break;
            }
            let job = self.jobs.get_mut(&id).expect("retained run");
            if let Some(trace) = job.trace.take() {
                self.trace_entries -= trace.entries.len();
                job.summary.trace_status = Some(TraceStatus::Evicted);
                job.summary.update += 1;
            }
        }
    }

    fn next_task(&mut self, config: WorkspaceConfig) -> Option<Task> {
        let can_compile = !self.queue.is_empty() && self.resident < config.resident_runs;
        if !self.ready.is_empty() && (self.prefer_ready || !can_compile) {
            self.prefer_ready = false;
            let id = self.ready.pop_front().expect("ready run");
            let job = self.job_mut(id);
            job.status(RunStatus::Running);
            let simulation = job.simulation.take().expect("ready simulation");
            Some(Task {
                id,
                work: TaskWork::Run(simulation),
            })
        } else if can_compile {
            self.prefer_ready = true;
            let id = self.queue.pop_front().expect("queued run");
            self.resident += 1;
            self.peak_resident = self.peak_resident.max(self.resident);
            self.compiling += 1;
            self.compiling_peak = self.compiling_peak.max(self.compiling);
            self.compiles_started += 1;
            let job = self.job_mut(id);
            job.status(RunStatus::Compiling);
            Some(Task {
                id,
                work: TaskWork::Compile(job.input.take().expect("queued input")),
            })
        } else {
            None
        }
    }
}

struct Task {
    id: RunId,
    work: TaskWork,
}

enum TaskWork {
    Compile(String),
    Run(Box<Simulation>),
}

struct Shared {
    config: WorkspaceConfig,
    state: Mutex<State>,
    wake: Condvar,
}

pub(crate) struct Scheduler {
    shared: Arc<Shared>,
    threads: Vec<JoinHandle<()>>,
}

impl Scheduler {
    pub(crate) fn new(config: WorkspaceConfig) -> Result<Self, WorkspaceError> {
        let shared = Arc::new(Shared {
            config,
            state: Mutex::new(State::default()),
            wake: Condvar::new(),
        });
        let mut scheduler = Self {
            shared,
            threads: Vec::with_capacity(config.workers),
        };
        for index in 0..config.workers {
            let pool = Arc::clone(&scheduler.shared);
            let thread = thread::Builder::new()
                .name(format!("reliner-{index}"))
                .spawn(move || worker(&pool))?;
            scheduler.threads.push(thread);
        }
        Ok(scheduler)
    }

    pub(crate) fn submit(
        &self,
        origin: RunOrigin,
        input: &str,
        mode: RunMode,
    ) -> Result<AcceptedRun, WorkspaceError> {
        let mut state = self.shared.state.lock().unwrap();
        if state.queue.len() >= self.shared.config.queue_slots {
            return Err(WorkspaceError::QueueFull);
        }
        if input.len() > self.shared.config.input_bytes - state.input_bytes {
            return Err(WorkspaceError::InputBudget);
        }
        state.input_bytes += input.len();
        state.peak_input_bytes = state.peak_input_bytes.max(state.input_bytes);
        state.next_id += 1;
        let id = RunId(state.next_id);
        let accepted = AcceptedRun {
            id,
            document_id: origin.document_id,
            effective_revision: origin.effective_revision,
        };
        state.jobs.insert(
            id,
            Job {
                summary: RunSummary {
                    run: accepted,
                    origin,
                    status: RunStatus::Queued,
                    update: 0,
                    last_event_ns: 0,
                    last_event_id: None,
                    stop_reason: None,
                    error: None,
                    input_error: None,
                    trace_status: None,
                    probe_counts: None,
                    stop_limit: None,
                },
                intent: match mode {
                    RunMode::Automatic => Intent::Run,
                    RunMode::Paused => Intent::Pause,
                },
                input: Some(input.into()),
                simulation: None,
                report: None,
                trace: None,
                comparison_index: Arc::new(OnceLock::new()),
            },
        );
        state.queue.push_back(id);
        self.shared.wake.notify_all();
        Ok(accepted)
    }

    pub(crate) fn summary(&self, id: RunId) -> Result<RunSummary, WorkspaceError> {
        Ok(self.shared.state.lock().unwrap().job(id)?.summary.clone())
    }

    pub(crate) fn wait(
        &self,
        id: RunId,
        after: u64,
        timeout: Duration,
    ) -> Result<RunSummary, WorkspaceError> {
        let state = self.shared.state.lock().unwrap();
        let (state, _) = self
            .shared
            .wake
            .wait_timeout_while(state, timeout, |state| {
                state.jobs.get(&id).is_some_and(|job| {
                    !job.summary.status.is_terminal() && job.summary.update <= after
                })
            })
            .unwrap();
        Ok(state.job(id)?.summary.clone())
    }

    pub(crate) fn report(&self, id: RunId) -> Result<Arc<RunReport>, WorkspaceError> {
        let state = self.shared.state.lock().unwrap();
        state
            .job(id)?
            .report
            .clone()
            .ok_or(WorkspaceError::ReportUnavailable(id))
    }

    pub(crate) fn reports(
        &self,
        ids: [RunId; 2],
    ) -> Result<[(AcceptedRun, Arc<RunReport>); 2], WorkspaceError> {
        let state = self.shared.state.lock().unwrap();
        let [baseline, candidate] = ids.map(|id| {
            let job = state.job(id)?;
            let report = job
                .report
                .clone()
                .ok_or(WorkspaceError::ReportUnavailable(id))?;
            Ok::<_, WorkspaceError>((job.summary.run, report))
        });
        Ok([baseline?, candidate?])
    }

    pub(crate) fn comparison_snapshots(
        &self,
        ids: [RunId; 2],
    ) -> Result<[crate::compare::ComparisonSnapshot; 2], WorkspaceError> {
        let state = self.shared.state.lock().unwrap();
        let [baseline, candidate] = ids;
        let jobs = [state.job(baseline)?, state.job(candidate)?];
        let [before, after] = jobs.map(|job| {
            Ok::<_, WorkspaceError>(crate::compare::ComparisonSnapshot {
                run: job.summary.run,
                origin: job.summary.origin.clone(),
                report: job
                    .report
                    .clone()
                    .ok_or(WorkspaceError::ReportUnavailable(job.summary.run.id))?,
                index: Arc::clone(&job.comparison_index),
            })
        });
        Ok([before?, after?])
    }

    pub(crate) fn read(&self, id: RunId) -> Result<RunRead, WorkspaceError> {
        let state = self.shared.state.lock().unwrap();
        let job = state.job(id)?;
        Ok(RunRead {
            id,
            origin: job.summary.origin.clone(),
            report: job.report.clone(),
            trace: job.trace.clone(),
            trace_status: job.summary.trace_status,
        })
    }

    pub(crate) fn trace(
        &self,
        id: RunId,
    ) -> Result<(Option<TraceStatus>, Option<Arc<TraceReport>>), WorkspaceError> {
        let state = self.shared.state.lock().unwrap();
        let job = state.job(id)?;
        Ok((job.summary.trace_status, job.trace.clone()))
    }

    pub(crate) fn control(&self, id: RunId, control: RunControl) -> Result<(), WorkspaceError> {
        let mut state = self.shared.state.lock().unwrap();
        let job = state.job(id)?;
        let status = job.summary.status;
        if status.is_terminal()
            || (control == RunControl::Step && status != RunStatus::Paused)
            || job.intent == Intent::Cancel
        {
            return Err(WorkspaceError::InvalidControl { status, control });
        }
        if control == RunControl::Cancel && status == RunStatus::Queued {
            let job = state.job_mut(id);
            let input = job.input.take().expect("queued input");
            job.summary.stop_reason = Some(StopReason::Cancelled);
            job.status(RunStatus::Cancelled);
            state.input_bytes -= input.len();
            state.queue.retain(|&queued| queued != id);
            state.history.push_back(id);
            state.prune(self.shared.config);
        } else {
            let job = state.job_mut(id);
            job.intent = match control {
                RunControl::Pause => Intent::Pause,
                RunControl::Resume => Intent::Run,
                RunControl::Step => Intent::Step,
                RunControl::Cancel => Intent::Cancel,
            };
            match (status, control) {
                (RunStatus::Ready, RunControl::Pause) => {
                    job.status(RunStatus::Paused);
                    state.ready.retain(|&queued| queued != id);
                }
                (RunStatus::Paused, RunControl::Resume | RunControl::Step | RunControl::Cancel) => {
                    job.status(RunStatus::Ready);
                    state.ready.push_back(id);
                }
                _ => {}
            }
        }
        self.shared.wake.notify_all();
        Ok(())
    }

    pub(crate) fn stats(&self) -> SchedulerStats {
        let state = self.shared.state.lock().unwrap();
        let count = |status| {
            state
                .jobs
                .values()
                .filter(|job| job.summary.status == status)
                .count()
        };
        SchedulerStats {
            queued: state.queue.len(),
            compiling: state.compiling,
            compiling_peak: state.compiling_peak,
            compiles_started: state.compiles_started,
            ready: state.ready.len(),
            running: count(RunStatus::Running),
            paused: count(RunStatus::Paused),
            resident: state.resident,
            peak_resident: state.peak_resident,
            retained: state.history.len(),
            retained_trace_entries: state.trace_entries,
            input_bytes: state.input_bytes,
            peak_input_bytes: state.peak_input_bytes,
        }
    }
}

impl Drop for Scheduler {
    fn drop(&mut self) {
        self.shared.state.lock().unwrap().closing = true;
        self.shared.wake.notify_all();
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

fn worker(shared: &Shared) {
    loop {
        let task = {
            let mut state = shared.state.lock().unwrap();
            loop {
                if state.closing {
                    return;
                }
                if let Some(task) = state.next_task(shared.config) {
                    shared.wake.notify_all();
                    break task;
                }
                state = shared.wake.wait(state).unwrap();
            }
        };
        match task.work {
            TaskWork::Compile(input) => compile(shared, task.id, input),
            TaskWork::Run(simulation) => run_batch(shared, task.id, simulation),
        }
    }
}

fn compile(shared: &Shared, id: RunId, input: String) {
    let result = crate::validation::compile_for_workspace(&input)
        .map(|topology| Box::new(Simulation::new(topology, shared.config.run_limits)));
    let bytes = input.len();
    drop(input);
    let mut state = shared.state.lock().unwrap();
    state.input_bytes -= bytes;
    state.compiling -= 1;
    let job = state.job_mut(id);
    match result {
        Ok(simulation) if simulation.next_work().is_none() => {
            drop(state);
            finish_run(shared, id, simulation);
            return;
        }
        Ok(simulation) => {
            job.simulation = Some(simulation);
            if job.intent == Intent::Pause {
                job.status(RunStatus::Paused);
            } else {
                job.status(RunStatus::Ready);
                state.ready.push_back(id);
            }
        }
        Err(error) => {
            job.summary.error = Some(error.to_string());
            job.summary.input_error = Some(error);
            job.status(RunStatus::Failed);
            state.resident -= 1;
            state.history.push_back(id);
            state.prune(shared.config);
        }
    }
    shared.wake.notify_all();
}

fn run_batch(shared: &Shared, id: RunId, mut simulation: Box<Simulation>) {
    let mut work = 0;
    while let Some(cost) = simulation.next_work() {
        {
            let mut state = shared.state.lock().unwrap();
            if state.closing {
                return;
            }
            let job = state.job_mut(id);
            job.progress(&simulation);
            match job.intent {
                Intent::Cancel => {
                    simulation.cancel();
                    break;
                }
                Intent::Pause => {
                    job.simulation = Some(simulation);
                    job.status(RunStatus::Paused);
                    shared.wake.notify_all();
                    return;
                }
                Intent::Step => job.intent = Intent::Pause,
                Intent::Run if work > 0 && cost > shared.config.batch_work.saturating_sub(work) => {
                    job.simulation = Some(simulation);
                    job.status(RunStatus::Ready);
                    state.ready.push_back(id);
                    shared.wake.notify_all();
                    return;
                }
                Intent::Run => {}
            }
        }
        simulation.step();
        work += cost;
    }
    finish_run(shared, id, simulation);
}

fn finish_run(shared: &Shared, id: RunId, simulation: Box<Simulation>) {
    let last = simulation.last_event();
    let mut report = simulation.report();
    let (stop, trace_status, trace) = match &mut report {
        RunReport::Legacy(report) => match report.outcome {
            crate::Outcome::Incomplete { reason, .. } => (Some(reason), None, None),
            _ => (None, None, None),
        },
        RunReport::Scenario(report) => {
            let status = report.trace.status;
            let entries = std::mem::take(&mut report.trace.entries);
            let trace = (!entries.is_empty()).then_some(TraceReport { status, entries });
            (report.stop_reason, Some(status), trace)
        }
    };
    let probe_counts = report.probe_counts();
    let mut state = shared.state.lock().unwrap();
    state.resident -= 1;
    state.trace_entries += trace.as_ref().map_or(0, |trace| trace.entries.len());
    let job = state.job_mut(id);
    if let Some((at_ns, event)) = last {
        job.summary.last_event_id = Some(event);
        job.summary.last_event_ns = at_ns;
    }
    job.summary.stop_reason = stop;
    job.summary.trace_status = trace_status;
    job.summary.probe_counts = Some(probe_counts);
    if let Some(StopReason::ResourceLimit { limit }) = stop {
        job.summary.stop_limit = Some(shared.config.run_limits.maximum(limit));
    }
    job.trace = trace.map(Arc::new);
    job.report = Some(Arc::new(report));
    job.status(match stop {
        None => RunStatus::Finished,
        Some(StopReason::Cancelled) => RunStatus::Cancelled,
        Some(_) => RunStatus::Incomplete,
    });
    state.history.push_back(id);
    state.prune(shared.config);
    shared.wake.notify_all();
}
