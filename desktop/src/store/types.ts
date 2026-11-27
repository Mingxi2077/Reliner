import type {
  BridgeRunOrigin,
  ComparisonData,
  DocumentInfo,
  DocumentValidation,
  EffectiveSetting,
  EvidenceItem,
  ImportGapSummary,
  LinkEvent,
  ProbeRow,
  RoutingEvent,
  RunSummary,
  StpBridgeState,
  StpEventSummary,
  StpLinkState,
  TraceEntry,
  TraceStatus,
} from "../protocol/types";
import type { NativeLabDocument, PresentationData } from "../model/document";
import type { SelectionTarget } from "../canvas/TopologyCanvas";
import type { RightPanelTab } from "../components/RightPanel";

export interface RunRecord {
  sessionId: string;
  runId: string;
  origin: BridgeRunOrigin;
  summary: RunSummary | null;
  evicted: boolean;
  probes: ProbeRow[];
  probesNextOffset: number | null;
  evidence: EvidenceItem[];
  evidenceNextOffset: number | null;
  linkEvents: LinkEvent[];
  linkEventsNextOffset: number | null;
  routingEvents: RoutingEvent[];
  routingEventsNextOffset: number | null;
  stpSummaries: StpEventSummary[];
  stpDetailsByEventId: Record<
    string,
    { bridges: StpBridgeState[]; links: StpLinkState[] }
  >;
  trace: {
    status: TraceStatus | null;
    entries: TraceEntry[];
    nextOffset: number | null;
  } | null;
  importGaps: ImportGapSummary[] | null;
}

export interface DocumentTabState {
  documentId: string;
  info: DocumentInfo;
  doc: NativeLabDocument;
  presentation: PresentationData;
  validation: DocumentValidation | null;
  effectiveSettings: EffectiveSetting[] | null;
  selection: SelectionTarget;
  rightTab: RightPanelTab;
  runs: RunRecord[];
  activeRunId: string | null;
  selectedProbeId: string | null;
  selectedStpEventId: string | null;
  compareBaselineId: string;
  compareCandidateId: string;
  comparison: ComparisonData | null;
  compareLoading: boolean;
  compareError: string | null;
}

export interface BannerNotice {
  kind: "info" | "ok" | "warn" | "error";
  text: string;
  actionLabel?: string;
  onAction?: () => void;
  secondaryActionLabel?: string;
  onSecondaryAction?: () => void;
}
