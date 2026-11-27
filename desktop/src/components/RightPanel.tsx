import React, { useEffect, useMemo, useState } from "react";
import {
  Activity,
  AlertCircle,
  AlertTriangle,
  CheckCircle2,
  ChevronDown,
  ChevronRight,
  ExternalLink,
  GitCompare,
  HelpCircle,
  Layers,
  Maximize2,
  Minimize2,
  Settings,
  Sliders,
  X,
} from "lucide-react";
import type { NativeLabDocument } from "../model/document";
import type {
  ComparisonData,
  DocumentValidation,
  EffectiveSetting,
  EvidenceItem,
  StpEventSummary,
} from "../protocol/types";
import {
  diagnoseProbe,
  formatEvidenceSummary,
  stopReasonText,
  traceEventLabel,
} from "../protocol/diagnosis";
import type { SelectionTarget } from "../canvas/TopologyCanvas";
import type { RunRecord } from "../store/types";
import { DeviceForm } from "./forms/DeviceForm";
import { LinkForm } from "./forms/LinkForm";
import { ScenarioForm } from "./forms/ScenarioForm";
import { CompareView } from "./CompareView";

export type RightPanelTab = "result" | "config" | "compare";

interface RightPanelProps {
  width?: number;
  isWide?: boolean;
  floating?: boolean;
  onStartResize?: (startX: number) => void;
  onToggleWide?: () => void;
  onCloseDrawer?: () => void;
  docKey: string;
  doc: NativeLabDocument;
  activeTab: RightPanelTab;
  onChangeTab: (tab: RightPanelTab) => void;
  selection: SelectionTarget;
  onSelect: (target: SelectionTarget) => void;
  onChangeDoc: (next: NativeLabDocument) => void;
  onDeleteDevice: (deviceId: string) => void;
  onDeleteLink: (linkId: string) => void;
  onSaveAndRerun?: () => void;
  rerunLabel?: string;
  validation: DocumentValidation | null;
  effectiveSettings: EffectiveSetting[] | null;
  onLoadEffectiveSettings: () => void;
  runs: RunRecord[];
  activeRun: RunRecord | null;
  selectedProbeId: string | null;
  selectedStpEventId: string | null;
  onSelectRun: (runId: string) => void;
  onSelectProbe: (probeId: string) => void;
  onLoadMoreProbes: (runId: string) => void;
  onLoadMoreEvidence: (runId: string) => void;
  onLoadTrace: (runId: string, reset?: boolean) => void;
  onSelectStpEvent: (runId: string, eventId: string) => void;
  onLoadImportGaps: (runId: string, probeId: string) => void;
  compareBaselineId: string;
  compareCandidateId: string;
  comparison: ComparisonData | null;
  compareLoading: boolean;
  compareError: string | null;
  onChangeCompareBaseline: (runId: string) => void;
  onChangeCompareCandidate: (runId: string) => void;
  onRunCompare: (baselineId: string, candidateId: string) => void;
  onLoadMoreCompare: () => void;
  onInspectRunEvidence: (runId: string, evidenceId: string) => void;
}

const Disclosure: React.FC<{
  title: string;
  badge?: string;
  defaultOpen?: boolean;
  open?: boolean;
  onToggle?: (next: boolean) => void;
  children: React.ReactNode;
}> = ({ title, badge, defaultOpen = false, open, onToggle, children }) => {
  const [internalOpen, setInternalOpen] = useState(defaultOpen);
  const isOpen = open ?? internalOpen;
  const toggle = () => {
    const next = !isOpen;
    setInternalOpen(next);
    onToggle?.(next);
  };
  return (
    <div className="disclosure">
      <button className="disclosure-header" onClick={toggle}>
        <span style={{ display: "inline-flex", alignItems: "center", gap: 5 }}>
          {isOpen ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
          {title}
        </span>
        {badge && <span className="pill pill-neutral">{badge}</span>}
      </button>
      {isOpen && <div className="disclosure-body">{children}</div>}
    </div>
  );
};

function runStatusLabel(status: string): string {
  switch (status) {
    case "queued":
      return "排队中";
    case "compiling":
      return "编译校验中";
    case "ready":
      return "就绪";
    case "running":
      return "运行中";
    case "paused":
      return "已暂停（单步模式）";
    case "finished":
      return "已完成";
    case "incomplete":
      return "未完成（触及边界）";
    case "cancelled":
      return "已取消";
    case "failed":
      return "编译失败";
    default:
      return status;
  }
}

export const RightPanel: React.FC<RightPanelProps> = ({
  width = 448,
  isWide = false,
  floating = false,
  onStartResize,
  onToggleWide,
  onCloseDrawer,
  docKey,
  doc,
  activeTab,
  onChangeTab,
  selection,
  onSelect,
  onChangeDoc,
  onDeleteDevice,
  onDeleteLink,
  onSaveAndRerun,
  rerunLabel = "保存并重跑",
  validation,
  effectiveSettings,
  onLoadEffectiveSettings,
  runs,
  activeRun,
  selectedProbeId,
  selectedStpEventId,
  onSelectRun,
  onSelectProbe,
  onLoadMoreProbes,
  onLoadMoreEvidence,
  onLoadTrace,
  onSelectStpEvent,
  onLoadImportGaps,
  compareBaselineId,
  compareCandidateId,
  comparison,
  compareLoading,
  compareError,
  onChangeCompareBaseline,
  onChangeCompareCandidate,
  onRunCompare,
  onLoadMoreCompare,
  onInspectRunEvidence,
}) => {
  const [showModeledDetail, setShowModeledDetail] = useState(false);
  const [stpDisclosureOpen, setStpDisclosureOpen] = useState(false);
  const [effectiveOpen, setEffectiveOpen] = useState(false);

  // Auto-refresh effective_settings whenever the disclosure stays open across edits
  useEffect(() => {
    if (effectiveOpen && activeTab === "config" && effectiveSettings === null) {
      onLoadEffectiveSettings();
    }
  }, [effectiveOpen, activeTab, effectiveSettings, onLoadEffectiveSettings]);

  const comparableRunsCount = useMemo(
    () =>
      runs.filter(
        (r) =>
          !r.evicted &&
          r.summary &&
          r.summary.probe_counts !== undefined &&
          (r.summary.status === "finished" ||
            r.summary.status === "incomplete" ||
            r.summary.status === "cancelled")
      ).length,
    [runs]
  );

  const evidenceById = useMemo(() => {
    const map = new Map<string, EvidenceItem>();
    for (const item of activeRun?.evidence ?? []) {
      map.set(item.id, item);
    }
    return map;
  }, [activeRun?.evidence]);

  const selectedProbe = useMemo(() => {
    if (!activeRun || activeRun.probes.length === 0) return null;
    return (
      activeRun.probes.find((p) => p.id === selectedProbeId) ??
      activeRun.probes[0]
    );
  }, [activeRun, selectedProbeId]);

  const diagnosis = useMemo(() => {
    if (!selectedProbe || !activeRun) return null;
    return diagnoseProbe(selectedProbe, evidenceById, activeRun.routingEvents);
  }, [selectedProbe, activeRun, evidenceById]);

  const jumpToValidationLocation = () => {
    if (!validation || validation.status !== "invalid") return;
    const loc = validation.error.location;
    if (loc.port_id && loc.device_id) {
      onSelect({ kind: "port", deviceId: loc.device_id, portId: loc.port_id });
    } else if (loc.device_id) {
      onSelect({ kind: "device", deviceId: loc.device_id });
    } else if (loc.link_id) {
      onSelect({ kind: "link", linkId: loc.link_id });
    } else {
      onSelect({ kind: "scenario" });
    }
    onChangeTab("config");
  };

  const jumpToInputErrorLocation = (loc: {
    device_id?: string;
    port_id?: string;
    link_id?: string;
  }) => {
    if (loc.port_id && loc.device_id) {
      onSelect({ kind: "port", deviceId: loc.device_id, portId: loc.port_id });
    } else if (loc.device_id) {
      onSelect({ kind: "device", deviceId: loc.device_id });
    } else if (loc.link_id) {
      onSelect({ kind: "link", linkId: loc.link_id });
    } else {
      onSelect({ kind: "scenario" });
    }
    onChangeTab("config");
  };

  const handleOpenFaultConfig = () => {
    const target = diagnosis?.target;
    if (!target) return;
    if (target.kind === "port" && target.deviceId && target.portId) {
      onSelect({ kind: "port", deviceId: target.deviceId, portId: target.portId });
    } else if (target.kind === "device" && target.deviceId) {
      onSelect({ kind: "device", deviceId: target.deviceId });
    } else if (target.kind === "link" && target.linkId) {
      onSelect({ kind: "link", linkId: target.linkId });
    }
    onChangeTab("config");
  };

  const selectedDevice =
    selection.kind === "device" || selection.kind === "port"
      ? doc.devices.find((d) => d.id === selection.deviceId) ?? null
      : null;
  const selectedLink =
    selection.kind === "link"
      ? doc.links.find((l) => l.id === selection.linkId) ?? null
      : null;

  return (
    <aside
      className={`right-panel ${floating ? "floating-drawer" : ""}`}
      style={{ width }}
    >
      {floating && onStartResize && (
        <div
          className="drawer-resize-grip"
          title="向左或向右拖拽，自由调节编辑面板宽度"
          onMouseDown={(e) => {
            e.preventDefault();
            onStartResize(e.clientX);
          }}
        />
      )}
      <div className="right-tabs">
        <button
          className={`right-tab ${activeTab === "result" ? "active" : ""}`}
          onClick={() => onChangeTab("result")}
        >
          <Activity size={13} /> 排障结果
          {activeRun?.summary?.probe_counts && (
            <span className="pill pill-neutral" style={{ padding: "0 5px", fontSize: 10.5 }}>
              {activeRun.summary.probe_counts.reachable}/
              {activeRun.summary.probe_counts.completed +
                activeRun.summary.probe_counts.incomplete}
            </span>
          )}
        </button>
        <button
          className={`right-tab ${activeTab === "config" ? "active" : ""}`}
          onClick={() => onChangeTab("config")}
        >
          <Settings size={13} /> 属性配置
        </button>
        <button
          className={`right-tab ${activeTab === "compare" ? "active" : ""}`}
          onClick={() => onChangeTab("compare")}
        >
          <GitCompare size={13} /> 对比
        </button>
        <div style={{ marginLeft: "auto", display: "inline-flex", alignItems: "center", gap: 2 }}>
          {onToggleWide && (
            <button
              className="tab-close"
              title={isWide ? "恢复标准宽度" : "加宽编辑面板（便于改路由 / ACL）"}
              onClick={onToggleWide}
            >
              {isWide ? <Minimize2 size={13} /> : <Maximize2 size={13} />}
            </button>
          )}
          {floating && onCloseDrawer && (
            <button
              className="tab-close"
              title="收起抽屉查看全屏画布"
              onClick={onCloseDrawer}
            >
              <X size={15} />
            </button>
          )}
        </div>
      </div>

      <div className="panel-scroll">
        {/* Validation Error Banner (always visible if invalid) */}
        {validation && validation.status === "invalid" && (
          <div className="card fault">
            <div
              style={{
                display: "flex",
                alignItems: "center",
                gap: 6,
                color: "var(--status-fault)",
                fontWeight: 700,
                marginBottom: 4,
              }}
            >
              <AlertCircle size={15} /> 配置校验未通过 ({validation.error.code})
            </div>
            <div className="small" style={{ marginBottom: 6 }}>
              {validation.error.message}
            </div>
            <div className="small mono muted" style={{ marginBottom: 8 }}>
              路径：{validation.error.location.path || "/"}
              {validation.error.location.device_id &&
                ` · 设备 ${validation.error.location.device_id}`}
              {validation.error.location.port_id &&
                ` · 端口 ${validation.error.location.port_id}`}
              {validation.error.location.link_id &&
                ` · 链路 ${validation.error.location.link_id}`}
            </div>
            <button className="btn btn-xs btn-primary" onClick={jumpToValidationLocation}>
              <ExternalLink size={11} /> 定位到对应配置
            </button>
          </div>
        )}

        {/* TAB 1: RESULT & TROUBLESHOOTING */}
        {activeTab === "result" && (
          <>
            {!activeRun ? (
              <div className="card">
                <div style={{ fontWeight: 700, marginBottom: 6 }}>
                  尚未执行连通性探测
                </div>
                <div className="small muted">
                  点击顶部工具栏的「运行实验」执行当前拓扑的全部探测，或点「测连通」在画布上选择源主机和目的主机；也可点「单步」按事件边界逐步推进。
                </div>
              </div>
            ) : (
              <>
                {/* Run switcher & status header */}
                <div className="card">
                  <div
                    style={{
                      display: "flex",
                      justifyContent: "space-between",
                      alignItems: "center",
                      gap: 6,
                      marginBottom: 8,
                    }}
                  >
                    <select
                      style={{
                        flex: 1,
                        minWidth: 0,
                        height: 28,
                        borderRadius: 5,
                        border: "1px solid var(--border-strong)",
                        fontSize: 12,
                      }}
                      value={activeRun.runId}
                      onChange={(e) => onSelectRun(e.target.value)}
                    >
                      {runs.map((r) => (
                        <option key={r.runId} value={r.runId}>
                          运行 #{r.runId} · 原生修订 {r.origin.native_revision}（生效{" "}
                          {r.origin.effective_revision}）
                          {r.evicted ? " [已淘汰]" : ""}
                        </option>
                      ))}
                    </select>
                    {comparableRunsCount >= 2 && (
                      <button
                        className="btn btn-xs"
                        onClick={() => onChangeTab("compare")}
                      >
                        <GitCompare size={11} /> 对比
                      </button>
                    )}
                  </div>

                  {activeRun.evicted ? (
                    <div className="small" style={{ color: "var(--status-warn)" }}>
                      该运行已从工作区历史池中淘汰（后端返回 <code className="mono">unknown_run</code>），不可当作空报告。
                    </div>
                  ) : (
                    activeRun.summary && (
                      <div className="small">
                        <div
                          style={{
                            display: "flex",
                            justifyContent: "space-between",
                            alignItems: "center",
                            marginBottom: 4,
                          }}
                        >
                          <span className="pill pill-neutral">
                            {runStatusLabel(activeRun.summary.status)}
                          </span>
                          <span className="mono muted">
                            {activeRun.summary.last_event_id
                              ? `事件 #${activeRun.summary.last_event_id} · ${activeRun.summary.last_event_ns} ns`
                              : "尚未提交事件"}
                          </span>
                        </div>

                        {activeRun.summary.probe_counts && (
                          <div
                            style={{
                              display: "flex",
                              flexWrap: "wrap",
                              gap: 6,
                              marginTop: 6,
                            }}
                          >
                            <span className="pill pill-ok">
                              连通 {activeRun.summary.probe_counts.reachable}
                            </span>
                            {activeRun.summary.probe_counts.timed_out > 0 && (
                              <span className="pill pill-fault">
                                超时 {activeRun.summary.probe_counts.timed_out}
                              </span>
                            )}
                            {activeRun.summary.probe_counts.rejected_before_send > 0 && (
                              <span className="pill pill-fault">
                                发包前拒绝{" "}
                                {activeRun.summary.probe_counts.rejected_before_send}
                              </span>
                            )}
                            {activeRun.summary.probe_counts.incomplete > 0 && (
                              <span className="pill pill-warn">
                                未完成 {activeRun.summary.probe_counts.incomplete}
                              </span>
                            )}
                            {(activeRun.summary.probe_counts.undetermined ?? 0) > 0 && (
                              <span className="pill pill-warn">
                                未判定 {activeRun.summary.probe_counts.undetermined}
                              </span>
                            )}
                          </div>
                        )}

                        {activeRun.summary.stop_reason && (
                          <div
                            style={{
                              marginTop: 6,
                              padding: "6px 8px",
                              borderRadius: 6,
                              background: "var(--status-warn-bg)",
                              border: "1px solid var(--status-warn-border)",
                              color: "var(--status-warn)",
                            }}
                          >
                            停止原因：{stopReasonText(activeRun.summary.stop_reason)}
                            {activeRun.summary.stop_limit !== undefined &&
                              `（额度 ${activeRun.summary.stop_limit}）`}
                          </div>
                        )}

                        {activeRun.summary.status === "failed" && (
                          <div
                            style={{
                              marginTop: 6,
                              padding: "8px",
                              borderRadius: 6,
                              background: "var(--status-fault-bg)",
                              border: "1px solid var(--status-fault-border)",
                              color: "var(--status-fault)",
                            }}
                          >
                            <div style={{ fontWeight: 700 }}>
                              后台编译失败
                              {activeRun.summary.input_error
                                ? ` (${activeRun.summary.input_error.code})`
                                : ""}
                            </div>
                            <div style={{ marginTop: 2 }}>
                              {activeRun.summary.input_error?.message ??
                                activeRun.summary.error}
                            </div>
                            {activeRun.summary.input_error && (
                              <>
                                <div className="small mono muted" style={{ marginTop: 4 }}>
                                  路径：{activeRun.summary.input_error.location.path || "/"}
                                  {activeRun.summary.input_error.location.device_id &&
                                    ` · 设备 ${activeRun.summary.input_error.location.device_id}`}
                                  {activeRun.summary.input_error.location.port_id &&
                                    ` · 端口 ${activeRun.summary.input_error.location.port_id}`}
                                  {activeRun.summary.input_error.location.link_id &&
                                    ` · 链路 ${activeRun.summary.input_error.location.link_id}`}
                                  {activeRun.summary.input_error.location.probe_id &&
                                    ` · 探测 ${activeRun.summary.input_error.location.probe_id}`}
                                </div>
                                <button
                                  className="btn btn-xs btn-primary"
                                  style={{ marginTop: 6 }}
                                  onClick={() =>
                                    jumpToInputErrorLocation(
                                      activeRun.summary!.input_error!.location
                                    )
                                  }
                                >
                                  定位到配置
                                </button>
                              </>
                            )}
                          </div>
                        )}
                      </div>
                    )
                  )}
                </div>

                {/* Multi-probe selector */}
                {activeRun.probes.length > 1 && (
                  <div className="card" style={{ padding: 8 }}>
                    <div className="panel-section-title" style={{ marginBottom: 6 }}>
                      <span>探测结果列表 ({activeRun.probes.length})</span>
                    </div>
                    <div style={{ display: "flex", flexDirection: "column", gap: 4 }}>
                      {activeRun.probes.map((probe) => {
                        const isSel = selectedProbe?.id === probe.id;
                        const status = probe.outcome.status;
                        return (
                          <button
                            key={probe.id}
                            className={`list-item ${isSel ? "selected" : ""}`}
                            onClick={() => {
                              onSelectProbe(probe.id);
                              setShowModeledDetail(false);
                            }}
                          >
                            <span
                              style={{
                                display: "inline-flex",
                                alignItems: "center",
                                gap: 5,
                              }}
                            >
                              {status === "reachable" && (
                                <CheckCircle2 size={13} color="#15803d" />
                              )}
                              {status === "timeout" && (
                                <AlertTriangle size={13} color="#dc2626" />
                              )}
                              {(status === "incomplete" ||
                                status === "undetermined") && (
                                <HelpCircle size={13} color="#d97706" />
                              )}
                              <span className="mono">{probe.id}</span>
                              {probe.payload_bytes !== undefined && (
                                <span className="small muted">
                                  ({probe.payload_bytes}B)
                                </span>
                              )}
                            </span>
                            <span className="small muted">
                              {status === "reachable"
                                ? `连通 · ${probe.outcome.at_ns}ns`
                                : status === "timeout"
                                  ? probe.outcome.cause
                                    ? "超时·已定位"
                                    : "超时·未定位"
                                  : status === "undetermined"
                                    ? "未判定"
                                    : "未完成"}
                            </span>
                          </button>
                        );
                      })}
                    </div>
                    {activeRun.probesNextOffset !== null && (
                      <button
                        className="btn btn-xs"
                        style={{ width: "100%", marginTop: 6 }}
                        onClick={() => onLoadMoreProbes(activeRun.runId)}
                      >
                        加载更多探测...
                      </button>
                    )}
                  </div>
                )}

                {/* FIRST-SCREEN DIAGNOSIS CARD */}
                {selectedProbe && diagnosis && (
                  <div
                    className={`card ${
                      diagnosis.severity === "ok"
                        ? "ok"
                        : diagnosis.severity === "fault"
                          ? "fault"
                          : "warn"
                    }`}
                  >
                    <div
                      style={{
                        display: "flex",
                        justifyContent: "space-between",
                        alignItems: "center",
                        marginBottom: 8,
                      }}
                    >
                      <span
                        className={`pill ${
                          diagnosis.severity === "ok"
                            ? "pill-ok"
                            : diagnosis.severity === "fault"
                              ? "pill-fault"
                              : "pill-warn"
                        }`}
                      >
                        {diagnosis.severity === "ok" && <CheckCircle2 size={12} />}
                        {diagnosis.severity === "fault" && (
                          <AlertTriangle size={12} />
                        )}
                        {(diagnosis.severity === "undetermined" ||
                          diagnosis.severity === "unlocated" ||
                          diagnosis.severity === "incomplete") && (
                          <HelpCircle size={12} />
                        )}
                        {diagnosis.badgeText}
                      </span>
                      <span className="small mono muted">
                        探测 {selectedProbe.id}
                      </span>
                    </div>

                    <div style={{ fontWeight: 700, marginBottom: 4 }}>
                      {selectedProbe.source.name} ({selectedProbe.source.ipv4}) →{" "}
                      {selectedProbe.destination.name} (
                      {selectedProbe.destination.ipv4})
                    </div>
                    <div className="small" style={{ marginBottom: 6 }}>
                      {diagnosis.headline}
                    </div>

                    {diagnosis.requestNote && (
                      <div
                        className="small"
                        style={{
                          padding: "5px 8px",
                          borderRadius: 5,
                          background: "#eff6ff",
                          color: "#1e40af",
                          marginBottom: 6,
                        }}
                      >
                        {diagnosis.requestNote}
                      </div>
                    )}

                    <div className="highlighter-bar">
                      {diagnosis.explanation}
                    </div>

                    <div className="small" style={{ marginBottom: 8 }}>
                      {diagnosis.protectedLinkNote && (
                        <div className="muted" style={{ marginBottom: 3 }}>
                          {diagnosis.protectedLinkNote}
                        </div>
                      )}
                      {diagnosis.routingBasis && (
                        <div className="muted" style={{ marginBottom: 3 }}>
                          {diagnosis.routingBasis}
                        </div>
                      )}
                      {diagnosis.causeEvidence && (
                        <div className="mono muted">
                          运行 #{activeRun.runId} · 证据 #{diagnosis.causeEvidence.id} · 帧 #
                          {diagnosis.causeEvidence.frame.id}
                          {diagnosis.causeEvidence.frame.parent_id
                            ? ` (父帧 #${diagnosis.causeEvidence.frame.parent_id})`
                            : ""}
                        </div>
                      )}
                    </div>

                    {/* Undetermined: never promote modeled.reachable to green; collapse behind toggle */}
                    {diagnosis.severity === "undetermined" && (
                      <div style={{ marginBottom: 8 }}>
                        <div style={{ display: "flex", gap: 6, marginBottom: 6 }}>
                          <button
                            className="btn btn-xs"
                            onClick={() => setShowModeledDetail((v) => !v)}
                          >
                            {showModeledDetail ? "收起模型推演结果" : "展开查看模型推演结果"}
                          </button>
                          <button
                            className="btn btn-xs"
                            onClick={() =>
                              onLoadImportGaps(activeRun.runId, selectedProbe.id)
                            }
                          >
                            查看影响缺项
                          </button>
                        </div>
                        {showModeledDetail && diagnosis.modeledNote && (
                          <div className="small muted" style={{ padding: 6, background: "#f8fafc", borderRadius: 5 }}>
                            {diagnosis.modeledNote}
                          </div>
                        )}
                      </div>
                    )}

                    {diagnosis.nextStep && (
                      <div className="small" style={{ marginBottom: 8 }}>
                        <strong>下一步建议：</strong>
                        {diagnosis.nextStep}
                      </div>
                    )}

                    <div style={{ display: "flex", flexWrap: "wrap", gap: 6 }}>
                      {diagnosis.target && (
                        <button
                          className="btn btn-sm btn-primary"
                          onClick={handleOpenFaultConfig}
                        >
                          <ExternalLink size={13} /> {diagnosis.target.label}
                        </button>
                      )}
                      {diagnosis.stpEventId && (
                        <button
                          className="btn btn-sm"
                          onClick={() => {
                            setStpDisclosureOpen(true);
                            onSelectStpEvent(activeRun.runId, diagnosis.stpEventId!);
                          }}
                        >
                          <Layers size={13} /> 回查对应 STP 树 (#
                          {diagnosis.stpEventId})
                        </button>
                      )}
                    </div>
                  </div>
                )}

                {/* COLLAPSED BY DEFAULT: On-demand Evidence, Link, Route, STP, Trace */}
                {!activeRun.evicted && activeRun.summary?.status !== "failed" && (
                  <>
                    <Disclosure
                      title="必要证据明细"
                      badge={`${activeRun.evidence.length}`}
                    >
                      {activeRun.evidence.length === 0 ? (
                        <div className="small muted">暂无证据记录。</div>
                      ) : (
                        <>
                          {activeRun.evidence.map((ev) => {
                            const formatted = formatEvidenceSummary(ev);
                            return (
                              <div
                                key={`${activeRun.runId}-${ev.id}`}
                                style={{
                                  padding: "6px 0",
                                  borderBottom: "1px solid var(--border-subtle)",
                                }}
                              >
                                <div
                                  className="small mono muted"
                                  style={{
                                    display: "flex",
                                    justifyContent: "space-between",
                                  }}
                                >
                                  <span>
                                    证据 #{ev.id} · {ev.at_ns} ns
                                    {ev.probe_id ? ` · ${ev.probe_id}` : ""}
                                  </span>
                                  <span>
                                    帧 #{ev.frame.id}
                                    {ev.frame.parent_id ? `←#${ev.frame.parent_id}` : ""}
                                  </span>
                                </div>
                                <div className="small" style={{ marginTop: 2 }}>
                                  {formatted.text}
                                </div>
                              </div>
                            );
                          })}
                          {activeRun.evidenceNextOffset !== null && (
                            <button
                              className="btn btn-xs"
                              style={{ width: "100%", marginTop: 6 }}
                              onClick={() => onLoadMoreEvidence(activeRun.runId)}
                            >
                              加载更多证据...
                            </button>
                          )}
                        </>
                      )}
                    </Disclosure>

                    {activeRun.linkEvents.length > 0 && (
                      <Disclosure
                        title="链路与主备切换事件"
                        badge={`${activeRun.linkEvents.length}`}
                      >
                        <table className="mini-table">
                          <thead>
                            <tr>
                              <th>时刻</th>
                              <th>链路 / 状态</th>
                              <th>主备角色</th>
                            </tr>
                          </thead>
                          <tbody>
                            {activeRun.linkEvents.map((ev) => (
                              <tr key={ev.event_id}>
                                <td className="mono">{ev.at_ns}ns</td>
                                <td className="mono">
                                  {ev.link_id} → {ev.state}
                                  {ev.simultaneous && (
                                    <div>
                                      {ev.simultaneous.link_id} → {ev.simultaneous.state}
                                    </div>
                                  )}
                                </td>
                                <td className="mono">
                                  {ev.protection
                                    ? `${ev.protection.previous_active_link_id ?? "无"} → ${ev.protection.active_link_id ?? "无"}`
                                    : "—"}
                                </td>
                              </tr>
                            ))}
                          </tbody>
                        </table>
                      </Disclosure>
                    )}

                    {activeRun.routingEvents.length > 0 && (
                      <Disclosure
                        title="动态路由表发布事件"
                        badge={`${activeRun.routingEvents.length}`}
                      >
                        <table className="mini-table">
                          <thead>
                            <tr>
                              <th>事件</th>
                              <th>时刻</th>
                              <th>路由器</th>
                              <th>安装条数</th>
                            </tr>
                          </thead>
                          <tbody>
                            {activeRun.routingEvents.map((ev) => (
                              <tr key={ev.event_id}>
                                <td className="mono">#{ev.event_id}</td>
                                <td className="mono">{ev.at_ns}ns</td>
                                <td className="mono">{ev.router_id}</td>
                                <td className="mono">{ev.installed_routes}</td>
                              </tr>
                            ))}
                          </tbody>
                        </table>
                      </Disclosure>
                    )}

                    {activeRun.stpSummaries.length > 0 && (
                      <Disclosure
                        title="STP 选树与端口角色"
                        badge={`${activeRun.stpSummaries.length} 轮`}
                        open={stpDisclosureOpen}
                        onToggle={setStpDisclosureOpen}
                      >
                        <div style={{ display: "flex", flexDirection: "column", gap: 4, marginBottom: 8 }}>
                          {activeRun.stpSummaries.map((s: StpEventSummary) => {
                            const isSel = selectedStpEventId === s.event_id;
                            return (
                              <button
                                key={s.event_id}
                                className={`list-item ${isSel ? "selected" : ""}`}
                                onClick={() => onSelectStpEvent(activeRun.runId, s.event_id)}
                              >
                                <span className="mono">
                                  事件 #{s.event_id} · {s.at_ns} ns
                                </span>
                                <span className="small muted">
                                  {s.bridge_count} 桥 / {s.link_count} 链路
                                </span>
                              </button>
                            );
                          })}
                        </div>
                        {selectedStpEventId &&
                          activeRun.stpDetailsByEventId[selectedStpEventId] && (
                            <div>
                              <div className="panel-section-title">
                                <span>事件 #{selectedStpEventId} 桥与根桥</span>
                              </div>
                              <table className="mini-table" style={{ marginBottom: 8 }}>
                                <thead>
                                  <tr>
                                    <th>交换机</th>
                                    <th>根桥</th>
                                    <th>开销</th>
                                    <th>根端口</th>
                                  </tr>
                                </thead>
                                <tbody>
                                  {activeRun.stpDetailsByEventId[
                                    selectedStpEventId
                                  ].bridges.map((b) => (
                                    <tr key={b.bridge_id}>
                                      <td className="mono">{b.bridge_id}</td>
                                      <td className="mono">{b.root_bridge_id}</td>
                                      <td className="mono">{b.root_path_cost}</td>
                                      <td className="mono">{b.root_port_id ?? "根桥"}</td>
                                    </tr>
                                  ))}
                                </tbody>
                              </table>

                              <div className="panel-section-title">
                                <span>端口角色（已同步至画布标注）</span>
                              </div>
                              <table className="mini-table">
                                <thead>
                                  <tr>
                                    <th>链路</th>
                                    <th>端口角色</th>
                                  </tr>
                                </thead>
                                <tbody>
                                  {activeRun.stpDetailsByEventId[
                                    selectedStpEventId
                                  ].links.map((l) => (
                                    <tr key={l.link_id}>
                                      <td className="mono">{l.link_id}</td>
                                      <td className="mono">
                                        {l.ports
                                          .map((p) => `${p.port_id}:${p.role}`)
                                          .join(" | ")}
                                      </td>
                                    </tr>
                                  ))}
                                </tbody>
                              </table>
                            </div>
                          )}
                      </Disclosure>
                    )}

                    <Disclosure
                      title="逐跳时间线与详细轨迹 (Trace)"
                      badge={activeRun.summary?.trace_status ?? "按需加载"}
                      onToggle={(open) => {
                        if (open && !activeRun.trace) {
                          onLoadTrace(activeRun.runId, true);
                        }
                      }}
                    >
                      {!activeRun.trace ? (
                        <div className="small muted">正在按需加载轨迹...</div>
                      ) : (
                        <>
                          <div className="small muted" style={{ marginBottom: 6 }}>
                            轨迹状态：
                            <strong className="mono">
                              {activeRun.trace.status === "disabled"
                                ? "未开启（trace_limit = 0）"
                                : activeRun.trace.status === "collected"
                                  ? "已完整采集"
                                  : activeRun.trace.status === "truncated"
                                    ? "已达单次限额截断（不影响结案证据）"
                                    : activeRun.trace.status === "evicted"
                                      ? "历史轨迹已淘汰（必要证据仍保留）"
                                      : "无轨迹"}
                            </strong>
                          </div>
                          {activeRun.trace.entries.length > 0 && (
                            <table className="mini-table">
                              <thead>
                                <tr>
                                  <th>时刻</th>
                                  <th>事件</th>
                                  <th>位置 / 细节</th>
                                </tr>
                              </thead>
                              <tbody>
                                {activeRun.trace.entries.map((entry) => (
                                  <tr key={entry.event_id}>
                                    <td className="mono">{entry.at_ns}ns</td>
                                    <td>{traceEventLabel(entry.event)}</td>
                                    <td className="mono">
                                      {entry.port
                                        ? `${entry.port.device_name}/${entry.port.port_name}`
                                        : entry.probe_id ?? "—"}
                                      {entry.route?.ipv4_mtu && (
                                        <div>
                                          MTU {entry.route.ipv4_mtu.ipv4_total_bytes}/
                                          {entry.route.ipv4_mtu.mtu_bytes}B (
                                          {entry.route.ipv4_mtu.passed ? "通过" : "超限"})
                                        </div>
                                      )}
                                      {entry.learning && (
                                        <div>
                                          学习{" "}
                                          {entry.learning.kind === "arp"
                                            ? `ARP ${entry.learning.neighbor}`
                                            : `MAC VLAN ${entry.learning.vlan}`}
                                        </div>
                                      )}
                                    </td>
                                  </tr>
                                ))}
                              </tbody>
                            </table>
                          )}
                          {activeRun.trace.nextOffset !== null && (
                            <button
                              className="btn btn-xs"
                              style={{ width: "100%", marginTop: 6 }}
                              onClick={() => onLoadTrace(activeRun.runId, false)}
                            >
                              加载更多轨迹...
                            </button>
                          )}
                        </>
                      )}
                    </Disclosure>

                    {activeRun.importGaps && activeRun.importGaps.length > 0 && (
                      <Disclosure
                        title="影响当前探测的导入缺项"
                        badge={`${activeRun.importGaps.length}`}
                        defaultOpen
                      >
                        {activeRun.importGaps.map((gap) => (
                          <div
                            key={gap.id}
                            className="small"
                            style={{
                              padding: "5px 0",
                              borderBottom: "1px solid var(--border-subtle)",
                            }}
                          >
                            <div className="mono muted">{gap.source_path}</div>
                            <div>{gap.message}</div>
                          </div>
                        ))}
                      </Disclosure>
                    )}
                  </>
                )}
              </>
            )}
          </>
        )}

        {/* TAB 2: CONFIGURATION FORMS */}
        {activeTab === "config" && (
          <>
            {selectedDevice ? (
              <DeviceForm
                docKey={docKey}
                doc={doc}
                device={selectedDevice}
                selectedPortId={
                  selection.kind === "port" ? selection.portId : undefined
                }
                onSelectPort={(portId) =>
                  portId
                    ? onSelect({
                        kind: "port",
                        deviceId: selectedDevice.id,
                        portId,
                      })
                    : onSelect({ kind: "device", deviceId: selectedDevice.id })
                }
                onChangeDoc={onChangeDoc}
                onDeleteDevice={onDeleteDevice}
                onSaveAndRerun={onSaveAndRerun}
                rerunLabel={rerunLabel}
              />
            ) : selectedLink ? (
              <LinkForm
                doc={doc}
                link={selectedLink}
                onChangeDoc={onChangeDoc}
                onDeleteLink={onDeleteLink}
                onJumpToPort={(deviceId, portId) =>
                  onSelect({ kind: "port", deviceId, portId })
                }
                onSaveAndRerun={onSaveAndRerun}
                rerunLabel={rerunLabel}
              />
            ) : (
              <ScenarioForm
                doc={doc}
                onChangeDoc={onChangeDoc}
                onSaveAndRerun={onSaveAndRerun}
                rerunLabel={rerunLabel}
              />
            )}

            {/* Effective settings disclosure */}
            <Disclosure
              title="生效配置与默认值来源 (effective_settings)"
              open={effectiveOpen}
              onToggle={(open) => {
                setEffectiveOpen(open);
                if (open && effectiveSettings === null) {
                  onLoadEffectiveSettings();
                }
              }}
            >
              {!effectiveSettings ? (
                <div className="small muted">点击展开以查询当前文档修订的生效配置。</div>
              ) : effectiveSettings.length === 0 ? (
                <div className="small muted">无可列举的生效配置项。</div>
              ) : (
                <table className="mini-table">
                  <thead>
                    <tr>
                      <th>对象 / 键</th>
                      <th>生效值</th>
                      <th>来源</th>
                    </tr>
                  </thead>
                  <tbody>
                    {effectiveSettings.map((item, idx) => (
                      <tr key={idx}>
                        <td className="mono">
                          {item.location.port_id ??
                            item.location.device_id ??
                            item.location.probe_id ??
                            "全局"}
                          .{item.key}
                        </td>
                        <td className="mono">{String(item.value)}</td>
                        <td>
                          <span
                            className={`pill ${
                              item.source === "explicit"
                                ? "pill-ok"
                                : "pill-neutral"
                            }`}
                          >
                            {item.source === "explicit" ? "显式" : "默认"}
                          </span>
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              )}
            </Disclosure>

            {selection.kind !== "scenario" && (
              <button
                className="btn btn-sm"
                style={{ width: "100%" }}
                onClick={() => onSelect({ kind: "scenario" })}
              >
                <Sliders size={13} /> 编辑全局场景与探测清单
              </button>
            )}
          </>
        )}

        {/* TAB 3: RUN COMPARISON */}
        {activeTab === "compare" && (
          <CompareView
            runs={runs}
            baselineRunId={compareBaselineId}
            candidateRunId={compareCandidateId}
            comparison={comparison}
            loading={compareLoading}
            error={compareError}
            onChangeBaseline={onChangeCompareBaseline}
            onChangeCandidate={onChangeCompareCandidate}
            onRunCompare={onRunCompare}
            onLoadMore={onLoadMoreCompare}
            onInspectEvidence={onInspectRunEvidence}
          />
        )}
      </div>
    </aside>
  );
};
