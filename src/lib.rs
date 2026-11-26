mod archive;
pub mod bridge;
mod compare;
mod dynamic;
mod effective;
mod gns3;
mod import_archive;
mod import_source;
mod importer;
mod ios;
mod ios_device;
pub mod lldp;
pub mod real_stack;
mod report;
mod scheduler;
mod sim;
pub mod stack_compare;
mod stp;
mod topology;
mod validation;
mod workspace;

pub use archive::{
    ArchiveError, ArchiveSource, ArchiveSummary, ArchivedRun, MAX_ARCHIVE_BYTES, read_archive,
    write_archive,
};
pub use compare::{ComparedProbe, ComparedRun, ProbeComparison, RunComparison, RunComparisonPage};
pub use effective::{EffectiveSetting, SettingSource, effective_settings};
pub use gns3::{import_gns3, preview_gns3};
pub use import_source::{
    ComponentId, CoverageComponent, CoverageRef, ImportGap, ImportGapSummary, ImportScope,
    ImportSource, ReportCoverage,
};
pub use importer::{
    ImportError, ImportOptions, ImportPreview, import_containerlab, preview_containerlab,
};
pub use ios::{IosGap, IosInterface, preview_ios};
pub use ios_device::{IosDevice, IosDevicePort, IosGlobal, VlanDefinition, preview_ios_device};
pub use report::*;
pub use scheduler::{
    AcceptedRun, RunControl, RunId, RunMode, RunOrigin, RunStatus, RunSummary, SchedulerStats,
};
pub use stack_compare::compare_real_stack;
pub use validation::{
    InputError, InputLocation, NativeFormat, NativePreview, ValidationResult, ValidationSummary,
    preview_native, validate_lab,
};
pub use workspace::*;

pub const BACKEND_API_VERSION: u32 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct RunLimits {
    pub event_work: usize,
    pub total_work: usize,
    pub pending_events: usize,
    pub state_entries: usize,
    pub essential_facts: usize,
    pub trace_entries: usize,
}

impl RunLimits {
    pub fn maximum(self, limit: ResourceLimit) -> usize {
        match limit {
            ResourceLimit::EventWork => self.event_work,
            ResourceLimit::TotalWork => self.total_work,
            ResourceLimit::PendingEvents => self.pending_events,
            ResourceLimit::StateEntries => self.state_entries,
            ResourceLimit::EssentialFacts => self.essential_facts,
        }
    }
}

impl Default for RunLimits {
    fn default() -> Self {
        Self {
            event_work: 8_192,
            total_work: 2_000_000,
            pending_events: 32_768,
            state_entries: 16_384,
            essential_facts: 4_096,
            trace_entries: 4_096,
        }
    }
}

pub fn run_experiment(input: &str) -> Result<Report, InputError> {
    let topology = topology::Topology::from_json(input, true)?;
    let RunReport::Legacy(report) = sim::run(topology, RunLimits::default()) else {
        unreachable!("compiled as legacy")
    };
    Ok(report)
}

pub fn run_lab(input: &str) -> Result<RunReport, InputError> {
    run_lab_with_limits(input, RunLimits::default())
}

pub fn run_lab_with_limits(input: &str, limits: RunLimits) -> Result<RunReport, InputError> {
    let topology = topology::Topology::from_json(input, false)?;
    Ok(sim::run(topology, limits))
}
