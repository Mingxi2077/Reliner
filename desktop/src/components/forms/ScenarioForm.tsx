import React, { useEffect, useState } from "react";
import { Plus, Trash2 } from "lucide-react";
import type { NativeLabDocument, ScenarioSpec } from "../../model/document";
import { bumpNativeRevision, listAllPorts, nextUniqueId } from "../../model/document";
import { NumericInput } from "./NumericInput";

interface ScenarioFormProps {
  doc: NativeLabDocument;
  onChangeDoc: (next: NativeLabDocument) => void;
  onSaveAndRerun?: () => void;
  rerunLabel?: string;
}

export const ScenarioForm: React.FC<ScenarioFormProps> = ({
  doc,
  onChangeDoc,
  onSaveAndRerun,
  rerunLabel = "保存并重跑",
}) => {
  const hosts = doc.devices.filter((d) => d.kind === "host");
  const allPorts = listAllPorts(doc);
  const ipPorts = allPorts.filter((p) => p.deviceKind !== "switch");

  // Buffer native revision locally so Backspace never immediately commits "" ([invalid_document])
  const [revDraft, setRevDraft] = useState(doc.revision);
  const [revFocused, setRevFocused] = useState(false);
  useEffect(() => {
    if (!revFocused) {
      setRevDraft(doc.revision);
    }
  }, [doc.revision, revFocused]);

  const updateScenario = (updater: (s: ScenarioSpec) => ScenarioSpec) => {
    if (!doc.scenario) return;
    onChangeDoc({ ...doc, scenario: updater(doc.scenario) });
  };

  return (
    <div>
      <div className="card">
        <div className="panel-section-title">
          <span>拓扑元信息</span>
          <span className="pill pill-neutral">format_version {doc.format_version}</span>
        </div>
        <div className="form-grid">
          <div className="field">
            <label>拓扑 ID（创建后不可改）</label>
            <input className="mono" value={doc.id} disabled />
          </div>
          <div className="field">
            <label>原生修订号 (revision)</label>
            <div style={{ display: "flex", gap: 4 }}>
              <input
                className="mono"
                style={{ flex: 1, minWidth: 0 }}
                value={revDraft}
                onFocus={() => setRevFocused(true)}
                onBlur={() => {
                  setRevFocused(false);
                  const trimmed = revDraft.trim();
                  if (trimmed === "") {
                    setRevDraft(doc.revision);
                  } else if (trimmed !== doc.revision) {
                    onChangeDoc({ ...doc, revision: trimmed });
                  }
                }}
                onChange={(e) => {
                  const val = e.target.value;
                  setRevDraft(val);
                  if (val.trim() !== "") {
                    onChangeDoc({ ...doc, revision: val.trim() });
                  }
                }}
              />
              <button
                className="btn btn-xs"
                style={{ height: 31, flexShrink: 0 }}
                title="递增修订号"
                onClick={() => {
                  const nextRev = bumpNativeRevision(doc.revision);
                  setRevDraft(nextRev);
                  onChangeDoc({ ...doc, revision: nextRev });
                }}
              >
                +1
              </button>
            </div>
          </div>
        </div>
      </div>

      {doc.format_version === 1 ? (
        <div className="card">
          <div className="panel-section-title">
            <span>单探测配置（格式 1）</span>
          </div>
          <div className="form-grid">
            <div className="field">
              <label>源主机</label>
              <select
                value={doc.probe?.source ?? ""}
                onChange={(e) =>
                  onChangeDoc({
                    ...doc,
                    probe: {
                      source: e.target.value,
                      destination: doc.probe?.destination ?? "",
                      deadline_ns: doc.probe?.deadline_ns ?? "2000",
                    },
                  })
                }
              >
                {(!doc.probe?.source ||
                  !hosts.some((h) => h.id === doc.probe?.source)) && (
                  <option value={doc.probe?.source ?? ""}>
                    {doc.probe?.source
                      ? `${doc.probe.source}（已不存在）`
                      : "— 请选择源主机 —"}
                  </option>
                )}
                {hosts.map((h) => (
                  <option key={h.id} value={h.id}>
                    {h.name} ({h.id})
                  </option>
                ))}
              </select>
            </div>
            <div className="field">
              <label>目的主机</label>
              <select
                value={doc.probe?.destination ?? ""}
                onChange={(e) =>
                  onChangeDoc({
                    ...doc,
                    probe: {
                      source: doc.probe?.source ?? "",
                      destination: e.target.value,
                      deadline_ns: doc.probe?.deadline_ns ?? "2000",
                    },
                  })
                }
              >
                {(!doc.probe?.destination ||
                  !hosts.some((h) => h.id === doc.probe?.destination)) && (
                  <option value={doc.probe?.destination ?? ""}>
                    {doc.probe?.destination
                      ? `${doc.probe.destination}（已不存在）`
                      : "— 请选择目的主机 —"}
                  </option>
                )}
                {hosts.map((h) => (
                  <option key={h.id} value={h.id}>
                    {h.name} ({h.id})
                  </option>
                ))}
              </select>
            </div>
          </div>
          <div className="field">
            <label>截止时刻 deadline_ns（纳秒）</label>
            <input
              className="mono"
              value={doc.probe?.deadline_ns ?? "2000"}
              onChange={(e) =>
                onChangeDoc({
                  ...doc,
                  probe: {
                    source: doc.probe?.source ?? "",
                    destination: doc.probe?.destination ?? "",
                    deadline_ns: e.target.value,
                  },
                })
              }
            />
          </div>
          <div
            style={{
              display: "flex",
              alignItems: "center",
              justifyContent: "space-between",
              gap: 8,
            }}
          >
            <div className="small muted">
              保留格式 1 原语义，保存时不自动升级版本。
            </div>
            {onSaveAndRerun && (
              <button
                type="button"
                className="btn btn-sm btn-coral"
                onClick={onSaveAndRerun}
              >
                {rerunLabel}
              </button>
            )}
          </div>
        </div>
      ) : (
        doc.scenario && (
          <>
            <div className="card">
              <div className="panel-section-title">
                <span>场景时钟与收敛参数</span>
              </div>
              <div className="form-grid">
                <div className="field">
                  <label>场景结束时刻 end_ns</label>
                  <input
                    className="mono"
                    value={doc.scenario.end_ns}
                    onChange={(e) =>
                      updateScenario((s) => ({ ...s, end_ns: e.target.value }))
                    }
                  />
                </div>
                <div className="field">
                  <label>轨迹限额 trace_limit</label>
                  <NumericInput
                    optional
                    min={0}
                    max={1000000}
                    placeholder="0（关闭轨迹）"
                    value={doc.scenario.trace_limit}
                    onValueChange={(trace_limit) =>
                      updateScenario((s) => ({ ...s, trace_limit }))
                    }
                  />
                </div>
                <div className="field">
                  <label>ARP 生存期 arp_lifetime_ns</label>
                  <input
                    className="mono"
                    value={doc.scenario.arp_lifetime_ns}
                    onChange={(e) =>
                      updateScenario((s) => ({
                        ...s,
                        arp_lifetime_ns: e.target.value,
                      }))
                    }
                  />
                </div>
                <div className="field">
                  <label>MAC 生存期 mac_lifetime_ns</label>
                  <input
                    className="mono"
                    value={doc.scenario.mac_lifetime_ns}
                    onChange={(e) =>
                      updateScenario((s) => ({
                        ...s,
                        mac_lifetime_ns: e.target.value,
                      }))
                    }
                  />
                </div>
                <div className="field">
                  <label>动态路由收敛时长 (ns，可选)</label>
                  <input
                    className="mono"
                    placeholder="未启用"
                    value={doc.scenario.routing_convergence_ns ?? ""}
                    onChange={(e) =>
                      updateScenario((s) => ({
                        ...s,
                        routing_convergence_ns:
                          e.target.value.trim() === ""
                            ? undefined
                            : e.target.value.trim(),
                      }))
                    }
                  />
                </div>
                <div className="field">
                  <label>STP 收敛时长 (ns，可选)</label>
                  <input
                    className="mono"
                    placeholder="未启用"
                    value={doc.scenario.stp_convergence_ns ?? ""}
                    onChange={(e) =>
                      updateScenario((s) => ({
                        ...s,
                        stp_convergence_ns:
                          e.target.value.trim() === ""
                            ? undefined
                            : e.target.value.trim(),
                      }))
                    }
                  />
                </div>
              </div>
              {onSaveAndRerun && (
                <div style={{ display: "flex", justifyContent: "flex-end", marginTop: 4 }}>
                  <button
                    type="button"
                    className="btn btn-sm btn-coral"
                    onClick={onSaveAndRerun}
                  >
                    {rerunLabel}
                  </button>
                </div>
              )}
            </div>

            {/* Probes */}
            <div className="card">
              <div className="panel-section-title">
                <span>探测清单 ({doc.scenario.probes.length})</span>
                <button
                  className="btn btn-xs"
                  disabled={hosts.length < 2}
                  onClick={() =>
                    updateScenario((s) => {
                      const usedIds = new Set(s.probes.map((p) => p.id));
                      return {
                        ...s,
                        probes: [
                          ...s.probes,
                          {
                            id: nextUniqueId("probe", usedIds),
                            source: hosts[0]?.id ?? "",
                            destination: hosts[1]?.id ?? hosts[0]?.id ?? "",
                            at_ns: "0",
                            deadline_ns: s.end_ns || "2000",
                          },
                        ],
                      };
                    })
                  }
                >
                  <Plus size={11} /> 添加探测
                </button>
              </div>
              {doc.scenario.probes.map((probe, idx) => (
                <div
                  key={idx}
                  style={{
                    border: "1px solid var(--border-subtle)",
                    borderRadius: 6,
                    padding: 8,
                    marginBottom: 6,
                    background: "var(--bg-subtle)",
                  }}
                >
                  <div className="form-grid">
                    <div className="field">
                      <label>探测 ID</label>
                      <input
                        className="mono"
                        value={probe.id}
                        onChange={(e) =>
                          updateScenario((s) => {
                            const probes = [...s.probes];
                            probes[idx] = { ...probe, id: e.target.value };
                            return { ...s, probes };
                          })
                        }
                      />
                    </div>
                    <div className="field">
                      <label>载荷字节（可选，默认 56）</label>
                      <NumericInput
                        optional
                        min={0}
                        max={65507}
                        placeholder="56"
                        value={probe.payload_bytes}
                        onValueChange={(payload_bytes) =>
                          updateScenario((s) => {
                            const probes = [...s.probes];
                            probes[idx] = { ...probe, payload_bytes };
                            return { ...s, probes };
                          })
                        }
                      />
                    </div>
                    <div className="field">
                      <label>源主机</label>
                      <select
                        value={probe.source}
                        onChange={(e) =>
                          updateScenario((s) => {
                            const probes = [...s.probes];
                            probes[idx] = { ...probe, source: e.target.value };
                            return { ...s, probes };
                          })
                        }
                      >
                        {!hosts.some((h) => h.id === probe.source) && (
                          <option value={probe.source}>
                            {probe.source || "— 请选择 —"}
                          </option>
                        )}
                        {hosts.map((h) => (
                          <option key={h.id} value={h.id}>
                            {h.name} ({h.id})
                          </option>
                        ))}
                      </select>
                    </div>
                    <div className="field">
                      <label>目的主机</label>
                      <select
                        value={probe.destination}
                        onChange={(e) =>
                          updateScenario((s) => {
                            const probes = [...s.probes];
                            probes[idx] = { ...probe, destination: e.target.value };
                            return { ...s, probes };
                          })
                        }
                      >
                        {!hosts.some((h) => h.id === probe.destination) && (
                          <option value={probe.destination}>
                            {probe.destination || "— 请选择 —"}
                          </option>
                        )}
                        {hosts.map((h) => (
                          <option key={h.id} value={h.id}>
                            {h.name} ({h.id})
                          </option>
                        ))}
                      </select>
                    </div>
                    <div className="field">
                      <label>发送时刻 at_ns</label>
                      <input
                        className="mono"
                        value={probe.at_ns}
                        onChange={(e) =>
                          updateScenario((s) => {
                            const probes = [...s.probes];
                            probes[idx] = { ...probe, at_ns: e.target.value };
                            return { ...s, probes };
                          })
                        }
                      />
                    </div>
                    <div className="field">
                      <label>截止时刻 deadline_ns</label>
                      <input
                        className="mono"
                        value={probe.deadline_ns}
                        onChange={(e) =>
                          updateScenario((s) => {
                            const probes = [...s.probes];
                            probes[idx] = { ...probe, deadline_ns: e.target.value };
                            return { ...s, probes };
                          })
                        }
                      />
                    </div>
                  </div>
                  <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center" }}>
                    <label className="small muted" style={{ display: "inline-flex", alignItems: "center", gap: 4 }}>
                      TTL:
                      <NumericInput
                        optional
                        min={1}
                        max={255}
                        placeholder="64"
                        style={{
                          width: 56,
                          height: 22,
                          padding: "0 4px",
                          borderRadius: 4,
                          border: "1px solid var(--border-strong)",
                        }}
                        value={probe.ttl}
                        onValueChange={(ttl) =>
                          updateScenario((s) => {
                            const probes = [...s.probes];
                            probes[idx] = { ...probe, ttl };
                            return { ...s, probes };
                          })
                        }
                      />
                    </label>
                    <button
                      className="btn btn-xs btn-danger"
                      onClick={() =>
                        updateScenario((s) => ({
                          ...s,
                          probes: s.probes.filter((_, i) => i !== idx),
                        }))
                      }
                    >
                      <Trash2 size={11} /> 删除探测
                    </button>
                  </div>
                </div>
              ))}
            </div>

            {/* Link Actions */}
            <div className="card">
              <div className="panel-section-title">
                <span>定时链路通断动作 ({doc.scenario.link_actions?.length ?? 0})</span>
                <button
                  className="btn btn-xs"
                  disabled={doc.links.length === 0}
                  onClick={() =>
                    updateScenario((s) => {
                      const existing = s.link_actions ?? [];
                      const linkId = doc.links[0].id;
                      const forLink = existing.filter((a) => a.link === linkId);
                      const lastForLink = forLink[forLink.length - 1];
                      const nextState: "up" | "down" =
                        lastForLink?.state === "down" ? "up" : "down";
                      const maxEnd = /^\d+$/.test((s.end_ns ?? "").trim())
                        ? BigInt(s.end_ns.trim())
                        : 5000n;
                      const prevAt =
                        lastForLink && /^\d+$/.test(lastForLink.at_ns.trim())
                          ? BigInt(lastForLink.at_ns.trim())
                          : 0n;
                      const proposed = prevAt + 500n;
                      const atNs =
                        proposed < maxEnd
                          ? proposed.toString()
                          : maxEnd > 1n
                            ? (maxEnd - 1n).toString()
                            : "1";
                      return {
                        ...s,
                        link_actions: [
                          ...existing,
                          { link: linkId, state: nextState, at_ns: atNs },
                        ],
                      };
                    })
                  }
                >
                  <Plus size={11} /> 添加动作
                </button>
              </div>
              {(doc.scenario.link_actions ?? []).map((action, idx) => (
                <div
                  key={idx}
                  style={{
                    display: "grid",
                    gridTemplateColumns: "minmax(0, 1fr) 64px 72px 26px",
                    gap: 4,
                    marginBottom: 5,
                    alignItems: "center",
                  }}
                >
                  <select
                    style={{
                      width: "100%",
                      minWidth: 0,
                      height: 26,
                      borderRadius: 4,
                      border: "1px solid var(--border-strong)",
                    }}
                    value={action.link}
                    onChange={(e) =>
                      updateScenario((s) => {
                        const list = [...(s.link_actions ?? [])];
                        list[idx] = { ...action, link: e.target.value };
                        return { ...s, link_actions: list };
                      })
                    }
                  >
                    {!doc.links.some((l) => l.id === action.link) && (
                      <option value={action.link}>{action.link}</option>
                    )}
                    {doc.links.map((l) => (
                      <option key={l.id} value={l.id}>
                        {l.id}
                      </option>
                    ))}
                  </select>
                  <select
                    style={{
                      width: "100%",
                      minWidth: 0,
                      height: 26,
                      borderRadius: 4,
                      border: "1px solid var(--border-strong)",
                    }}
                    value={action.state}
                    onChange={(e) =>
                      updateScenario((s) => {
                        const list = [...(s.link_actions ?? [])];
                        list[idx] = {
                          ...action,
                          state: e.target.value as "up" | "down",
                        };
                        return { ...s, link_actions: list };
                      })
                    }
                  >
                    <option value="down">down</option>
                    <option value="up">up</option>
                  </select>
                  <input
                    className="mono"
                    style={{
                      width: "100%",
                      minWidth: 0,
                      height: 26,
                      padding: "0 5px",
                      borderRadius: 4,
                      border: "1px solid var(--border-strong)",
                    }}
                    value={action.at_ns}
                    onChange={(e) =>
                      updateScenario((s) => {
                        const list = [...(s.link_actions ?? [])];
                        list[idx] = { ...action, at_ns: e.target.value };
                        return { ...s, link_actions: list };
                      })
                    }
                  />
                  <button
                    className="btn btn-xs btn-danger"
                    onClick={() =>
                      updateScenario((s) => ({
                        ...s,
                        link_actions: (s.link_actions ?? []).filter((_, i) => i !== idx),
                      }))
                    }
                  >
                    <Trash2 size={10} />
                  </button>
                </div>
              ))}
            </div>

            {/* Warmup */}
            <div className="card">
              <div className="panel-section-title">
                <span>ARP 预热 ({doc.scenario.warmup?.length ?? 0})</span>
                <button
                  className="btn btn-xs"
                  disabled={ipPorts.length === 0}
                  title={
                    ipPorts.length === 0
                      ? "需要至少一个主机或路由器三层端口"
                      : "添加一条 ARP 邻居预热记录"
                  }
                  onClick={() =>
                    updateScenario((s) => ({
                      ...s,
                      warmup: [
                        ...(s.warmup ?? []),
                        {
                          port: ipPorts[0]?.portId ?? "",
                          neighbor: "192.0.2.1",
                          at_ns: "0",
                        },
                      ],
                    }))
                  }
                >
                  <Plus size={11} /> 添加预热
                </button>
              </div>
              {(doc.scenario.warmup ?? []).map((w, idx) => (
                <div
                  key={idx}
                  style={{
                    display: "grid",
                    gridTemplateColumns: "minmax(0, 1fr) 96px 56px 26px",
                    gap: 4,
                    marginBottom: 5,
                    alignItems: "center",
                  }}
                >
                  <select
                    style={{
                      width: "100%",
                      minWidth: 0,
                      height: 26,
                      borderRadius: 4,
                      border: "1px solid var(--border-strong)",
                    }}
                    value={w.port}
                    onChange={(e) =>
                      updateScenario((s) => {
                        const list = [...(s.warmup ?? [])];
                        list[idx] = { ...w, port: e.target.value };
                        return { ...s, warmup: list };
                      })
                    }
                  >
                    {!ipPorts.some((p) => p.portId === w.port) && (
                      <option value={w.port}>{w.port || "— 请选择接口 —"}</option>
                    )}
                    {ipPorts.map((p) => (
                      <option key={p.portId} value={p.portId}>
                        {p.deviceName}/{p.portName}
                      </option>
                    ))}
                  </select>
                  <input
                    className="mono"
                    style={{
                      width: "100%",
                      minWidth: 0,
                      height: 26,
                      padding: "0 5px",
                      borderRadius: 4,
                      border: "1px solid var(--border-strong)",
                    }}
                    value={w.neighbor}
                    onChange={(e) =>
                      updateScenario((s) => {
                        const list = [...(s.warmup ?? [])];
                        list[idx] = { ...w, neighbor: e.target.value };
                        return { ...s, warmup: list };
                      })
                    }
                  />
                  <input
                    className="mono"
                    style={{
                      width: "100%",
                      minWidth: 0,
                      height: 26,
                      padding: "0 5px",
                      borderRadius: 4,
                      border: "1px solid var(--border-strong)",
                    }}
                    value={w.at_ns}
                    onChange={(e) =>
                      updateScenario((s) => {
                        const list = [...(s.warmup ?? [])];
                        list[idx] = { ...w, at_ns: e.target.value };
                        return { ...s, warmup: list };
                      })
                    }
                  />
                  <button
                    className="btn btn-xs btn-danger"
                    onClick={() =>
                      updateScenario((s) => ({
                        ...s,
                        warmup: (s.warmup ?? []).filter((_, i) => i !== idx),
                      }))
                    }
                  >
                    <Trash2 size={10} />
                  </button>
                </div>
              ))}
            </div>
          </>
        )
      )}
    </div>
  );
};
