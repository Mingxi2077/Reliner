//! Protocol 1 for a persistent, locally owned Workspace session.
//!
//! JSONL drivers pass one line to `Backend::handle_json` and write the returned
//! line followed by a newline. A driver must keep the same Backend between lines.

use std::{
    collections::hash_map::RandomState,
    fs::{self, File},
    hash::BuildHasher,
    io::{self, BufWriter, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::SystemTime,
};

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use serde_json::{Value, json, value::RawValue};

use crate::{
    AcceptedRun, ArchiveError, ComparedRun, CoverageRef, DocumentId, DocumentInfo, EventId,
    EvidenceId, IcmpIdentity, ImportError, ImportOptions, Outcome, Page, Probe, ProbeComparison,
    ProbeReport, Revision, RunControl, RunId, RunMode, RunReport, RunSummary, Workspace,
    WorkspaceConfig, WorkspaceError, import_containerlab, import_gns3, preview_containerlab,
    preview_gns3,
};

pub const PROTOCOL_VERSION: u32 = 1;
pub const MAX_PAGE_SIZE: usize = 128;

const COMMAND_NAMES: &[&str] = &[
    "hello",
    "import.preview",
    "import.ios.preview",
    "import.ios.device.preview",
    "lldp.check",
    "import.open",
    "import.gns3.preview",
    "import.gns3.open",
    "document.open",
    "document.get",
    "document.info",
    "document.replace_experiment",
    "document.replace_presentation",
    "document.validate",
    "document.effective_settings",
    "document.preview_file",
    "document.save",
    "document.save_and_rerun",
    "document.close",
    "run.submit",
    "run.summary",
    "run.control",
    "run.probes",
    "run.evidence",
    "run.links",
    "run.routes",
    "run.stp",
    "run.stp_bridges",
    "run.stp_links",
    "run.compare",
    "run.import_gaps",
    "run.import_gap",
    "run.trace",
    "run.archive_to",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DecimalU64(pub u64);

impl Serialize for DecimalU64 {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for DecimalU64 {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(de::Error::custom("ID 和修订须为十进制整数字符串。"));
        }
        text.parse().map(Self).map_err(de::Error::custom)
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub protocol_version: u32,
    pub request_id: DecimalU64,
    pub session_id: Option<String>,
    pub command: Command,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunModeDto {
    #[default]
    Automatic,
    Paused,
}

impl From<RunModeDto> for RunMode {
    fn from(mode: RunModeDto) -> Self {
        match mode {
            RunModeDto::Automatic => Self::Automatic,
            RunModeDto::Paused => Self::Paused,
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunControlDto {
    Pause,
    Resume,
    Step,
    Cancel,
}

impl From<RunControlDto> for RunControl {
    fn from(control: RunControlDto) -> Self {
        match control {
            RunControlDto::Pause => Self::Pause,
            RunControlDto::Resume => Self::Resume,
            RunControlDto::Step => Self::Step,
            RunControlDto::Cancel => Self::Cancel,
        }
    }
}

impl From<RunControl> for RunControlDto {
    fn from(control: RunControl) -> Self {
        match control {
            RunControl::Pause => Self::Pause,
            RunControl::Resume => Self::Resume,
            RunControl::Step => Self::Step,
            RunControl::Cancel => Self::Cancel,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", deny_unknown_fields)]
pub enum Command {
    #[serde(rename = "hello")]
    Hello,
    #[serde(rename = "import.preview")]
    ImportPreview { yaml: String },
    #[serde(rename = "import.ios.preview")]
    IosPreview { config: String },
    #[serde(rename = "import.ios.device.preview")]
    IosDevicePreview { config: String },
    #[serde(rename = "lldp.check")]
    LldpCheck {
        path: PathBuf,
        expectation: crate::lldp::LldpExpectation,
    },
    #[serde(rename = "import.open")]
    ImportOpen {
        yaml: String,
        options: ImportOptions,
    },
    #[serde(rename = "import.gns3.preview")]
    Gns3Preview { project: String },
    #[serde(rename = "import.gns3.open")]
    Gns3Open {
        project: String,
        options: ImportOptions,
    },
    #[serde(rename = "document.open")]
    DocumentOpen {
        input: Option<String>,
        path: Option<PathBuf>,
    },
    #[serde(rename = "document.get")]
    DocumentGet { document_id: DecimalU64 },
    #[serde(rename = "document.info")]
    DocumentInfo { document_id: DecimalU64 },
    #[serde(rename = "document.replace_experiment")]
    DocumentReplaceExperiment {
        document_id: DecimalU64,
        expected_revision: DecimalU64,
        input: String,
    },
    #[serde(rename = "document.replace_presentation")]
    DocumentReplacePresentation {
        document_id: DecimalU64,
        expected_revision: DecimalU64,
        presentation: Option<String>,
    },
    #[serde(rename = "document.validate")]
    DocumentValidate {
        document_id: DecimalU64,
        expected_revision: DecimalU64,
    },
    #[serde(rename = "document.effective_settings")]
    DocumentEffectiveSettings {
        document_id: DecimalU64,
        expected_revision: DecimalU64,
        offset: usize,
        count: usize,
    },
    #[serde(rename = "document.preview_file")]
    DocumentPreviewFile { path: PathBuf },
    #[serde(rename = "document.save")]
    DocumentSave {
        document_id: DecimalU64,
        expected_revision: DecimalU64,
        path: PathBuf,
    },
    #[serde(rename = "document.save_and_rerun")]
    DocumentSaveAndRerun {
        document_id: DecimalU64,
        expected_revision: DecimalU64,
        path: PathBuf,
        #[serde(default)]
        mode: RunModeDto,
    },
    #[serde(rename = "document.close")]
    DocumentClose {
        document_id: DecimalU64,
        expected_revision: DecimalU64,
        #[serde(default)]
        discard_unsaved: bool,
    },
    #[serde(rename = "run.submit")]
    RunSubmit {
        document_id: DecimalU64,
        expected_revision: DecimalU64,
        #[serde(default)]
        mode: RunModeDto,
    },
    #[serde(rename = "run.summary")]
    RunSummary { run_id: DecimalU64 },
    #[serde(rename = "run.control")]
    RunControl {
        run_id: DecimalU64,
        control: RunControlDto,
    },
    #[serde(rename = "run.probes")]
    RunProbes {
        run_id: DecimalU64,
        offset: usize,
        count: usize,
    },
    #[serde(rename = "run.evidence")]
    RunEvidence {
        run_id: DecimalU64,
        offset: usize,
        count: usize,
    },
    #[serde(rename = "run.links")]
    RunLinks {
        run_id: DecimalU64,
        offset: usize,
        count: usize,
    },
    #[serde(rename = "run.routes")]
    RunRoutes {
        run_id: DecimalU64,
        offset: usize,
        count: usize,
    },
    #[serde(rename = "run.stp")]
    RunStp {
        run_id: DecimalU64,
        offset: usize,
        count: usize,
    },
    #[serde(rename = "run.stp_bridges")]
    RunStpBridges {
        run_id: DecimalU64,
        event_id: DecimalU64,
        offset: usize,
        count: usize,
    },
    #[serde(rename = "run.stp_links")]
    RunStpLinks {
        run_id: DecimalU64,
        event_id: DecimalU64,
        offset: usize,
        count: usize,
    },
    #[serde(rename = "run.compare")]
    RunCompare {
        baseline: DecimalU64,
        candidate: DecimalU64,
        offset: usize,
        count: usize,
    },
    #[serde(rename = "run.import_gaps")]
    RunImportGaps {
        run_id: DecimalU64,
        reference: Option<CoverageRef>,
        offset: usize,
        count: usize,
    },
    #[serde(rename = "run.import_gap")]
    RunImportGap { run_id: DecimalU64, gap_id: String },
    #[serde(rename = "run.trace")]
    RunTrace {
        run_id: DecimalU64,
        offset: usize,
        count: usize,
    },
    #[serde(rename = "run.archive_to")]
    RunArchiveTo { run_id: DecimalU64, path: PathBuf },
}

impl Command {
    fn page_size(&self) -> Option<usize> {
        match self {
            Self::DocumentEffectiveSettings { count, .. }
            | Self::RunProbes { count, .. }
            | Self::RunEvidence { count, .. }
            | Self::RunLinks { count, .. }
            | Self::RunRoutes { count, .. }
            | Self::RunStp { count, .. }
            | Self::RunStpBridges { count, .. }
            | Self::RunStpLinks { count, .. }
            | Self::RunCompare { count, .. }
            | Self::RunImportGaps { count, .. }
            | Self::RunTrace { count, .. } => Some(*count),
            _ => None,
        }
    }

    fn expected_revision(&self) -> Option<DecimalU64> {
        match self {
            Self::DocumentReplaceExperiment {
                expected_revision, ..
            }
            | Self::DocumentReplacePresentation {
                expected_revision, ..
            }
            | Self::DocumentValidate {
                expected_revision, ..
            }
            | Self::DocumentEffectiveSettings {
                expected_revision, ..
            }
            | Self::DocumentSave {
                expected_revision, ..
            }
            | Self::DocumentSaveAndRerun {
                expected_revision, ..
            }
            | Self::DocumentClose {
                expected_revision, ..
            }
            | Self::RunSubmit {
                expected_revision, ..
            } => Some(*expected_revision),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    InvalidRequest,
    UnsupportedProtocol,
    HandshakeRequired,
    SessionMismatch,
    InvalidPage,
    InvalidConfig,
    DocumentTooLarge,
    DocumentBudget,
    DocumentSlots,
    UnknownDocument,
    StaleRevision,
    UnsavedDocument,
    InvalidDocument,
    PathInUse,
    SaveConflict,
    QueueFull,
    InputBudget,
    UnknownRun,
    UnknownStpEvent,
    UnknownImportGap,
    InvalidCoverageReference,
    ReportUnavailable,
    InvalidControl,
    IoError,
    JsonError,
    InputError,
    ImportError,
    LldpError,
    UnsupportedArchiveVersion,
    UnsupportedReportVersion,
    ArchiveTooLarge,
    InvalidArchive,
    SerializationError,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BridgeError {
    pub code: ErrorCode,
    pub message: String,
    pub details: Value,
}

impl BridgeError {
    fn new(code: ErrorCode, message: impl Into<String>, details: Value) -> Self {
        Self {
            code,
            message: message.into(),
            details,
        }
    }

    fn io(error: io::Error) -> Self {
        let kind = match error.kind() {
            io::ErrorKind::NotFound => "not_found",
            io::ErrorKind::PermissionDenied => "permission_denied",
            io::ErrorKind::AlreadyExists => "already_exists",
            io::ErrorKind::InvalidInput => "invalid_input",
            io::ErrorKind::InvalidData => "invalid_data",
            io::ErrorKind::WriteZero => "write_zero",
            io::ErrorKind::Interrupted => "interrupted",
            io::ErrorKind::UnexpectedEof => "unexpected_eof",
            io::ErrorKind::Unsupported => "unsupported",
            io::ErrorKind::OutOfMemory => "out_of_memory",
            io::ErrorKind::TimedOut => "timed_out",
            io::ErrorKind::BrokenPipe => "broken_pipe",
            _ => "other",
        };
        Self::new(
            ErrorCode::IoError,
            error.to_string(),
            json!({
                "io_kind": kind,
                "io_kind_debug": format!("{:?}", error.kind()),
                "os_code": error.raw_os_error(),
            }),
        )
    }

    fn json(error: serde_json::Error, code: ErrorCode) -> Self {
        Self::new(
            code,
            error.to_string(),
            json!({
                "line": error.line(), "column": error.column(),
            }),
        )
    }
}

impl From<WorkspaceError> for BridgeError {
    fn from(error: WorkspaceError) -> Self {
        let message = error.to_string();
        let (code, details) = match error {
            WorkspaceError::InvalidConfig => (ErrorCode::InvalidConfig, json!({})),
            WorkspaceError::DocumentTooLarge => (ErrorCode::DocumentTooLarge, json!({})),
            WorkspaceError::DocumentBudget => (ErrorCode::DocumentBudget, json!({})),
            WorkspaceError::DocumentSlots => (ErrorCode::DocumentSlots, json!({})),
            WorkspaceError::UnknownDocument(id) => (
                ErrorCode::UnknownDocument,
                json!({"document_id": DecimalU64(id.0)}),
            ),
            WorkspaceError::StaleEdit { actual } => (
                ErrorCode::StaleRevision,
                json!({"actual": DecimalU64(actual.0)}),
            ),
            WorkspaceError::UnsavedDocument(id) => (
                ErrorCode::UnsavedDocument,
                json!({"document_id": DecimalU64(id.0)}),
            ),
            WorkspaceError::InvalidDocument(_) => (ErrorCode::InvalidDocument, json!({})),
            WorkspaceError::PathInUse(path) => (ErrorCode::PathInUse, json!({"path": path})),
            WorkspaceError::SaveConflict(path) => (ErrorCode::SaveConflict, json!({"path": path})),
            WorkspaceError::QueueFull => (ErrorCode::QueueFull, json!({})),
            WorkspaceError::InputBudget => (ErrorCode::InputBudget, json!({})),
            WorkspaceError::UnknownRun(id) => {
                (ErrorCode::UnknownRun, json!({"run_id": DecimalU64(id.0)}))
            }
            WorkspaceError::InvalidCoverageReference => {
                (ErrorCode::InvalidCoverageReference, json!({}))
            }
            WorkspaceError::ReportUnavailable(id) => (
                ErrorCode::ReportUnavailable,
                json!({"run_id": DecimalU64(id.0)}),
            ),
            WorkspaceError::InvalidControl { status, control } => (
                ErrorCode::InvalidControl,
                json!({"status": status, "control": RunControlDto::from(control)}),
            ),
            WorkspaceError::Io(error) => return Self::io(error),
            WorkspaceError::Json(error) => return Self::json(error, ErrorCode::JsonError),
            WorkspaceError::Input(error) => (ErrorCode::InputError, json!({"input_error": error})),
            WorkspaceError::Archive(error) => return error.into(),
        };
        Self::new(code, message, details)
    }
}

impl From<ArchiveError> for BridgeError {
    fn from(error: ArchiveError) -> Self {
        let message = error.to_string();
        let (code, details) = match error {
            ArchiveError::Io(error) => return Self::io(error),
            ArchiveError::Json(error) => return Self::json(error, ErrorCode::JsonError),
            ArchiveError::UnsupportedVersion(version) => (
                ErrorCode::UnsupportedArchiveVersion,
                json!({"archive_version": version}),
            ),
            ArchiveError::UnsupportedReportVersion(version) => (
                ErrorCode::UnsupportedReportVersion,
                json!({"report_version": version}),
            ),
            ArchiveError::TooLarge => (ErrorCode::ArchiveTooLarge, json!({})),
            ArchiveError::InvalidReport(_) => (ErrorCode::InvalidArchive, json!({})),
        };
        Self::new(code, message, details)
    }
}

impl From<ImportError> for BridgeError {
    fn from(error: ImportError) -> Self {
        Self::new(
            ErrorCode::ImportError,
            error.to_string(),
            json!({"import_error": error}),
        )
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Response {
    pub protocol_version: u32,
    pub request_id: Option<DecimalU64>,
    pub session_id: String,
    #[serde(flatten)]
    pub body: ResponseBody,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ResponseBody {
    Ok { data: Value },
    Error { error: BridgeError },
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RunOrigin {
    pub run_id: DecimalU64,
    pub document_id: DecimalU64,
    pub document_revision: DecimalU64,
    pub effective_revision: DecimalU64,
    pub topology_id: String,
    pub native_revision: String,
}

impl RunOrigin {
    fn from_summary(summary: &RunSummary) -> Self {
        Self::from_origin(summary.run.id, &summary.origin)
    }

    fn from_origin(id: RunId, origin: &crate::RunOrigin) -> Self {
        Self {
            run_id: DecimalU64(id.0),
            document_id: DecimalU64(origin.document_id.0),
            document_revision: DecimalU64(origin.document_revision.0),
            effective_revision: DecimalU64(origin.effective_revision.0),
            topology_id: origin.topology_id.clone(),
            native_revision: origin.native_revision.clone(),
        }
    }

    fn from_document(run: AcceptedRun, document: DocumentInfo) -> Self {
        Self {
            run_id: DecimalU64(run.id.0),
            document_id: DecimalU64(run.document_id.0),
            document_revision: DecimalU64(document.revision.0),
            effective_revision: DecimalU64(run.effective_revision.0),
            topology_id: document.topology_id,
            native_revision: document.native_revision,
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRequest {
    protocol_version: u32,
    request_id: DecimalU64,
    session_id: Option<String>,
    command: Box<RawValue>,
}

#[derive(Serialize)]
struct DocumentData<'a> {
    info: DocumentInfo,
    experiment: &'a str,
    presentation: Option<&'a str>,
}

#[derive(Serialize)]
struct SubmittedRun {
    run: AcceptedRun,
    origin: RunOrigin,
}

#[derive(Serialize)]
struct SummaryData {
    origin: RunOrigin,
    summary: RunSummary,
}

#[derive(Serialize)]
struct RunPage<T> {
    origin: RunOrigin,
    #[serde(flatten)]
    page: T,
}

#[derive(Serialize)]
struct EventPage<T> {
    origin: RunOrigin,
    event_id: DecimalU64,
    #[serde(flatten)]
    page: T,
}

#[derive(Serialize)]
struct CoveragePage<T> {
    origin: RunOrigin,
    reference: Option<CoverageRef>,
    #[serde(flatten)]
    page: T,
}

#[derive(Serialize)]
struct ComparisonData {
    baseline_origin: RunOrigin,
    candidate_origin: RunOrigin,
    baseline: ComparedRun,
    candidate: ComparedRun,
    #[serde(flatten)]
    page: Page<ProbeComparison>,
}

#[derive(Serialize)]
struct ProbeRow<'a> {
    id: &'a str,
    #[serde(flatten)]
    probe: &'a Probe,
    at_ns: DecimalU64,
    ttl: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    payload_bytes: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    icmp: Option<&'a IcmpIdentity>,
    outcome: &'a Outcome,
    request_evidence_id: Option<EvidenceId>,
}

impl<'a> From<&'a ProbeReport> for ProbeRow<'a> {
    fn from(probe: &'a ProbeReport) -> Self {
        Self {
            id: &probe.id,
            probe: &probe.probe,
            at_ns: DecimalU64(probe.at_ns),
            ttl: probe.ttl,
            payload_bytes: probe.payload_bytes,
            icmp: Some(&probe.icmp),
            outcome: &probe.outcome,
            request_evidence_id: probe.request_evidence_id,
        }
    }
}

#[derive(Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum RerunResult {
    Accepted { run: AcceptedRun, origin: RunOrigin },
    Rejected { error: BridgeError },
}

pub struct Backend {
    workspace: Workspace,
    config: WorkspaceConfig,
    session_id: String,
    handshaken: bool,
}

impl Backend {
    pub fn new(config: WorkspaceConfig) -> Result<Self, WorkspaceError> {
        let workspace = Workspace::new(config)?;
        static NEXT_SESSION: AtomicU64 = AtomicU64::new(0);
        let sequence = NEXT_SESSION.fetch_add(1, Ordering::Relaxed);
        // Random seeds distinguish process restarts, where Workspace IDs restart at 1.
        let high = RandomState::new().hash_one((std::process::id(), SystemTime::now(), sequence));
        let low = RandomState::new().hash_one((high, sequence));
        let session_id = ((u128::from(high) << 64) | u128::from(low)).to_string();
        Ok(Self {
            workspace,
            config,
            session_id,
            handshaken: false,
        })
    }

    pub fn handle_json(&mut self, line: &str) -> String {
        let response = match serde_json::from_str::<RawRequest>(line) {
            Err(error) => self.response(
                None,
                Err(BridgeError::json(error, ErrorCode::InvalidRequest)),
            ),
            Ok(raw) => {
                let id = Some(raw.request_id);
                match self.check_header(raw.protocol_version, raw.session_id.as_deref()) {
                    Err(error) => self.response(id, Err(error)),
                    Ok(()) => match serde_json::from_str(raw.command.get()) {
                        Err(error) => self
                            .response(id, Err(BridgeError::json(error, ErrorCode::InvalidRequest))),
                        Ok(command) => self.handle(Request {
                            protocol_version: raw.protocol_version,
                            request_id: raw.request_id,
                            session_id: raw.session_id,
                            command,
                        }),
                    },
                }
            }
        };
        // The response contains only JSON values, strings, and finite integer fields.
        serde_json::to_string(&response).expect("bridge response is JSON-compatible")
    }

    pub fn handle(&mut self, request: Request) -> Response {
        let id = Some(request.request_id);
        if let Err(error) =
            self.check_header(request.protocol_version, request.session_id.as_deref())
        {
            return self.response(id, Err(error));
        }
        if !matches!(&request.command, Command::Hello) {
            if !self.handshaken {
                return self.response(
                    id,
                    Err(BridgeError::new(
                        ErrorCode::HandshakeRequired,
                        "请先发送 hello 请求。",
                        json!({}),
                    )),
                );
            }
            if request.session_id.as_deref() != Some(self.session_id.as_str()) {
                return self.response(
                    id,
                    Err(BridgeError::new(
                        ErrorCode::SessionMismatch,
                        "请求没有绑定当前工作区会话。",
                        json!({}),
                    )),
                );
            }
        }
        if let Some(count) = request.command.page_size()
            && !(1..=MAX_PAGE_SIZE).contains(&count)
        {
            return self.response(
                id,
                Err(BridgeError::new(
                    ErrorCode::InvalidPage,
                    "每页数量须在 1 到 128 之间。",
                    json!({"count": count, "min": 1, "max": MAX_PAGE_SIZE}),
                )),
            );
        }
        let expected = request.command.expected_revision();
        let result = self.dispatch(request.command).map_err(|mut error| {
            if error.code == ErrorCode::StaleRevision
                && let Some(expected) = expected
            {
                error.details["expected"] = json!(expected);
            }
            error
        });
        self.response(id, result)
    }

    fn check_header(&self, version: u32, session: Option<&str>) -> Result<(), BridgeError> {
        if version != PROTOCOL_VERSION {
            return Err(BridgeError::new(
                ErrorCode::UnsupportedProtocol,
                "不支持该请求协议版本。",
                json!({"requested": version, "supported": [PROTOCOL_VERSION]}),
            ));
        }
        if let Some(session) = session
            && session != self.session_id
        {
            return Err(BridgeError::new(
                ErrorCode::SessionMismatch,
                "请求属于另一个工作区会话。",
                json!({}),
            ));
        }
        Ok(())
    }

    fn response(
        &self,
        request_id: Option<DecimalU64>,
        result: Result<Value, BridgeError>,
    ) -> Response {
        Response {
            protocol_version: PROTOCOL_VERSION,
            request_id,
            session_id: self.session_id.clone(),
            body: match result {
                Ok(data) => ResponseBody::Ok { data },
                Err(error) => ResponseBody::Error { error },
            },
        }
    }

    fn dispatch(&mut self, command: Command) -> Result<Value, BridgeError> {
        match command {
            Command::Hello => {
                self.handshaken = true;
                encode(json!({
                    "protocol_version": PROTOCOL_VERSION,
                    "backend_api_version": crate::BACKEND_API_VERSION,
                    "crate_version": env!("CARGO_PKG_VERSION"),
                    "session_id": self.session_id,
                    "native_formats": [1, 2, 3],
                    "report_versions": [1, 2, 3, 4, 5, 6, 7, 8, 9],
                    "commands": COMMAND_NAMES,
                    "max_page_size": MAX_PAGE_SIZE,
                    "limits": {
                        "max_documents": self.config.max_documents,
                        "bytes_per_document": self.config.bytes_per_document,
                        "document_bytes": self.config.document_bytes,
                        "queue_slots": self.config.queue_slots,
                        "resident_runs": self.config.resident_runs,
                        "input_bytes": self.config.input_bytes,
                        "history_runs": self.config.history_runs,
                        "history_trace_entries": self.config.history_trace_entries,
                        "run_limits": self.config.run_limits,
                    },
                }))
            }
            Command::ImportPreview { yaml } => encode(preview_containerlab(&yaml)?),
            Command::IosPreview { config } => encode(crate::preview_ios(&config)?),
            Command::IosDevicePreview { config } => encode(crate::preview_ios_device(&config)?),
            Command::LldpCheck { path, expectation } => {
                let bytes = crate::lldp::read_pcap(&path).map_err(BridgeError::io)?;
                let observation =
                    crate::lldp::check_lldp_pcap(&bytes, &expectation).map_err(|error| {
                        BridgeError::new(
                            ErrorCode::LldpError,
                            error.to_string(),
                            json!({"reason":error}),
                        )
                    })?;
                encode(observation)
            }
            Command::ImportOpen { yaml, options } => {
                let input = import_containerlab(&yaml, &options)?;
                encode(self.workspace.open_document(input)?)
            }
            Command::Gns3Preview { project } => encode(preview_gns3(&project)?),
            Command::Gns3Open { project, options } => {
                let input = import_gns3(&project, &options)?;
                encode(self.workspace.open_document(input)?)
            }
            Command::DocumentOpen { input, path } => {
                let info = match (input, path) {
                    (Some(input), None) => self.workspace.open_document(input)?,
                    (None, Some(path)) => self.workspace.open_file(&path)?,
                    _ => {
                        return Err(BridgeError::new(
                            ErrorCode::InvalidRequest,
                            "document.open 须且只能提供 input 或 path。",
                            json!({}),
                        ));
                    }
                };
                encode(info)
            }
            Command::DocumentGet { document_id } => {
                let doc = self.workspace.document(DocumentId(document_id.0))?;
                encode(DocumentData {
                    info: doc.info,
                    experiment: &doc.experiment,
                    presentation: doc.presentation.as_deref(),
                })
            }
            Command::DocumentInfo { document_id } => {
                encode(self.workspace.document(DocumentId(document_id.0))?.info)
            }
            Command::DocumentReplaceExperiment {
                document_id,
                expected_revision,
                input,
            } => encode(self.workspace.replace_experiment(
                DocumentId(document_id.0),
                Revision(expected_revision.0),
                input,
            )?),
            Command::DocumentReplacePresentation {
                document_id,
                expected_revision,
                presentation,
            } => encode(self.workspace.replace_presentation(
                DocumentId(document_id.0),
                Revision(expected_revision.0),
                presentation,
            )?),
            Command::DocumentValidate {
                document_id,
                expected_revision,
            } => encode(
                self.workspace
                    .validate_document(DocumentId(document_id.0), Revision(expected_revision.0))?,
            ),
            Command::DocumentEffectiveSettings {
                document_id,
                expected_revision,
                offset,
                count,
            } => encode(self.workspace.effective_settings_page(
                DocumentId(document_id.0),
                Revision(expected_revision.0),
                offset,
                count,
            )?),
            Command::DocumentPreviewFile { path } => encode(self.workspace.preview_file(&path)?),
            Command::DocumentSave {
                document_id,
                expected_revision,
                path,
            } => {
                let revision = self.workspace.save_document_at(
                    DocumentId(document_id.0),
                    Revision(expected_revision.0),
                    &path,
                )?;
                encode(json!({"document_id": document_id, "saved_revision": revision}))
            }
            Command::DocumentSaveAndRerun {
                document_id,
                expected_revision,
                path,
                mode,
            } => {
                let document = self.workspace.document(DocumentId(document_id.0))?.info;
                let saved = self.workspace.save_and_rerun_at(
                    DocumentId(document_id.0),
                    Revision(expected_revision.0),
                    &path,
                    mode.into(),
                )?;
                let rerun = match saved.rerun {
                    Ok(run) => RerunResult::Accepted {
                        run,
                        origin: RunOrigin::from_document(run, document),
                    },
                    Err(error) => RerunResult::Rejected {
                        error: error.into(),
                    },
                };
                encode(json!({
                    "document_id": document_id,
                    "saved_revision": saved.revision,
                    "effective_revision": saved.effective_revision,
                    "rerun": rerun,
                }))
            }
            Command::DocumentClose {
                document_id,
                expected_revision,
                discard_unsaved,
            } => {
                self.workspace.close_document_at(
                    DocumentId(document_id.0),
                    Revision(expected_revision.0),
                    discard_unsaved,
                )?;
                encode(json!({"document_id": document_id, "closed": true}))
            }
            Command::RunSubmit {
                document_id,
                expected_revision,
                mode,
            } => {
                // Backend exclusively owns Workspace. Capture receipt metadata before
                // acceptance so history eviction cannot turn an accepted run into an error.
                let document = self.workspace.document(DocumentId(document_id.0))?.info;
                let run = self.workspace.submit_run_at(
                    DocumentId(document_id.0),
                    Revision(expected_revision.0),
                    mode.into(),
                )?;
                encode(SubmittedRun {
                    run,
                    origin: RunOrigin::from_document(run, document),
                })
            }
            Command::RunSummary { run_id } => {
                let id = RunId(run_id.0);
                let summary = self.workspace.run_summary(id)?;
                encode(SummaryData {
                    origin: RunOrigin::from_summary(&summary),
                    summary,
                })
            }
            Command::RunControl { run_id, control } => {
                self.workspace
                    .control_run(RunId(run_id.0), control.into())?;
                encode(json!({"run_id": run_id, "control": control, "accepted": true}))
            }
            Command::RunProbes {
                run_id,
                offset,
                count,
            } => self.probes(RunId(run_id.0), offset, count),
            Command::RunEvidence {
                run_id,
                offset,
                count,
            } => self.run_page(run_id, |read| read.evidence_page(offset, count)),
            Command::RunLinks {
                run_id,
                offset,
                count,
            } => self.run_page(run_id, |read| read.link_event_page(offset, count)),
            Command::RunRoutes {
                run_id,
                offset,
                count,
            } => self.run_page(run_id, |read| read.routing_event_page(offset, count)),
            Command::RunStp {
                run_id,
                offset,
                count,
            } => self.run_page(run_id, |read| read.stp_summary_page(offset, count)),
            Command::RunStpBridges {
                run_id,
                event_id,
                offset,
                count,
            } => {
                let read = self.workspace.read_run(RunId(run_id.0))?;
                let page = read
                    .stp_bridge_page(EventId(event_id.0), offset, count)?
                    .ok_or_else(|| {
                        BridgeError::new(
                            ErrorCode::UnknownStpEvent,
                            "该运行没有对应的 STP 发布。",
                            json!({"run_id": run_id, "event_id": event_id}),
                        )
                    })?;
                encode(EventPage {
                    origin: RunOrigin::from_origin(read.id, &read.origin),
                    event_id,
                    page,
                })
            }
            Command::RunStpLinks {
                run_id,
                event_id,
                offset,
                count,
            } => {
                let read = self.workspace.read_run(RunId(run_id.0))?;
                let page = read
                    .stp_link_page(EventId(event_id.0), offset, count)?
                    .ok_or_else(|| {
                        BridgeError::new(
                            ErrorCode::UnknownStpEvent,
                            "该运行没有对应的 STP 发布。",
                            json!({"run_id": run_id, "event_id": event_id}),
                        )
                    })?;
                encode(EventPage {
                    origin: RunOrigin::from_origin(read.id, &read.origin),
                    event_id,
                    page,
                })
            }
            Command::RunCompare {
                baseline,
                candidate,
                offset,
                count,
            } => {
                let [before, after] = self
                    .workspace
                    .read_comparison(RunId(baseline.0), RunId(candidate.0))?;
                let baseline_origin = RunOrigin::from_origin(before.run.id, &before.origin);
                let candidate_origin = RunOrigin::from_origin(after.run.id, &after.origin);
                let comparison = crate::compare::compare_page(before, after, offset, count);
                encode(ComparisonData {
                    baseline_origin,
                    candidate_origin,
                    baseline: comparison.baseline,
                    candidate: comparison.candidate,
                    page: comparison.page,
                })
            }
            Command::RunImportGaps {
                run_id,
                reference,
                offset,
                count,
            } => {
                let read = self.workspace.read_run(RunId(run_id.0))?;
                let page = read
                    .import_gap_page(reference, offset, count)
                    .map_err(|error| {
                        let mut error = BridgeError::from(error);
                        if error.code == ErrorCode::InvalidCoverageReference {
                            error.details["reference"] = json!(reference);
                        }
                        error
                    })?;
                encode(CoveragePage {
                    origin: RunOrigin::from_origin(read.id, &read.origin),
                    reference,
                    page,
                })
            }
            Command::RunImportGap { run_id, gap_id } => {
                let read = self.workspace.read_run(RunId(run_id.0))?;
                let origin = RunOrigin::from_origin(read.id, &read.origin);
                let gap = read.import_gap(&gap_id)?.ok_or_else(|| {
                    BridgeError::new(
                        ErrorCode::UnknownImportGap,
                        "该运行没有对应的导入缺项。",
                        json!({"run_id": run_id, "gap_id": gap_id}),
                    )
                })?;
                encode(json!({"origin": origin, "gap": gap}))
            }
            Command::RunTrace {
                run_id,
                offset,
                count,
            } => self.run_page(run_id, |read| Ok(read.trace_page(offset, count))),
            Command::RunArchiveTo { run_id, path } => self.archive_to(RunId(run_id.0), &path),
        }
    }

    fn run_page<T: Serialize>(
        &self,
        id: DecimalU64,
        page: impl FnOnce(&crate::scheduler::RunRead) -> Result<T, WorkspaceError>,
    ) -> Result<Value, BridgeError> {
        let read = self.workspace.read_run(RunId(id.0))?;
        encode(RunPage {
            origin: RunOrigin::from_origin(read.id, &read.origin),
            page: page(&read)?,
        })
    }

    fn probes(&self, id: RunId, offset: usize, count: usize) -> Result<Value, BridgeError> {
        let read = self.workspace.read_run(id)?;
        let origin = RunOrigin::from_origin(read.id, &read.origin);
        // Borrow only the requested probe rows; never serialize the report or its evidence.
        match read.report()? {
            RunReport::Legacy(report) => encode(RunPage {
                origin,
                page: Page::indexed(1, offset, count, |_| ProbeRow {
                    id: "1",
                    probe: &report.probe,
                    at_ns: DecimalU64(0),
                    ttl: crate::topology::DEFAULT_TTL,
                    payload_bytes: None,
                    icmp: None,
                    outcome: &report.outcome,
                    request_evidence_id: None,
                }),
            }),
            RunReport::Scenario(report) => encode(RunPage {
                origin,
                page: Page::indexed(report.probes.len(), offset, count, |index| {
                    ProbeRow::from(&report.probes[index])
                }),
            }),
        }
    }

    fn archive_to(&self, id: RunId, path: &Path) -> Result<Value, BridgeError> {
        let read = self.workspace.read_run(id)?;
        let origin = RunOrigin::from_origin(read.id, &read.origin);
        let file = File::create_new(path).map_err(BridgeError::io)?;
        let mut writer = BufWriter::new(file);
        let result = (|| -> Result<(), WorkspaceError> {
            read.write_archive(&mut writer)?;
            writer.flush()?;
            writer.get_ref().sync_all()?;
            Ok(())
        })();
        drop(writer);
        if let Err(error) = result {
            let primary = BridgeError::from(error);
            if let Err(cleanup) = fs::remove_file(path) {
                let mut error = BridgeError::io(cleanup);
                error.details["operation"] = json!("archive_cleanup");
                error.details["primary_error"] = json!(primary);
                return Err(error);
            }
            return Err(primary);
        }
        encode(json!({"origin": origin, "path": path}))
    }
}

fn encode(value: impl Serialize) -> Result<Value, BridgeError> {
    serde_json::to_value(value)
        .map_err(|error| BridgeError::json(error, ErrorCode::SerializationError))
}
