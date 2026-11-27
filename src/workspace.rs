use std::{
    borrow::Cow,
    collections::HashMap,
    fmt,
    fs::{self, File},
    io::{self, BufWriter, Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;

mod save;
use save::{ExpectedFile, FileBaseline};

const PRESENTATION_FIELD_BYTES: usize = ",\"presentation\":".len();

use crate::{
    AcceptedRun, EventId, EvidenceItem, LinkEvent, Page, RunComparison, RunControl, RunId,
    RunLimits, RunMode, RunReport, RunStatus, RunSummary, SchedulerStats, TracePage, TraceReport,
    TraceStatus, ValidationResult,
    report::decimal,
    scheduler::{RunRead, Scheduler},
    validation::compile_for_workspace,
};

#[derive(Clone, Copy, Debug)]
pub struct WorkspaceConfig {
    pub workers: usize,
    pub queue_slots: usize,
    pub resident_runs: usize,
    pub input_bytes: usize,
    pub max_documents: usize,
    pub document_bytes: usize,
    pub bytes_per_document: usize,
    pub history_runs: usize,
    pub history_trace_entries: usize,
    pub batch_work: usize,
    pub run_limits: RunLimits,
}

impl Default for WorkspaceConfig {
    fn default() -> Self {
        Self {
            workers: 2,
            queue_slots: 8,
            resident_runs: 4,
            input_bytes: 8 * 1024 * 1024,
            max_documents: 128,
            document_bytes: 64 * 1024 * 1024,
            bytes_per_document: 1024 * 1024,
            history_runs: 8,
            history_trace_entries: 4096,
            batch_work: 512,
            run_limits: RunLimits {
                trace_entries: 1024,
                ..RunLimits::default()
            },
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct DocumentId(#[serde(serialize_with = "decimal")] pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct Revision(#[serde(serialize_with = "decimal")] pub u64);

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DocumentInfo {
    pub id: DocumentId,
    pub topology_id: String,
    pub format_version: u32,
    pub native_revision: String,
    pub revision: Revision,
    pub effective_revision: Revision,
    pub saved_revision: Option<Revision>,
    pub path: Option<PathBuf>,
    pub dirty: bool,
}

impl DocumentInfo {
    fn run_origin(&self) -> crate::RunOrigin {
        crate::RunOrigin {
            document_id: self.id,
            document_revision: self.revision,
            effective_revision: self.effective_revision,
            topology_id: self.topology_id.clone(),
            native_revision: self.native_revision.clone(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct DocumentView {
    pub info: DocumentInfo,
    pub experiment: Arc<str>,
    pub presentation: Option<Arc<str>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DocumentValidation {
    pub document_id: DocumentId,
    pub revision: Revision,
    pub effective_revision: Revision,
    #[serde(flatten)]
    pub result: ValidationResult,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct EffectiveSettingsPage {
    pub document_id: DocumentId,
    pub revision: Revision,
    pub effective_revision: Revision,
    #[serde(flatten)]
    pub page: Page<crate::EffectiveSetting>,
}

#[derive(Debug)]
pub struct SaveAndRun {
    pub revision: Revision,
    pub effective_revision: Revision,
    pub rerun: Result<AcceptedRun, WorkspaceError>,
}

#[derive(Debug)]
pub enum WorkspaceError {
    InvalidConfig,
    DocumentTooLarge,
    DocumentBudget,
    DocumentSlots,
    UnknownDocument(DocumentId),
    StaleEdit {
        actual: Revision,
    },
    UnsavedDocument(DocumentId),
    InvalidDocument(String),
    InvalidCoverageReference,
    PathInUse(PathBuf),
    SaveConflict(PathBuf),
    QueueFull,
    InputBudget,
    UnknownRun(RunId),
    ReportUnavailable(RunId),
    InvalidControl {
        status: RunStatus,
        control: RunControl,
    },
    Io(io::Error),
    Json(serde_json::Error),
    Input(crate::InputError),
    Archive(crate::ArchiveError),
}

impl fmt::Display for WorkspaceError {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfig => {
                out.write_str("工作区的线程、待办、驻留、文档、历史和批次上限必须大于零。")
            }
            Self::DocumentTooLarge => out.write_str("文档超过单文件字节上限。"),
            Self::DocumentBudget => out.write_str("工作区的文档驻留空间已满。"),
            Self::DocumentSlots => out.write_str("工作区打开的文档已达上限。"),
            Self::UnknownDocument(id) => write!(out, "文档 {} 未打开。", id.0),
            Self::StaleEdit { actual } => {
                write!(out, "编辑所基于的文档修订已过期，当前修订为 {}。", actual.0)
            }
            Self::UnsavedDocument(id) => write!(out, "文档 {} 还有未保存的修改。", id.0),
            Self::InvalidDocument(message) => out.write_str(message),
            Self::InvalidCoverageReference => out.write_str("缺项引用指向不存在的覆盖组件。"),
            Self::PathInUse(path) => write!(out, "{} 已由另一个打开的文档占用。", path.display()),
            Self::SaveConflict(path) => {
                write!(out, "{} 的磁盘内容已改变或正在保存。", path.display())
            }
            Self::QueueFull => out.write_str("实验待办已满。"),
            Self::InputBudget => out.write_str("实验输入快照空间已满。"),
            Self::UnknownRun(id) => write!(out, "运行 {} 不在当前工作区的保留范围。", id.0),
            Self::ReportUnavailable(id) => write!(out, "运行 {} 暂无仿真报告。", id.0),
            Self::InvalidControl { status, control } => {
                write!(out, "运行处于 {status:?}，无法执行 {control:?}。")
            }
            Self::Io(error) => error.fmt(out),
            Self::Json(error) => error.fmt(out),
            Self::Input(error) => error.fmt(out),
            Self::Archive(error) => error.fmt(out),
        }
    }
}

impl std::error::Error for WorkspaceError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Json(error) => Some(error),
            Self::Input(error) => Some(error),
            Self::Archive(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for WorkspaceError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for WorkspaceError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct NativeDocument<'a> {
    format_version: u32,
    #[serde(borrow)]
    id: Cow<'a, str>,
    #[serde(borrow)]
    revision: Cow<'a, str>,
    #[serde(borrow)]
    devices: &'a RawValue,
    #[serde(borrow)]
    links: &'a RawValue,
    #[serde(borrow, skip_serializing_if = "Option::is_none")]
    probe: Option<&'a RawValue>,
    #[serde(borrow, skip_serializing_if = "Option::is_none")]
    scenario: Option<&'a RawValue>,
    #[serde(
        borrow,
        default,
        deserialize_with = "provided_raw",
        skip_serializing_if = "Option::is_none"
    )]
    import_source: Option<&'a RawValue>,
    #[serde(borrow, skip_serializing_if = "Option::is_none")]
    presentation: Option<&'a RawValue>,
}

fn provided_raw<'de, D: serde::Deserializer<'de>>(
    decoder: D,
) -> Result<Option<&'de RawValue>, D::Error> {
    let raw = <&RawValue>::deserialize(decoder)?;
    if raw.get() == "null" {
        return Err(serde::de::Error::custom(
            "import_source 须为对象，不能为 null。",
        ));
    }
    Ok(Some(raw))
}

impl<'a> NativeDocument<'a> {
    fn split_layout(&mut self) -> Result<Option<(String, Arc<str>)>, WorkspaceError> {
        let Some(layout) = self.presentation.take() else {
            return Ok(None);
        };
        Ok(Some((
            serde_json::to_string(self)?,
            presentation_value(layout.get())?,
        )))
    }

    fn read(input: &'a str) -> Result<Self, WorkspaceError> {
        let input = input.strip_prefix('\u{feff}').unwrap_or(input);
        let doc: Self = serde_json::from_str(input)?;
        if doc.import_source.is_some() && doc.format_version != 3 {
            return Err(WorkspaceError::Input(
                crate::InputError::new("导入缺项需要 format_version 3。".into())
                    .code("unsupported_feature")
                    .at("/import_source"),
            ));
        }
        if doc.id.trim().is_empty() || doc.revision.trim().is_empty() {
            return Err(WorkspaceError::InvalidDocument(
                "拓扑 ID 和原生修订不能为空。".into(),
            ));
        }
        match (
            doc.format_version,
            doc.probe.is_some(),
            doc.scenario.is_some(),
        ) {
            (1, true, false) | (2 | 3, false, true) => Ok(doc),
            (1..=3, _, _) => Err(WorkspaceError::InvalidDocument(
                "版本 1 需要 probe，版本 2 需要 scenario。".into(),
            )),
            (version, _, _) => Err(WorkspaceError::InvalidDocument(format!(
                "暂不支持文件版本 format_version {version}。"
            ))),
        }
    }
}

struct SavedFile {
    selected_path: PathBuf,
    baseline: FileBaseline,
}

struct Draft {
    info: DocumentInfo,
    saved_file: Option<SavedFile>,
    experiment: Arc<str>,
    presentation: Option<Arc<str>>,
}

impl Draft {
    fn bytes(&self) -> usize {
        self.experiment.len() + self.presentation.as_ref().map_or(0, |layout| layout.len())
    }

    fn view(&self) -> DocumentView {
        DocumentView {
            info: self.info.clone(),
            experiment: Arc::clone(&self.experiment),
            presentation: self.presentation.clone(),
        }
    }

    fn verify(&self, revision: Revision) -> Result<(), WorkspaceError> {
        if self.info.revision != revision {
            return Err(WorkspaceError::StaleEdit {
                actual: self.info.revision,
            });
        }
        Ok(())
    }

    fn edited(&mut self, effective: bool) -> DocumentInfo {
        self.info.revision.0 += 1;
        if effective {
            self.info.effective_revision.0 += 1;
        }
        self.info.dirty = true;
        self.info.clone()
    }
}

#[derive(Default)]
struct Documents {
    next_id: u64,
    entries: HashMap<DocumentId, Draft>,
    bytes: usize,
}

impl Documents {
    fn get(&self, id: DocumentId) -> Result<&Draft, WorkspaceError> {
        self.entries
            .get(&id)
            .ok_or(WorkspaceError::UnknownDocument(id))
    }

    fn path_available(
        &self,
        path: &Path,
        except: Option<DocumentId>,
    ) -> Result<(), WorkspaceError> {
        for doc in self.entries.values() {
            if Some(doc.info.id) != except
                && let Some(saved) = &doc.info.path
                && save::same_path(saved, path)?
            {
                return Err(WorkspaceError::PathInUse(path.to_owned()));
            }
        }
        Ok(())
    }
}

pub struct Workspace {
    config: WorkspaceConfig,
    documents: Mutex<Documents>,
    save_gate: Mutex<()>,
    scheduler: Scheduler,
}

impl Workspace {
    pub fn new(config: WorkspaceConfig) -> Result<Self, WorkspaceError> {
        if [
            config.workers,
            config.queue_slots,
            config.resident_runs,
            config.input_bytes,
            config.max_documents,
            config.document_bytes,
            config.bytes_per_document,
            config.history_runs,
            config.batch_work,
        ]
        .contains(&0)
        {
            return Err(WorkspaceError::InvalidConfig);
        }
        Ok(Self {
            config,
            documents: Mutex::new(Documents::default()),
            save_gate: Mutex::new(()),
            scheduler: Scheduler::new(config)?,
        })
    }

    pub fn open_document(&self, input: String) -> Result<DocumentInfo, WorkspaceError> {
        self.open(input, None)
    }

    pub fn open_file(&self, path: &Path) -> Result<DocumentInfo, WorkspaceError> {
        let _saving = self.save_gate.lock().unwrap();
        let selected_path = std::path::absolute(path)?;
        let path = fs::canonicalize(&selected_path)?;
        self.documents.lock().unwrap().path_available(&path, None)?;
        let input = self.read_file(&path)?;
        let binding = SavedFile {
            selected_path,
            baseline: FileBaseline::of_bytes(input.as_bytes()),
        };
        self.open(input, Some((path, binding)))
    }

    pub fn preview_file(&self, path: &Path) -> Result<crate::NativePreview, WorkspaceError> {
        let input = self.read_file(path)?;
        if input.len() > self.config.bytes_per_document {
            // Only a native document with a split layout gets the extra wire bytes.
            let layout = NativeDocument::read(&input)
                .ok()
                .and_then(|mut native| native.split_layout().ok().flatten());
            self.check_size(layout.map_or(input.len(), |(experiment, layout)| {
                experiment.len() + layout.len()
            }))?;
        }
        Ok(crate::preview_native(input))
    }

    fn read_file(&self, path: &Path) -> Result<String, WorkspaceError> {
        let limit = self
            .config
            .bytes_per_document
            .saturating_add(PRESENTATION_FIELD_BYTES);
        let mut bytes = Vec::new();
        File::open(path)?
            .take((limit as u64).saturating_add(1))
            .read_to_end(&mut bytes)?;
        if bytes.len() > limit {
            return Err(WorkspaceError::DocumentTooLarge);
        }
        String::from_utf8(bytes).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e).into())
    }

    fn open(
        &self,
        input: String,
        file: Option<(PathBuf, SavedFile)>,
    ) -> Result<DocumentInfo, WorkspaceError> {
        if input.len()
            > self
                .config
                .bytes_per_document
                .saturating_add(PRESENTATION_FIELD_BYTES)
        {
            return Err(WorkspaceError::DocumentTooLarge);
        }
        let mut native = NativeDocument::read(&input)?;
        let topology_id = native.id.to_string();
        let native_revision = native.revision.to_string();
        let format_version = native.format_version;
        let (path, saved_file) = file.unzip();
        let (experiment, presentation): (Arc<str>, _) = match native.split_layout()? {
            Some((experiment, layout)) => (experiment.into(), Some(layout)),
            None => (input.into(), None),
        };
        let bytes = experiment.len() + presentation.as_ref().map_or(0, |layout| layout.len());
        self.check_size(bytes)?;
        let mut docs = self.documents.lock().unwrap();
        if docs.entries.len() == self.config.max_documents {
            return Err(WorkspaceError::DocumentSlots);
        }
        if bytes > self.config.document_bytes - docs.bytes {
            return Err(WorkspaceError::DocumentBudget);
        }
        docs.next_id += 1;
        let revision = Revision(1);
        let info = DocumentInfo {
            id: DocumentId(docs.next_id),
            topology_id,
            format_version,
            native_revision,
            revision,
            effective_revision: revision,
            saved_revision: path.as_ref().map(|_| revision),
            dirty: path.is_none(),
            path,
        };
        docs.bytes += bytes;
        docs.entries.insert(
            info.id,
            Draft {
                info: info.clone(),
                saved_file,
                experiment,
                presentation,
            },
        );
        Ok(info)
    }

    pub fn document(&self, id: DocumentId) -> Result<DocumentView, WorkspaceError> {
        Ok(self.documents.lock().unwrap().get(id)?.view())
    }

    pub fn document_usage(&self) -> (usize, usize) {
        let docs = self.documents.lock().unwrap();
        (docs.entries.len(), docs.bytes)
    }

    pub fn validate_document(
        &self,
        id: DocumentId,
        revision: Revision,
    ) -> Result<DocumentValidation, WorkspaceError> {
        let doc = self.document(id)?;
        if doc.info.revision != revision {
            return Err(WorkspaceError::StaleEdit {
                actual: doc.info.revision,
            });
        }
        let result = compile_for_workspace(&doc.experiment).map(crate::ValidationSummary::from);
        Ok(DocumentValidation {
            document_id: id,
            revision,
            effective_revision: doc.info.effective_revision,
            result: result.into(),
        })
    }

    pub fn effective_settings_page(
        &self,
        id: DocumentId,
        revision: Revision,
        offset: usize,
        count: usize,
    ) -> Result<EffectiveSettingsPage, WorkspaceError> {
        let doc = self.document(id)?;
        if doc.info.revision != revision {
            return Err(WorkspaceError::StaleEdit {
                actual: doc.info.revision,
            });
        }
        let topology = compile_for_workspace(&doc.experiment).map_err(WorkspaceError::Input)?;
        let page =
            crate::effective::collect_page(&topology, &doc.experiment, offset, count.min(128));
        Ok(EffectiveSettingsPage {
            document_id: id,
            revision,
            effective_revision: doc.info.effective_revision,
            page,
        })
    }

    pub fn close_document(
        &self,
        id: DocumentId,
        discard_unsaved: bool,
    ) -> Result<(), WorkspaceError> {
        self.close_document_checked(id, None, discard_unsaved)
    }

    pub fn close_document_at(
        &self,
        id: DocumentId,
        revision: Revision,
        discard_unsaved: bool,
    ) -> Result<(), WorkspaceError> {
        self.close_document_checked(id, Some(revision), discard_unsaved)
    }

    fn close_document_checked(
        &self,
        id: DocumentId,
        revision: Option<Revision>,
        discard_unsaved: bool,
    ) -> Result<(), WorkspaceError> {
        let _saving = self.save_gate.lock().unwrap();
        let mut docs = self.documents.lock().unwrap();
        let doc = docs.get(id)?;
        if let Some(revision) = revision {
            doc.verify(revision)?;
        }
        if doc.info.dirty && !discard_unsaved {
            return Err(WorkspaceError::UnsavedDocument(id));
        }
        docs.bytes -= doc.bytes();
        docs.entries.remove(&id);
        Ok(())
    }

    pub fn replace_experiment(
        &self,
        id: DocumentId,
        revision: Revision,
        input: String,
    ) -> Result<DocumentInfo, WorkspaceError> {
        self.check_size(input.len())?;
        let native = NativeDocument::read(&input)?;
        if native.presentation.is_some() {
            return Err(WorkspaceError::InvalidDocument(
                "画布数据通过 replace_presentation 编辑。".into(),
            ));
        }
        let mut docs = self.documents.lock().unwrap();
        let doc = docs.get(id)?;
        doc.verify(revision)?;
        if doc.info.topology_id != native.id {
            return Err(WorkspaceError::InvalidDocument(
                "编辑不能改变拓扑 ID。".into(),
            ));
        }
        if doc.experiment.as_ref() == input {
            return Ok(doc.info.clone());
        }
        let format_version = native.format_version;
        let native_revision = native.revision.to_string();
        let new_size = input.len() + doc.presentation.as_ref().map_or(0, |layout| layout.len());
        self.check_size(new_size)?;
        let bytes = self.change_bytes(&docs, doc, new_size)?;
        let doc = docs.entries.get_mut(&id).expect("open document");
        doc.experiment = input.into();
        doc.info.format_version = format_version;
        doc.info.native_revision = native_revision;
        let result = doc.edited(true);
        docs.bytes = bytes;
        Ok(result)
    }

    pub fn replace_presentation(
        &self,
        id: DocumentId,
        revision: Revision,
        presentation: Option<String>,
    ) -> Result<DocumentInfo, WorkspaceError> {
        let presentation = presentation
            .as_deref()
            .map(|text| {
                self.check_size(text.len())?;
                let raw: &RawValue = serde_json::from_str(text)?;
                presentation_value(raw.get())
            })
            .transpose()?;
        let mut docs = self.documents.lock().unwrap();
        let doc = docs.get(id)?;
        doc.verify(revision)?;
        if doc.presentation == presentation {
            return Ok(doc.info.clone());
        }
        let new_size =
            doc.experiment.len() + presentation.as_ref().map_or(0, |layout| layout.len());
        self.check_size(new_size)?;
        let bytes = self.change_bytes(&docs, doc, new_size)?;
        let doc = docs.entries.get_mut(&id).expect("open document");
        doc.presentation = presentation;
        let result = doc.edited(false);
        docs.bytes = bytes;
        Ok(result)
    }

    fn check_size(&self, bytes: usize) -> Result<(), WorkspaceError> {
        if bytes > self.config.bytes_per_document {
            return Err(WorkspaceError::DocumentTooLarge);
        }
        Ok(())
    }

    fn change_bytes(
        &self,
        docs: &Documents,
        doc: &Draft,
        new_size: usize,
    ) -> Result<usize, WorkspaceError> {
        let rest = docs.bytes - doc.bytes();
        if new_size > self.config.document_bytes - rest {
            return Err(WorkspaceError::DocumentBudget);
        }
        Ok(rest + new_size)
    }

    pub fn submit_run(&self, id: DocumentId, mode: RunMode) -> Result<AcceptedRun, WorkspaceError> {
        self.submit_run_checked(id, None, mode)
    }

    pub fn submit_run_at(
        &self,
        id: DocumentId,
        revision: Revision,
        mode: RunMode,
    ) -> Result<AcceptedRun, WorkspaceError> {
        self.submit_run_checked(id, Some(revision), mode)
    }

    fn submit_run_checked(
        &self,
        id: DocumentId,
        revision: Option<Revision>,
        mode: RunMode,
    ) -> Result<AcceptedRun, WorkspaceError> {
        let docs = self.documents.lock().unwrap();
        let doc = docs.get(id)?;
        if let Some(revision) = revision {
            doc.verify(revision)?;
        }
        self.scheduler
            .submit(doc.info.run_origin(), &doc.experiment, mode)
    }

    pub fn save_document(&self, id: DocumentId, path: &Path) -> Result<Revision, WorkspaceError> {
        self.save_document_checked(id, None, path)
    }

    pub fn save_document_at(
        &self,
        id: DocumentId,
        revision: Revision,
        path: &Path,
    ) -> Result<Revision, WorkspaceError> {
        self.save_document_checked(id, Some(revision), path)
    }

    fn save_document_checked(
        &self,
        id: DocumentId,
        revision: Option<Revision>,
        path: &Path,
    ) -> Result<Revision, WorkspaceError> {
        let target = save_path(path)?;
        let _saving = self.save_gate.lock().unwrap();
        Ok(self.save(id, revision, target)?.info.revision)
    }

    pub fn save_and_rerun(
        &self,
        id: DocumentId,
        path: &Path,
        mode: RunMode,
    ) -> Result<SaveAndRun, WorkspaceError> {
        self.save_and_rerun_checked(id, None, path, mode)
    }

    pub fn save_and_rerun_at(
        &self,
        id: DocumentId,
        revision: Revision,
        path: &Path,
        mode: RunMode,
    ) -> Result<SaveAndRun, WorkspaceError> {
        self.save_and_rerun_checked(id, Some(revision), path, mode)
    }

    fn save_and_rerun_checked(
        &self,
        id: DocumentId,
        revision: Option<Revision>,
        path: &Path,
        mode: RunMode,
    ) -> Result<SaveAndRun, WorkspaceError> {
        let target = save_path(path)?;
        let _saving = self.save_gate.lock().unwrap();
        let snapshot = self.save(id, revision, target)?;
        let effective_revision = snapshot.info.effective_revision;
        let rerun = self
            .scheduler
            .submit(snapshot.info.run_origin(), &snapshot.experiment, mode);
        Ok(SaveAndRun {
            revision: snapshot.info.revision,
            effective_revision,
            rerun,
        })
    }

    fn save(
        &self,
        id: DocumentId,
        revision: Option<Revision>,
        target: SaveTarget,
    ) -> Result<DocumentView, WorkspaceError> {
        let SaveTarget {
            selected_path,
            path,
            expected,
        } = target;
        let (snapshot, expected) = {
            let docs = self.documents.lock().unwrap();
            let doc = docs.get(id)?;
            if let Some(revision) = revision {
                doc.verify(revision)?;
            }
            docs.path_available(&path, Some(id))?;
            let expected = match (&doc.saved_file, &doc.info.path) {
                (Some(file), Some(saved)) if save::same_path(saved, &path)? => {
                    if matches!(expected, ExpectedFile::Absent) {
                        return Err(WorkspaceError::SaveConflict(path));
                    }
                    ExpectedFile::Unchanged(file.baseline)
                }
                (Some(file), Some(saved))
                    if save::same_path(&file.selected_path, &selected_path)?
                        || save::same_path(saved, &selected_path)? =>
                {
                    // The same selected path now resolves to a different file.
                    return Err(WorkspaceError::SaveConflict(selected_path));
                }
                _ => expected,
            };
            (doc.view(), expected)
        };
        let baseline = write_document(&path, &snapshot, expected)?;
        let mut docs = self.documents.lock().unwrap();
        let doc = docs
            .entries
            .get_mut(&id)
            .expect("save gate holds document open");
        doc.info.path = Some(path);
        doc.saved_file = Some(SavedFile {
            selected_path,
            baseline,
        });
        doc.info.saved_revision = Some(snapshot.info.revision);
        doc.info.dirty = doc.info.revision != snapshot.info.revision;
        Ok(snapshot)
    }

    pub fn run_summary(&self, id: RunId) -> Result<RunSummary, WorkspaceError> {
        self.scheduler.summary(id)
    }

    pub fn wait_for_update(
        &self,
        id: RunId,
        after: u64,
        timeout: Duration,
    ) -> Result<RunSummary, WorkspaceError> {
        self.scheduler.wait(id, after, timeout)
    }

    pub fn control_run(&self, id: RunId, command: RunControl) -> Result<(), WorkspaceError> {
        self.scheduler.control(id, command)
    }

    /// The retained report contains probe outcomes and essential evidence. Detail is requested separately.
    pub fn run_report(&self, id: RunId) -> Result<Arc<RunReport>, WorkspaceError> {
        self.scheduler.report(id)
    }

    pub fn write_run_archive(&self, id: RunId, writer: impl Write) -> Result<(), WorkspaceError> {
        self.read_run(id)?.write_archive(writer)
    }

    pub(crate) fn read_run(&self, id: RunId) -> Result<RunRead, WorkspaceError> {
        self.scheduler.read(id)
    }

    pub(crate) fn read_comparison(
        &self,
        baseline: RunId,
        candidate: RunId,
    ) -> Result<[crate::compare::ComparisonSnapshot; 2], WorkspaceError> {
        self.scheduler.comparison_snapshots([baseline, candidate])
    }

    pub fn run_trace(
        &self,
        id: RunId,
    ) -> Result<(Option<TraceStatus>, Option<Arc<TraceReport>>), WorkspaceError> {
        self.scheduler.trace(id)
    }

    pub fn compare_runs(
        &self,
        baseline: RunId,
        candidate: RunId,
    ) -> Result<RunComparison, WorkspaceError> {
        let [(before, old), (after, new)] = self.scheduler.reports([baseline, candidate])?;
        Ok(crate::compare::compare_runs((before, &old), (after, &new)))
    }

    pub fn compare_run_page(
        &self,
        baseline: RunId,
        candidate: RunId,
        offset: usize,
        count: usize,
    ) -> Result<crate::RunComparisonPage, WorkspaceError> {
        let [before, after] = self.read_comparison(baseline, candidate)?;
        Ok(crate::compare::compare_page(before, after, offset, count))
    }

    pub fn run_import_gap_page(
        &self,
        id: RunId,
        reference: Option<crate::CoverageRef>,
        offset: usize,
        count: usize,
    ) -> Result<Page<crate::ImportGapSummary>, WorkspaceError> {
        self.read_run(id)?.import_gap_page(reference, offset, count)
    }

    pub fn run_import_gap(
        &self,
        id: RunId,
        gap_id: &str,
    ) -> Result<Option<crate::ImportGap>, WorkspaceError> {
        self.read_run(id)?.import_gap(gap_id)
    }

    pub fn run_stp_summary_page(
        &self,
        id: RunId,
        offset: usize,
        count: usize,
    ) -> Result<Page<crate::StpEventSummary>, WorkspaceError> {
        self.read_run(id)?.stp_summary_page(offset, count)
    }

    pub fn run_stp_bridge_page(
        &self,
        id: RunId,
        event_id: EventId,
        offset: usize,
        count: usize,
    ) -> Result<Option<Page<crate::StpBridgeState>>, WorkspaceError> {
        self.read_run(id)?.stp_bridge_page(event_id, offset, count)
    }

    pub fn run_stp_link_page(
        &self,
        id: RunId,
        event_id: EventId,
        offset: usize,
        count: usize,
    ) -> Result<Option<Page<crate::StpLinkState>>, WorkspaceError> {
        self.read_run(id)?.stp_link_page(event_id, offset, count)
    }

    pub fn run_evidence_page(
        &self,
        id: RunId,
        offset: usize,
        count: usize,
    ) -> Result<Page<EvidenceItem>, WorkspaceError> {
        self.read_run(id)?.evidence_page(offset, count)
    }

    pub fn run_link_event_page(
        &self,
        id: RunId,
        offset: usize,
        count: usize,
    ) -> Result<Page<LinkEvent>, WorkspaceError> {
        self.read_run(id)?.link_event_page(offset, count)
    }

    pub fn run_routing_event_page(
        &self,
        id: RunId,
        offset: usize,
        count: usize,
    ) -> Result<Page<crate::RoutingEvent>, WorkspaceError> {
        self.read_run(id)?.routing_event_page(offset, count)
    }

    pub fn run_trace_page(
        &self,
        id: RunId,
        offset: usize,
        count: usize,
    ) -> Result<TracePage, WorkspaceError> {
        Ok(self.read_run(id)?.trace_page(offset, count))
    }

    pub fn run_stp_event_page(
        &self,
        id: RunId,
        offset: usize,
        count: usize,
    ) -> Result<Page<crate::StpEvent>, WorkspaceError> {
        let report = self.scheduler.report(id)?;
        Ok(match report.as_ref() {
            RunReport::Legacy(_) => Page {
                entries: Vec::new(),
                next_offset: None,
            },
            RunReport::Scenario(report) => {
                Page::indexed(report.stp_events.len(), offset, count, |index| {
                    report.stp_events.get(index).expect("existing STP event")
                })
            }
        })
    }

    pub fn run_stp_event(
        &self,
        id: RunId,
        event_id: EventId,
    ) -> Result<Option<crate::StpEvent>, WorkspaceError> {
        let report = self.scheduler.report(id)?;
        Ok(match report.as_ref() {
            RunReport::Legacy(_) => None,
            RunReport::Scenario(report) => report.stp_events.by_event_id(event_id),
        })
    }

    pub fn stats(&self) -> SchedulerStats {
        self.scheduler.stats()
    }
}

impl RunRead {
    pub(crate) fn report(&self) -> Result<&RunReport, WorkspaceError> {
        self.report
            .as_deref()
            .ok_or(WorkspaceError::ReportUnavailable(self.id))
    }

    pub(crate) fn write_archive(&self, writer: impl Write) -> Result<(), WorkspaceError> {
        let report = self.report()?;
        let source = crate::ArchiveSource {
            run_id: self.id.0.to_string(),
            document_revision: self.origin.document_revision.0.to_string(),
            effective_revision: self.origin.effective_revision.0.to_string(),
        };
        let missing = self.trace_status.map(|status| TraceReport {
            status,
            entries: Vec::new(),
        });
        let trace = self.trace.as_deref().or(missing.as_ref());
        crate::write_archive(report, trace, Some(&source), writer).map_err(WorkspaceError::Archive)
    }

    pub(crate) fn import_gap_page(
        &self,
        reference: Option<crate::CoverageRef>,
        offset: usize,
        count: usize,
    ) -> Result<Page<crate::ImportGapSummary>, WorkspaceError> {
        let (source, coverage) = match self.report()? {
            RunReport::Legacy(_) => (None, None),
            RunReport::Scenario(report) => {
                (report.import_source.as_ref(), report.coverage.as_ref())
            }
        };
        let affected = if let Some(reference) = reference {
            let coverage = coverage.ok_or(WorkspaceError::InvalidCoverageReference)?;
            if ![reference.source_component, reference.destination_component]
                .into_iter()
                .all(|id| {
                    coverage
                        .components
                        .iter()
                        .any(|component| component.id == id)
                })
            {
                return Err(WorkspaceError::InvalidCoverageReference);
            }
            Some(
                coverage
                    .gap_ids(reference)
                    .collect::<std::collections::HashSet<_>>(),
            )
        } else {
            None
        };
        let mut gaps = source
            .into_iter()
            .flat_map(|source| &source.gaps)
            .filter(|gap| {
                affected
                    .as_ref()
                    .is_none_or(|ids| ids.contains(gap.id.as_str()))
            })
            .skip(offset);
        let entries: Vec<_> = gaps
            .by_ref()
            .take(count.min(128))
            .map(crate::ImportGapSummary::from)
            .collect();
        let next_offset =
            (!entries.is_empty() && gaps.next().is_some()).then_some(offset + entries.len());
        Ok(Page {
            entries,
            next_offset,
        })
    }

    pub(crate) fn import_gap(
        &self,
        gap_id: &str,
    ) -> Result<Option<crate::ImportGap>, WorkspaceError> {
        Ok(match self.report()? {
            RunReport::Legacy(_) => None,
            RunReport::Scenario(report) => report
                .import_source
                .as_ref()
                .and_then(|source| source.gaps.iter().find(|gap| gap.id == gap_id))
                .cloned(),
        })
    }

    pub(crate) fn stp_summary_page(
        &self,
        offset: usize,
        count: usize,
    ) -> Result<Page<crate::StpEventSummary>, WorkspaceError> {
        Ok(match self.report()? {
            RunReport::Legacy(_) => Page {
                entries: Vec::new(),
                next_offset: None,
            },
            RunReport::Scenario(report) => report.stp_events.summary_page(offset, count),
        })
    }

    pub(crate) fn stp_bridge_page(
        &self,
        event_id: EventId,
        offset: usize,
        count: usize,
    ) -> Result<Option<Page<crate::StpBridgeState>>, WorkspaceError> {
        Ok(match self.report()? {
            RunReport::Legacy(_) => None,
            RunReport::Scenario(report) => report.stp_events.bridge_page(event_id, offset, count),
        })
    }

    pub(crate) fn stp_link_page(
        &self,
        event_id: EventId,
        offset: usize,
        count: usize,
    ) -> Result<Option<Page<crate::StpLinkState>>, WorkspaceError> {
        Ok(match self.report()? {
            RunReport::Legacy(_) => None,
            RunReport::Scenario(report) => report.stp_events.link_page(event_id, offset, count),
        })
    }

    pub(crate) fn evidence_page(
        &self,
        offset: usize,
        count: usize,
    ) -> Result<Page<EvidenceItem>, WorkspaceError> {
        Ok(match self.report()? {
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
        })
    }

    pub(crate) fn link_event_page(
        &self,
        offset: usize,
        count: usize,
    ) -> Result<Page<LinkEvent>, WorkspaceError> {
        let events = match self.report()? {
            RunReport::Legacy(_) => &[][..],
            RunReport::Scenario(report) => report.link_events.as_slice(),
        };
        Ok(Page::collect(events, offset, count, Clone::clone))
    }

    pub(crate) fn routing_event_page(
        &self,
        offset: usize,
        count: usize,
    ) -> Result<Page<crate::RoutingEvent>, WorkspaceError> {
        let events = match self.report()? {
            RunReport::Legacy(_) => &[][..],
            RunReport::Scenario(report) => report.routing_events.as_slice(),
        };
        Ok(Page::collect(events, offset, count, Clone::clone))
    }

    pub(crate) fn trace_page(&self, offset: usize, count: usize) -> TracePage {
        let entries = self
            .trace
            .as_ref()
            .map_or(&[][..], |trace| trace.entries.as_slice());
        TracePage {
            status: self.trace_status,
            page: Page::collect(entries, offset, count, Clone::clone),
        }
    }
}

fn presentation_value(json: &str) -> Result<Arc<str>, WorkspaceError> {
    if !json.starts_with('{') {
        return Err(WorkspaceError::InvalidDocument(
            "presentation 须为 JSON 对象。".into(),
        ));
    }
    Ok(json.into())
}

struct SaveTarget {
    selected_path: PathBuf,
    path: PathBuf,
    expected: ExpectedFile,
}

fn save_path(path: &Path) -> Result<SaveTarget, WorkspaceError> {
    let selected_path = std::path::absolute(path)?;
    let (path, expected) = match fs::canonicalize(&selected_path) {
        Ok(path) => (path, ExpectedFile::OverwriteExisting),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let name = selected_path.file_name().ok_or(error)?;
            let parent = selected_path
                .parent()
                .expect("absolute path with file name");
            (fs::canonicalize(parent)?.join(name), ExpectedFile::Absent)
        }
        Err(error) => return Err(error.into()),
    };
    Ok(SaveTarget {
        selected_path,
        path,
        expected,
    })
}

fn write_document(
    path: &Path,
    document: &DocumentView,
    expected: ExpectedFile,
) -> Result<FileBaseline, WorkspaceError> {
    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
    let (temporary, file) = loop {
        let name = format!(
            ".reliner-{}-{}.tmp",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        );
        if path
            .file_name()
            .is_some_and(|target| target.eq_ignore_ascii_case(&name))
        {
            continue;
        }
        let temporary = path.with_file_name(name);
        match save::create_staged(&temporary, expected) {
            Ok(file) => break (temporary, file),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.into()),
        }
    };
    let result = (|| {
        let mut writer = BufWriter::new(file);
        if let Some(layout) = &document.presentation {
            let mut native = NativeDocument::read(&document.experiment)?;
            native.presentation = Some(serde_json::from_str(layout)?);
            serde_json::to_writer(&mut writer, &native)?;
        } else {
            writer.write_all(document.experiment.as_bytes())?;
        }
        writer.flush()?;
        writer.get_ref().sync_all()?;
        drop(writer);
        let baseline = FileBaseline::of_file(&File::open(&temporary)?)?;
        save::replace(&temporary, path, expected)?;
        Ok(baseline)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[cfg(test)]
mod read_tests {
    use super::*;

    #[test]
    fn first_save_as_keeps_the_absence_observed_by_each_transaction() {
        let directory = std::env::current_dir()
            .unwrap()
            .join(format!("reliner-first-save-{}", std::process::id()));
        fs::create_dir(&directory).unwrap();
        let name = directory.join("Lab.json");
        #[cfg(windows)]
        let other_name = {
            fs::write(&name, b"").unwrap();
            let alias = directory.join("LAB.json");
            let other = if alias.exists() { alias } else { name.clone() };
            fs::remove_file(&name).unwrap();
            other
        };
        #[cfg(not(windows))]
        let other_name = name.clone();
        let first = Workspace::new(WorkspaceConfig::default()).unwrap();
        let second = Workspace::new(WorkspaceConfig::default()).unwrap();
        let a = first
            .open_document(include_str!("../examples/vlan-lab.json").into())
            .unwrap();
        let b = second
            .open_document(include_str!("../examples/ipv4-lab.json").into())
            .unwrap();
        // Both calls have selected an absent name. Commit B after A has even released its lock.
        let first_target = save_path(&name).unwrap();
        let second_target = save_path(&other_name).unwrap();
        first.save(a.id, Some(a.revision), first_target).unwrap();
        let result = second.save(b.id, Some(b.revision), second_target);
        let disk = fs::read_to_string(&name).unwrap();
        let unchanged = second.document(b.id).unwrap().info;
        // A fresh, explicit Save As may replace the now-existing destination.
        let overwrite = second.save_document(b.id, &other_name);
        let replaced = fs::read_to_string(&other_name).unwrap();
        fs::remove_file(&name).unwrap();
        if other_name.exists() {
            fs::remove_file(&other_name).unwrap();
        }
        fs::remove_dir(&directory).unwrap();
        assert!(matches!(result, Err(WorkspaceError::SaveConflict(_))));
        assert_eq!(disk, include_str!("../examples/vlan-lab.json"));
        assert_eq!(unchanged, b);
        assert_eq!(overwrite.unwrap(), b.revision);
        assert_eq!(replaced, include_str!("../examples/ipv4-lab.json"));
    }

    #[cfg(unix)]
    #[test]
    fn unix_replacement_stage_is_private_before_writing() {
        use std::os::unix::fs::PermissionsExt;
        let path = std::env::current_dir()
            .unwrap()
            .join(format!("reliner-private-stage-{}", std::process::id()));
        let file = save::create_staged(&path, ExpectedFile::OverwriteExisting).unwrap();
        let mode = file.metadata().unwrap().permissions().mode();
        drop(file);
        fs::remove_file(path).unwrap();
        assert_eq!(mode & 0o077, 0);
    }

    fn finish(workspace: &Workspace, document: DocumentId) -> RunId {
        let run = workspace.submit_run(document, RunMode::Automatic).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let mut state = workspace.run_summary(run.id).unwrap();
        while !state.status.is_terminal() {
            assert!(std::time::Instant::now() < deadline);
            state = workspace
                .wait_for_update(run.id, state.update, Duration::from_millis(50))
                .unwrap();
        }
        assert_eq!(state.status, RunStatus::Finished);
        run.id
    }

    #[test]
    fn acquired_run_details_remain_coherent_after_history_eviction() {
        let workspace = Workspace::new(WorkspaceConfig {
            history_runs: 1,
            ..Default::default()
        })
        .unwrap();
        let document = workspace
            .open_document(include_str!("../examples/stp-lab.json").into())
            .unwrap();
        let first = finish(&workspace, document.id);
        let read = workspace.read_run(first).unwrap();
        assert_eq!(read.origin.document_revision, document.revision);
        let header = read.stp_summary_page(0, 1).unwrap().entries.remove(0);
        assert!(header.bridge_count > 0);

        finish(&workspace, document.id);
        assert!(
            matches!(workspace.read_run(first), Err(WorkspaceError::UnknownRun(id)) if id == first)
        );
        assert_eq!(
            read.stp_bridge_page(header.event_id, 0, 1)
                .unwrap()
                .unwrap()
                .entries
                .len(),
            1
        );
        assert_eq!(read.origin.document_revision, document.revision);
        let mut bytes = Vec::new();
        read.write_archive(&mut bytes).unwrap();
        let archived = crate::read_archive(bytes.as_slice()).unwrap();
        assert_eq!(archived.source.unwrap().run_id, first.0.to_string());
        assert_eq!(&archived.report, read.report().unwrap());
    }

    #[test]
    fn comparison_acquires_both_origins_and_reports_before_eviction() {
        let workspace = Workspace::new(WorkspaceConfig {
            history_runs: 2,
            ..Default::default()
        })
        .unwrap();
        let input = include_str!("../examples/ipv4-lab.json");
        let document = workspace.open_document(input.into()).unwrap();
        let first = finish(&workspace, document.id);
        let mut changed: serde_json::Value = serde_json::from_str(input).unwrap();
        changed["revision"] = "new".into();
        let revised = workspace
            .replace_experiment(document.id, document.revision, changed.to_string())
            .unwrap();
        let second = finish(&workspace, document.id);
        let [before, after] = workspace.read_comparison(first, second).unwrap();

        finish(&workspace, document.id);
        assert!(matches!(
            workspace.run_summary(first),
            Err(WorkspaceError::UnknownRun(_))
        ));
        assert_eq!(before.origin.document_revision, document.revision);
        assert_eq!(after.origin.document_revision, revised.revision);
        let comparison = crate::compare::compare_page(before, after, 0, 1);
        assert_eq!(comparison.baseline.revision, "1");
        assert_eq!(comparison.candidate.revision, "new");
        assert!(comparison.page.entries[0].same_probe_parameters);
    }
}
