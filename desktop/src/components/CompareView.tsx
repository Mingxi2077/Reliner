import React from "react";
import {
  AlertTriangle,
  ArrowRight,
  CheckCircle2,
  GitCompare,
  HelpCircle,
  Clock,
} from "lucide-react";
import type {
  ComparisonData,
  Outcome,
  ProbeComparison,
} from "../protocol/types";
import type { RunRecord } from "../store/types";

interface CompareViewProps {
  runs: RunRecord[];
  baselineRunId: string;
  candidateRunId: string;
  comparison: ComparisonData | null;
  loading: boolean;
  error: string | null;
  onChangeBaseline: (runId: string) => void;
  onChangeCandidate: (runId: string) => void;
  onRunCompare: (baselineId: string, candidateId: string) => void;
  onLoadMore: () => void;
  onInspectEvidence: (runId: string, evidenceId: string) => void;
}

function outcomeBadge(outcome: Outcome | null) {
  if (!outcome) {
    return <span className="pill pill-neutral">无此探测</span>;
  }
  switch (outcome.status) {
    case "reachable":
      return (
        <span className="pill pill-ok">
          <CheckCircle2 size={11} /> 连通 ({outcome.at_ns} ns)
        </span>
      );
    case "timeout":
      return (
        <span className="pill pill-fault">
          <AlertTriangle size={11} />{" "}
          {outcome.waiting_for === "probe_deadline" ? "发包前拒绝" : "超时"} (
          {outcome.at_ns} ns)
        </span>
      );
    case "incomplete":
      return (
        <span className="pill pill-warn">
          <Clock size={11} /> 未完成 ({outcome.at_ns} ns)
        </span>
      );
    case "undetermined":
      return (
        <span className="pill pill-warn">
          <HelpCircle size={11} /> 未判定（配置覆盖不足）
        </span>
      );
  }
}

function renderEvidenceRef(
  runId: string,
  outcome: Outcome | null,
  requestEvId: string | null | undefined,
  onInspect: (runId: string, evidenceId: string) => void
) {
  if (!outcome) return null;
  const links: Array<{ label: string; id: string }> = [];
  if (requestEvId) links.push({ label: `请求证据 #${requestEvId}`, id: requestEvId });
  if (outcome.status === "reachable") {
    links.push({ label: `回包证据 #${outcome.evidence_id}`, id: outcome.evidence_id });
  } else if (outcome.status === "timeout" && outcome.cause) {
    links.push({ label: `阻断证据 #${outcome.cause}`, id: outcome.cause });
  }
  if (links.length === 0) return null;
  return (
    <div style={{ display: "flex", flexWrap: "wrap", gap: 4, marginTop: 4 }}>
      {links.map((item) => (
        <button
          key={item.id}
          className="btn btn-xs"
          onClick={() => onInspect(runId, item.id)}
          title={`回查运行 #${runId} 的证据 #${item.id}`}
        >
          {item.label}
        </button>
      ))}
    </div>
  );
}

export const CompareView: React.FC<CompareViewProps> = ({
  runs,
  baselineRunId,
  candidateRunId,
  comparison,
  loading,
  error,
  onChangeBaseline,
  onChangeCandidate,
  onRunCompare,
  onLoadMore,
  onInspectEvidence,
}) => {
  const readyRuns = runs.filter(
    (r) =>
      !r.evicted &&
      r.summary &&
      r.summary.probe_counts !== undefined &&
      (r.summary.status === "finished" ||
        r.summary.status === "incomplete" ||
        r.summary.status === "cancelled")
  );
  const sameRunSelected =
    Boolean(baselineRunId) && baselineRunId === candidateRunId;

  if (readyRuns.length < 2) {
    return (
      <div className="card">
        <div style={{ fontWeight: 600, marginBottom: 6 }}>暂无可对比的两次运行</div>
        <div className="small muted">
          完成第一次排障运行后，修改配置并重跑，即可在此按探测 ID 对齐对比修复前后的修订与证据。
        </div>
      </div>
    );
  }

  return (
    <div>
      <div className="card">
        <div className="panel-section-title">
          <span style={{ display: "inline-flex", alignItems: "center", gap: 4 }}>
            <GitCompare size={13} /> 选择要对比的两次运行
          </span>
        </div>

        <div className="field">
          <label>基线运行（修复前）</label>
          <select
            value={baselineRunId}
            onChange={(e) => onChangeBaseline(e.target.value)}
          >
            {readyRuns.map((r) => (
              <option key={r.runId} value={r.runId}>
                运行 #{r.runId} · 原生修订 {r.origin.native_revision}（生效修订{" "}
                {r.origin.effective_revision}）
              </option>
            ))}
          </select>
        </div>

        <div className="field">
          <label>候选运行（修复后）</label>
          <select
            value={candidateRunId}
            onChange={(e) => onChangeCandidate(e.target.value)}
          >
            {readyRuns.map((r) => (
              <option key={r.runId} value={r.runId}>
                运行 #{r.runId} · 原生修订 {r.origin.native_revision}（生效修订{" "}
                {r.origin.effective_revision}）
              </option>
            ))}
          </select>
        </div>

        <button
          className="btn btn-sm btn-primary"
          style={{ width: "100%" }}
          disabled={loading || !baselineRunId || !candidateRunId || sameRunSelected}
          onClick={() => onRunCompare(baselineRunId, candidateRunId)}
        >
          <GitCompare size={13} />{" "}
          {loading
            ? "对比加载中..."
            : sameRunSelected
              ? "请选择两次不同的运行"
              : "执行修订对比"}
        </button>
      </div>

      {error && (
        <div className="card fault small" style={{ color: "var(--status-fault)" }}>
          对比失败：{error}
        </div>
      )}

      {comparison && (
        <>
          {(() => {
            let fixed = 0;
            let regressed = 0;
            let stillOk = 0;
            let stillTimeout = 0;
            let undetermined = 0;
            let incomplete = 0;
            let addedRemoved = 0;
            for (const row of comparison.entries) {
              const b = row.baseline?.outcome.status;
              const c = row.candidate?.outcome.status;
              if (!b || !c) {
                addedRemoved++;
              } else if (b === "timeout" && c === "reachable") {
                fixed++;
              } else if (b === "reachable" && c === "timeout") {
                regressed++;
              } else if (b === "reachable" && c === "reachable") {
                stillOk++;
              } else if (b === "timeout" && c === "timeout") {
                stillTimeout++;
              } else if (b === "undetermined" || c === "undetermined") {
                undetermined++;
              } else if (b === "incomplete" || c === "incomplete") {
                incomplete++;
              } else {
                stillTimeout++;
              }
            }
            return (
              <div className="compare-summary-strip">
                {fixed > 0 && <span className="pill pill-ok">✓ 新修复 {fixed}</span>}
                {regressed > 0 && <span className="pill pill-fault">✕ 新退化 {regressed}</span>}
                {stillOk > 0 && <span className="pill pill-neutral">保持连通 {stillOk}</span>}
                {stillTimeout > 0 && <span className="pill pill-fault">仍超时 {stillTimeout}</span>}
                {undetermined > 0 && <span className="pill pill-warn">未判定 {undetermined}</span>}
                {incomplete > 0 && <span className="pill pill-warn">未完成 {incomplete}</span>}
                {addedRemoved > 0 && <span className="pill pill-neutral">增删探测 {addedRemoved}</span>}
              </div>
            );
          })()}

          <div className="card small">
            <div style={{ display: "flex", alignItems: "center", gap: 6, flexWrap: "wrap" }}>
              <span className="pill pill-neutral mono">
                基线 #{comparison.baseline_origin.run_id} · r{comparison.baseline.revision} (m
                {comparison.baseline.model_version})
              </span>
              <ArrowRight size={13} className="muted" />
              <span className="pill pill-neutral mono">
                候选 #{comparison.candidate_origin.run_id} · r{comparison.candidate.revision} (m
                {comparison.candidate.model_version})
              </span>
            </div>
            <div className="muted" style={{ marginTop: 6 }}>
              注：「同探测参数」仅核对探测源目、时刻、TTL、载荷与模型版本；如涉及定时断链请另核对链路事件。
            </div>
          </div>

          {comparison.entries.map((row: ProbeComparison) => {
            const basePayloadText = row.baseline
              ? `${row.baseline.payload_bytes ?? 56} B`
              : "—";
            const candPayloadText = row.candidate
              ? `${row.candidate.payload_bytes ?? 56} B`
              : "—";
            const improved =
              row.baseline?.outcome.status === "timeout" &&
              row.candidate?.outcome.status === "reachable";
            const regressed =
              row.baseline?.outcome.status === "reachable" &&
              row.candidate?.outcome.status === "timeout";

            return (
              <div
                key={row.probe_id}
                className={`card ${improved ? "ok" : regressed ? "fault" : ""}`}
              >
                <div
                  style={{
                    display: "flex",
                    justifyContent: "space-between",
                    alignItems: "center",
                    marginBottom: 6,
                  }}
                >
                  <strong className="mono">{row.probe_id}</strong>
                  <span
                    className={`pill ${
                      row.same_probe_parameters ? "pill-ok" : "pill-warn"
                    }`}
                  >
                    {row.same_probe_parameters ? "同探测参数" : "探测参数已变"}
                  </span>
                </div>

                <div className="small muted" style={{ marginBottom: 8 }}>
                  {(row.candidate ?? row.baseline)?.source.name} →{" "}
                  {(row.candidate ?? row.baseline)?.destination.name} · 生效载荷：基线{" "}
                  {basePayloadText} / 候选 {candPayloadText}
                </div>

                <div
                  style={{
                    display: "grid",
                    gridTemplateColumns: "minmax(0, 1fr) minmax(0, 1fr)",
                    gap: 8,
                  }}
                >
                  <div
                    style={{
                      minWidth: 0,
                      padding: 6,
                      borderRadius: 6,
                      background: "var(--bg-subtle)",
                      border: "1px solid var(--border-subtle)",
                    }}
                  >
                    <div className="small muted" style={{ marginBottom: 4 }}>
                      基线 r{comparison.baseline.revision}
                    </div>
                    {outcomeBadge(row.baseline?.outcome ?? null)}
                    {renderEvidenceRef(
                      comparison.baseline_origin.run_id,
                      row.baseline?.outcome ?? null,
                      row.baseline?.request_evidence_id,
                      onInspectEvidence
                    )}
                  </div>

                  <div
                    style={{
                      minWidth: 0,
                      padding: 6,
                      borderRadius: 6,
                      background: "var(--bg-subtle)",
                      border: "1px solid var(--border-subtle)",
                    }}
                  >
                    <div className="small muted" style={{ marginBottom: 4 }}>
                      候选 r{comparison.candidate.revision}
                    </div>
                    {outcomeBadge(row.candidate?.outcome ?? null)}
                    {renderEvidenceRef(
                      comparison.candidate_origin.run_id,
                      row.candidate?.outcome ?? null,
                      row.candidate?.request_evidence_id,
                      onInspectEvidence
                    )}
                  </div>
                </div>
              </div>
            );
          })}

          {comparison.next_offset !== null && (
            <button
              className="btn btn-sm"
              style={{ width: "100%" }}
              disabled={loading}
              onClick={onLoadMore}
            >
              加载更多对比项...
            </button>
          )}
        </>
      )}
    </div>
  );
};
