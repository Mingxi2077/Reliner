import { invoke } from "@tauri-apps/api/core";
import type {
  BridgeError,
  BridgeResponse,
  ComparisonData,
  CoverageRef,
  DecimalU64,
  DocumentData,
  DocumentInfo,
  DocumentValidation,
  EffectiveSettingsPage,
  EvidenceItem,
  ExampleSummary,
  HandshakeState,
  LinkEvent,
  ProbeRow,
  RoutingEvent,
  RunControl,
  RunImportGapsResponse,
  RunMode,
  RunPageResponse,
  RunTraceResponse,
  SaveAndRerunResult,
  SaveResult,
  StpBridgeState,
  StpEventPageResponse,
  StpEventSummary,
  StpLinkState,
  SubmittedRun,
  SummaryData,
} from "./types";

export class BackendError extends Error {
  readonly code: BridgeError["code"];
  readonly details: Record<string, unknown>;

  constructor(error: BridgeError) {
    super(error.message);
    this.name = "BackendError";
    this.code = error.code;
    this.details = error.details;
  }
}

export interface BridgeTransport {
  handshake(): Promise<HandshakeState>;
  restart(): Promise<HandshakeState>;
  call(command: Record<string, unknown>): Promise<BridgeResponse<unknown>>;
  listExamples(): Promise<ExampleSummary[]>;
  readExample(fileName: string): Promise<string>;
}

const tauriTransport: BridgeTransport = {
  handshake: () => invoke<HandshakeState>("bridge_handshake"),
  restart: () => invoke<HandshakeState>("bridge_restart"),
  call: (command) => invoke<BridgeResponse<unknown>>("bridge_call", { command }),
  listExamples: () => invoke<ExampleSummary[]>("list_examples"),
  readExample: (fileName) => invoke<string>("read_example", { fileName }),
};

export class BackendClient {
  private readonly transport: BridgeTransport;
  private currentSessionId: string | null = null;

  constructor(transport: BridgeTransport = tauriTransport) {
    this.transport = transport;
  }

  get sessionId(): string | null {
    return this.currentSessionId;
  }

  async handshake(): Promise<HandshakeState> {
    const state = await this.transport.handshake();
    this.currentSessionId = state.session_id;
    return state;
  }

  async restart(): Promise<HandshakeState> {
    const state = await this.transport.restart();
    this.currentSessionId = state.session_id;
    return state;
  }

  async listExamples(): Promise<ExampleSummary[]> {
    return this.transport.listExamples();
  }

  async readExample(fileName: string): Promise<string> {
    return this.transport.readExample(fileName);
  }

  private async request<T>(command: Record<string, unknown>): Promise<T> {
    const response = await this.transport.call(command);
    if (response.session_id) {
      if (this.currentSessionId && response.session_id !== this.currentSessionId) {
        throw new Error(
          "BACKEND_SESSION_LOST:后端会话已变更，请点击底部「重启工作区」重新建立连接。"
        );
      }
      this.currentSessionId = response.session_id;
    }
    if (response.status === "error") {
      throw new BackendError(response.error);
    }
    return response.data as T;
  }

  openDocumentInput(input: string): Promise<DocumentInfo> {
    return this.request<DocumentInfo>({ type: "document.open", input });
  }

  openDocumentPath(path: string): Promise<DocumentInfo> {
    return this.request<DocumentInfo>({ type: "document.open", path });
  }

  getDocument(documentId: DecimalU64): Promise<DocumentData> {
    return this.request<DocumentData>({
      type: "document.get",
      document_id: documentId,
    });
  }

  getDocumentInfo(documentId: DecimalU64): Promise<DocumentInfo> {
    return this.request<DocumentInfo>({
      type: "document.info",
      document_id: documentId,
    });
  }

  replaceExperiment(
    documentId: DecimalU64,
    expectedRevision: DecimalU64,
    input: string
  ): Promise<DocumentInfo> {
    return this.request<DocumentInfo>({
      type: "document.replace_experiment",
      document_id: documentId,
      expected_revision: expectedRevision,
      input,
    });
  }

  replacePresentation(
    documentId: DecimalU64,
    expectedRevision: DecimalU64,
    presentation: string | null
  ): Promise<DocumentInfo> {
    return this.request<DocumentInfo>({
      type: "document.replace_presentation",
      document_id: documentId,
      expected_revision: expectedRevision,
      presentation,
    });
  }

  validateDocument(
    documentId: DecimalU64,
    expectedRevision: DecimalU64
  ): Promise<DocumentValidation> {
    return this.request<DocumentValidation>({
      type: "document.validate",
      document_id: documentId,
      expected_revision: expectedRevision,
    });
  }

  effectiveSettings(
    documentId: DecimalU64,
    expectedRevision: DecimalU64,
    offset = 0,
    count = 64
  ): Promise<EffectiveSettingsPage> {
    return this.request<EffectiveSettingsPage>({
      type: "document.effective_settings",
      document_id: documentId,
      expected_revision: expectedRevision,
      offset,
      count,
    });
  }

  saveDocument(
    documentId: DecimalU64,
    expectedRevision: DecimalU64,
    path: string
  ): Promise<SaveResult> {
    return this.request<SaveResult>({
      type: "document.save",
      document_id: documentId,
      expected_revision: expectedRevision,
      path,
    });
  }

  saveAndRerun(
    documentId: DecimalU64,
    expectedRevision: DecimalU64,
    path: string,
    mode: RunMode = "automatic"
  ): Promise<SaveAndRerunResult> {
    return this.request<SaveAndRerunResult>({
      type: "document.save_and_rerun",
      document_id: documentId,
      expected_revision: expectedRevision,
      path,
      mode,
    });
  }

  closeDocument(
    documentId: DecimalU64,
    expectedRevision: DecimalU64,
    discardUnsaved = false
  ): Promise<{ document_id: DecimalU64; closed: boolean }> {
    return this.request({
      type: "document.close",
      document_id: documentId,
      expected_revision: expectedRevision,
      discard_unsaved: discardUnsaved,
    });
  }

  submitRun(
    documentId: DecimalU64,
    expectedRevision: DecimalU64,
    mode: RunMode = "automatic"
  ): Promise<SubmittedRun> {
    return this.request<SubmittedRun>({
      type: "run.submit",
      document_id: documentId,
      expected_revision: expectedRevision,
      mode,
    });
  }

  runSummary(runId: DecimalU64): Promise<SummaryData> {
    return this.request<SummaryData>({
      type: "run.summary",
      run_id: runId,
    });
  }

  controlRun(
    runId: DecimalU64,
    control: RunControl
  ): Promise<{ run_id: DecimalU64; control: RunControl; accepted: boolean }> {
    return this.request({
      type: "run.control",
      run_id: runId,
      control,
    });
  }

  runProbes(
    runId: DecimalU64,
    offset = 0,
    count = 64
  ): Promise<RunPageResponse<ProbeRow>> {
    return this.request({
      type: "run.probes",
      run_id: runId,
      offset,
      count,
    });
  }

  runEvidence(
    runId: DecimalU64,
    offset = 0,
    count = 128
  ): Promise<RunPageResponse<EvidenceItem>> {
    return this.request({
      type: "run.evidence",
      run_id: runId,
      offset,
      count,
    });
  }

  runLinks(
    runId: DecimalU64,
    offset = 0,
    count = 64
  ): Promise<RunPageResponse<LinkEvent>> {
    return this.request({
      type: "run.links",
      run_id: runId,
      offset,
      count,
    });
  }

  runRoutes(
    runId: DecimalU64,
    offset = 0,
    count = 64
  ): Promise<RunPageResponse<RoutingEvent>> {
    return this.request({
      type: "run.routes",
      run_id: runId,
      offset,
      count,
    });
  }

  runStp(
    runId: DecimalU64,
    offset = 0,
    count = 64
  ): Promise<RunPageResponse<StpEventSummary>> {
    return this.request({
      type: "run.stp",
      run_id: runId,
      offset,
      count,
    });
  }

  runStpBridges(
    runId: DecimalU64,
    eventId: DecimalU64,
    offset = 0,
    count = 128
  ): Promise<StpEventPageResponse<StpBridgeState>> {
    return this.request({
      type: "run.stp_bridges",
      run_id: runId,
      event_id: eventId,
      offset,
      count,
    });
  }

  runStpLinks(
    runId: DecimalU64,
    eventId: DecimalU64,
    offset = 0,
    count = 128
  ): Promise<StpEventPageResponse<StpLinkState>> {
    return this.request({
      type: "run.stp_links",
      run_id: runId,
      event_id: eventId,
      offset,
      count,
    });
  }

  runTrace(
    runId: DecimalU64,
    offset = 0,
    count = 128
  ): Promise<RunTraceResponse> {
    return this.request({
      type: "run.trace",
      run_id: runId,
      offset,
      count,
    });
  }

  runImportGaps(
    runId: DecimalU64,
    reference: CoverageRef | null = null,
    offset = 0,
    count = 64
  ): Promise<RunImportGapsResponse> {
    return this.request({
      type: "run.import_gaps",
      run_id: runId,
      reference,
      offset,
      count,
    });
  }

  compareRuns(
    baseline: DecimalU64,
    candidate: DecimalU64,
    offset = 0,
    count = 64
  ): Promise<ComparisonData> {
    return this.request({
      type: "run.compare",
      baseline,
      candidate,
      offset,
      count,
    });
  }
}

export const backendClient = new BackendClient();
