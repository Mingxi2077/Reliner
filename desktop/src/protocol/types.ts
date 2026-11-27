// Exact TypeScript protocol types matching Reliner backend 0.4.1
// (src/bridge.rs, src/report.rs, src/workspace.rs, src/validation.rs, src/effective.rs, src/compare.rs)

export type DecimalU64 = string;

export type ErrorCode =
  | "invalid_request"
  | "unsupported_protocol"
  | "handshake_required"
  | "session_mismatch"
  | "invalid_page"
  | "invalid_config"
  | "document_too_large"
  | "document_budget"
  | "document_slots"
  | "unknown_document"
  | "stale_revision"
  | "unsaved_document"
  | "invalid_document"
  | "path_in_use"
  | "save_conflict"
  | "queue_full"
  | "input_budget"
  | "unknown_run"
  | "unknown_stp_event"
  | "unknown_import_gap"
  | "invalid_coverage_reference"
  | "report_unavailable"
  | "invalid_control"
  | "io_error"
  | "json_error"
  | "input_error"
  | "import_error"
  | "lldp_error"
  | "unsupported_archive_version"
  | "unsupported_report_version"
  | "archive_too_large"
  | "invalid_archive"
  | "serialization_error";

export interface BridgeError {
  code: ErrorCode;
  message: string;
  details: Record<string, unknown>;
}

export type BridgeResponse<T = unknown> =
  | {
      protocol_version: number;
      request_id: DecimalU64 | null;
      session_id: string;
      status: "ok";
      data: T;
    }
  | {
      protocol_version: number;
      request_id: DecimalU64 | null;
      session_id: string;
      status: "error";
      error: BridgeError;
    };

export interface RunLimits {
  event_work: number;
  total_work: number;
  pending_events: number;
  state_entries: number;
  essential_facts: number;
  trace_entries: number;
}

export interface HelloData {
  protocol_version: number;
  backend_api_version: number;
  crate_version: string;
  session_id: string;
  native_formats: number[];
  report_versions: number[];
  commands: string[];
  max_page_size: number;
  limits: {
    max_documents: number;
    bytes_per_document: number;
    document_bytes: number;
    queue_slots: number;
    resident_runs: number;
    input_bytes: number;
    history_runs: number;
    history_trace_entries: number;
    run_limits: RunLimits;
  };
}

export interface HandshakeState {
  session_id: string;
  backend_exe: string;
  hello: HelloData;
}

export interface ExampleSummary {
  file_name: string;
  id: string;
  format_version: number;
  revision: string;
  title: string;
  summary: string;
  device_count: number;
  link_count: number;
  probe_count: number;
  path: string;
}

export interface DocumentInfo {
  id: DecimalU64;
  topology_id: string;
  format_version: number;
  native_revision: string;
  revision: DecimalU64;
  effective_revision: DecimalU64;
  saved_revision: DecimalU64 | null;
  path: string | null;
  dirty: boolean;
}

export interface DocumentData {
  info: DocumentInfo;
  experiment: string;
  presentation: string | null;
}

export interface InputLocation {
  path: string;
  device_id?: string;
  port_id?: string;
  link_id?: string;
  probe_id?: string;
}

export interface InputError {
  code: string;
  message: string;
  location: InputLocation;
  line?: number;
  column?: number;
}

export interface ValidationSummary {
  topology_id: string;
  revision: string;
  format_version: number;
  devices: number;
  ports: number;
  links: number;
  probes: number;
}

export type DocumentValidation = {
  document_id: DecimalU64;
  revision: DecimalU64;
  effective_revision: DecimalU64;
} & (
  | { status: "valid"; summary: ValidationSummary }
  | { status: "invalid"; error: InputError }
);

export type SettingSource = "explicit" | "model_default";

export interface EffectiveSetting {
  key: string;
  location: InputLocation;
  value: unknown;
  source: SettingSource;
  configurable: boolean;
}

export interface EffectiveSettingsPage {
  document_id: DecimalU64;
  revision: DecimalU64;
  effective_revision: DecimalU64;
  entries: EffectiveSetting[];
  next_offset: number | null;
}

export interface Page<T> {
  entries: T[];
  next_offset: number | null;
}

export type RunMode = "automatic" | "paused";
export type RunControl = "pause" | "resume" | "step" | "cancel";

export type RunStatus =
  | "queued"
  | "compiling"
  | "ready"
  | "running"
  | "paused"
  | "finished"
  | "incomplete"
  | "cancelled"
  | "failed";

export interface AcceptedRun {
  id: DecimalU64;
  document_id: DecimalU64;
  effective_revision: DecimalU64;
}

export interface BridgeRunOrigin {
  run_id: DecimalU64;
  document_id: DecimalU64;
  document_revision: DecimalU64;
  effective_revision: DecimalU64;
  topology_id: string;
  native_revision: string;
}

export interface SubmittedRun {
  run: AcceptedRun;
  origin: BridgeRunOrigin;
}

export type RerunResult =
  | { status: "accepted"; run: AcceptedRun; origin: BridgeRunOrigin }
  | { status: "rejected"; error: BridgeError };

export interface SaveResult {
  document_id: DecimalU64;
  saved_revision: DecimalU64;
}

export interface SaveAndRerunResult {
  document_id: DecimalU64;
  saved_revision: DecimalU64;
  effective_revision: DecimalU64;
  rerun: RerunResult;
}

export type ResourceLimit =
  | "event_work"
  | "total_work"
  | "pending_events"
  | "state_entries"
  | "essential_facts";

export type StopReason =
  | "time_overflow"
  | "cancelled"
  | { resource_limit: { limit: ResourceLimit } };

export type TraceStatus = "disabled" | "collected" | "truncated" | "evicted";

export interface ProbeCounts {
  completed: number;
  reachable: number;
  timed_out: number;
  rejected_before_send: number;
  incomplete: number;
  undetermined?: number;
}

export interface RunSummary {
  id: DecimalU64;
  document_id: DecimalU64;
  effective_revision: DecimalU64;
  origin: {
    document_id: DecimalU64;
    document_revision: DecimalU64;
    effective_revision: DecimalU64;
    topology_id: string;
    native_revision: string;
  };
  status: RunStatus;
  update: DecimalU64;
  last_event_ns: DecimalU64;
  last_event_id: DecimalU64 | null;
  stop_reason: StopReason | null;
  error: string | null;
  input_error?: InputError;
  trace_status: TraceStatus | null;
  probe_counts?: ProbeCounts;
  stop_limit?: number;
}

export interface SummaryData {
  origin: BridgeRunOrigin;
  summary: RunSummary;
}

export interface Endpoint {
  device_id: string;
  name: string;
  ipv4: string;
}

export type WaitingFor = "arp_reply" | "echo_reply" | "probe_deadline";

export interface CoverageRef {
  source_component: DecimalU64;
  destination_component: DecimalU64;
}

export type ModeledOutcome =
  | { status: "reachable"; at_ns: DecimalU64; evidence_id: DecimalU64 }
  | {
      status: "timeout";
      at_ns: DecimalU64;
      waiting_for: WaitingFor;
      cause: DecimalU64 | null;
    }
  | { status: "incomplete"; at_ns: DecimalU64; reason: StopReason };

export type Outcome =
  | ModeledOutcome
  | { status: "undetermined"; modeled: ModeledOutcome; coverage: CoverageRef };

export interface IcmpIdentity {
  identifier: number;
  sequence: number;
}

export interface ProbeRow {
  id: string;
  source: Endpoint;
  destination: Endpoint;
  deadline_ns: DecimalU64;
  at_ns: DecimalU64;
  ttl: number;
  payload_bytes?: number;
  icmp?: IcmpIdentity;
  outcome: Outcome;
  request_evidence_id: DecimalU64 | null;
}

export interface PortReference {
  device_id: string;
  device_name: string;
  port_id: string;
  port_name: string;
}

export type FrameKind =
  | "arp_request"
  | "arp_reply"
  | "echo_request"
  | "echo_reply";

export interface FrameEvidence {
  id: DecimalU64;
  parent_id: DecimalU64 | null;
  kind: FrameKind;
}

export type PortStage = "ingress" | "egress";
export type Ipv4AclAction = "permit" | "deny";

export interface Ipv4AclMatch {
  rule_number: number;
  network: string;
  prefix_length: number;
}

export type DropRule =
  | { rule: "trunk_vlan_not_allowed"; vlan: number; allowed_vlans: number[] }
  | { rule: "trunk_requires_tag" }
  | { rule: "untagged_port_rejects_tag"; vlan: number }
  | { rule: "access_vlan_mismatch"; vlan: number; access_vlan: number };

export type DynamicRouteMissReason =
  | "awaiting_initial_convergence"
  | "no_advertised_prefix"
  | "no_path_to_advertisement";

export type EvidenceFact =
  | {
      kind: "ipv4_acl_decision";
      event_id: DecimalU64;
      direction: PortStage;
      source: string;
      destination: string;
      action: Ipv4AclAction;
      matched_rule: Ipv4AclMatch | null;
    }
  | { kind: "discarded"; stage: PortStage; reason: DropRule }
  | { kind: "arp_resolved" }
  | { kind: "echo_reply_received" }
  | { kind: "echo_request_received" }
  | {
      kind: "ipv4_route_miss";
      destination: string;
      request_evidence_id: DecimalU64 | null;
    }
  | {
      kind: "dynamic_route_miss";
      destination: string;
      request_evidence_id: DecimalU64 | null;
      reason: DynamicRouteMissReason;
      convergence_event_id: DecimalU64 | null;
    }
  | { kind: "ipv4_ttl_expired"; received_ttl: number }
  | {
      kind: "ipv4_mtu_exceeded";
      event_id: DecimalU64;
      packet_kind: FrameKind;
      ipv4_total_bytes: number;
      mtu_bytes: number;
    }
  | { kind: "ipv4_neighbor_not_unicast"; next_hop: string }
  | {
      kind: "in_flight_link_loss";
      link_id: string;
      planned_arrival_ns: DecimalU64;
      sent_generation: DecimalU64;
      invalidated_by: DecimalU64;
    }
  | { kind: "link_down"; link_id: string; down_event_id: DecimalU64 }
  | {
      kind: "transmit_queue_full";
      event_id: DecimalU64;
      link_id: string;
      rate_bps: DecimalU64;
      queue_frames: number;
      frame_bytes: number;
    }
  | {
      kind: "in_flight_protection_loss";
      link_id: string;
      invalidated_at_ns: DecimalU64;
      planned_arrival_ns: DecimalU64;
      sent_generation: DecimalU64;
      invalidated_by: DecimalU64;
    }
  | {
      kind: "stp_awaiting_convergence";
      link_id: string;
      up_event_id: DecimalU64;
    }
  | {
      kind: "in_flight_stp_loss";
      link_id: string;
      invalidated_at_ns: DecimalU64;
      planned_arrival_ns: DecimalU64;
      sent_generation: DecimalU64;
      invalidated_by: DecimalU64;
    }
  | {
      kind: "protection_unavailable";
      primary_link_id: string;
      primary_down_event_id: DecimalU64;
      standby_link_id: string;
      standby_down_event_id: DecimalU64;
    };

export interface ProtectedLinkEvidence {
  link_id: string;
  selected_by: DecimalU64 | null;
}

export interface EvidenceItem {
  probe_id: string | null;
  id: DecimalU64;
  at_ns: DecimalU64;
  port: PortReference;
  frame: FrameEvidence;
  fact: EvidenceFact;
  protected_link?: ProtectedLinkEvidence;
  stp_event_id?: DecimalU64;
  routing_event_id?: DecimalU64;
  model_only?: boolean;
}

export type LinkStatus = "up" | "down";

export interface LinkEvent {
  event_id: DecimalU64;
  link_id: string;
  at_ns: DecimalU64;
  state: LinkStatus;
  generation: DecimalU64;
  simultaneous?: {
    link_id: string;
    state: LinkStatus;
    generation: DecimalU64;
  };
  protection?: {
    primary_link_id: string;
    standby_link_id: string;
    previous_active_link_id: string | null;
    active_link_id: string | null;
  };
}

export interface RoutingEvent {
  event_id: DecimalU64;
  at_ns: DecimalU64;
  router_id: string;
  triggered_by_link_event_id: DecimalU64 | null;
  installed_routes: number;
}

export type StpPortRole = "root" | "designated" | "alternate" | "disabled";

export interface StpEventSummary {
  event_id: DecimalU64;
  at_ns: DecimalU64;
  triggered_by_link_event_id: DecimalU64 | null;
  bridge_count: number;
  link_count: number;
}

export interface StpBridgeState {
  bridge_id: string;
  priority: number;
  root_bridge_id: string;
  root_path_cost: DecimalU64;
  root_port_id: string | null;
}

export interface StpPortState {
  port_id: string;
  path_cost: number;
  role: StpPortRole;
}

export interface StpLinkState {
  link_id: string;
  ports: [StpPortState, StpPortState];
}

export type TraceEvent =
  | "link_change"
  | "routing_convergence"
  | "stp_convergence"
  | "table_expiry"
  | "probe_deadline"
  | "transmit_start"
  | "transmit_cancelled"
  | "frame_arrival"
  | "probe_start"
  | "neighbor_warmup";

export interface TraceEntry {
  event_id: DecimalU64;
  at_ns: DecimalU64;
  event: TraceEvent;
  frame: FrameEvidence | null;
  port: PortReference | null;
  probe_id: string | null;
  learning:
    | { kind: "arp"; neighbor: string }
    | { kind: "mac"; vlan: number }
    | null;
  route: {
    egress_port_id: string;
    destination: string;
    next_hop: string;
    network: string | null;
    prefix_length: number | null;
    ipv4_mtu?: {
      packet_kind: FrameKind;
      ipv4_total_bytes: number;
      mtu_bytes: number;
      passed: boolean;
    };
    learned?: {
      origin_router_id: string;
      cost: DecimalU64;
      convergence_event_id: DecimalU64;
    };
  } | null;
}

export interface RunPageResponse<T> {
  origin: BridgeRunOrigin;
  entries: T[];
  next_offset: number | null;
}

export interface RunTraceResponse {
  origin: BridgeRunOrigin;
  status: TraceStatus | null;
  entries: TraceEntry[];
  next_offset: number | null;
}

export interface StpEventPageResponse<T> {
  origin: BridgeRunOrigin;
  event_id: DecimalU64;
  entries: T[];
  next_offset: number | null;
}

export interface ComparedRun {
  run: AcceptedRun;
  topology_id: string;
  revision: string;
  model_version: number;
}

export interface ComparedProbe {
  source: Endpoint;
  destination: Endpoint;
  deadline_ns: DecimalU64;
  at_ns: DecimalU64;
  ttl: number;
  payload_bytes?: number;
  outcome: Outcome;
  request_evidence_id: DecimalU64 | null;
}

export interface ProbeComparison {
  probe_id: string;
  baseline: ComparedProbe | null;
  candidate: ComparedProbe | null;
  same_probe_parameters: boolean;
}

export interface ComparisonData {
  baseline_origin: BridgeRunOrigin;
  candidate_origin: BridgeRunOrigin;
  baseline: ComparedRun;
  candidate: ComparedRun;
  entries: ProbeComparison[];
  next_offset: number | null;
}

export type ImportScope =
  | { kind: "all" }
  | { kind: "device"; device_id: string }
  | { kind: "port"; port_id: string }
  | { kind: "link"; link_id: string };

export interface ImportGapSummary {
  id: string;
  source_path: string;
  scope: ImportScope;
  message: string;
}

export interface RunImportGapsResponse {
  origin: BridgeRunOrigin;
  reference: CoverageRef | null;
  entries: ImportGapSummary[];
  next_offset: number | null;
}
