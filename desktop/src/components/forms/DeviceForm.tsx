import React, { useEffect, useMemo, useState } from "react";
import { ArrowDown, ArrowUp, Plus, Shield, Trash2 } from "lucide-react";
import type {
  AclRuleSpec,
  DeviceSpec,
  HostDeviceSpec,
  IpPortSpec,
  NativeLabDocument,
  RouterDeviceSpec,
  SwitchDeviceSpec,
  SwitchPortSpec,
} from "../../model/document";
import { addPortToDevice, listAllPorts } from "../../model/document";
import { NumericInput } from "./NumericInput";

interface DeviceFormProps {
  docKey?: string;
  doc: NativeLabDocument;
  device: DeviceSpec;
  selectedPortId?: string;
  onSelectPort: (portId: string | undefined) => void;
  onChangeDoc: (next: NativeLabDocument) => void;
  onDeleteDevice: (deviceId: string) => void;
  onSaveAndRerun?: () => void;
  rerunLabel?: string;
}

function collectTopologyVlans(doc: NativeLabDocument): number[] {
  const set = new Set<number>();
  for (const d of doc.devices) {
    if (d.kind !== "switch") continue;
    for (const p of d.ports) {
      if (p.mode.kind === "access") {
        set.add(p.mode.vlan);
      } else {
        for (const v of p.mode.allowed_vlans) set.add(v);
        if (p.mode.native_vlan !== undefined) set.add(p.mode.native_vlan);
      }
    }
  }
  return Array.from(set).sort((a, b) => a - b);
}

function parseVlanCsv(text: string): { valid: number[]; ignored: string[] } {
  const nums: number[] = [];
  const ignored: string[] = [];
  for (const raw of text.split(/[,，\s]+/)) {
    const trimmed = raw.trim();
    if (!trimmed) continue;
    const range = /^(\d+)-(\d+)$/.exec(trimmed);
    if (range) {
      const a = Number(range[1]);
      const b = Number(range[2]);
      if (a >= 1 && b <= 4094 && a <= b && b - a < 256) {
        for (let v = a; v <= b; v++) nums.push(v);
      } else {
        ignored.push(trimmed);
      }
      continue;
    }
    if (/^\d+$/.test(trimmed)) {
      const n = Number(trimmed);
      if (Number.isInteger(n) && n >= 1 && n <= 4094) {
        nums.push(n);
        continue;
      }
    }
    ignored.push(trimmed);
  }
  return {
    valid: Array.from(new Set(nums)).sort((a, b) => a - b),
    ignored,
  };
}

function suggestNeighborIp(port: IpPortSpec | undefined): string {
  if (!port) return "192.0.2.2";
  const parts = port.ipv4.trim().split(".").map(Number);
  if (parts.length !== 4 || parts.some((n) => !Number.isInteger(n) || n < 0 || n > 255)) {
    return "192.0.2.2";
  }
  const last = parts[3];
  const nextLast = last === 1 ? 2 : last === 254 ? 253 : last + 1;
  return `${parts[0]}.${parts[1]}.${parts[2]}.${nextLast}`;
}

export const DeviceForm: React.FC<DeviceFormProps> = ({
  docKey,
  doc,
  device,
  selectedPortId,
  onSelectPort,
  onChangeDoc,
  onDeleteDevice,
  onSaveAndRerun,
  rerunLabel = "保存并重跑",
}) => {
  const isLegacy = doc.format_version === 1;
  const allPorts = listAllPorts(doc);
  const linkByPort = new Map(allPorts.map((p) => [p.portId, p.connectedLinkId]));
  const topologyVlans = collectTopologyVlans(doc);
  const effectiveDocKey = docKey ?? `${doc.id}-${doc.revision}`;

  // Identify router ports connected directly to another router (best candidates for link_state peers)
  const routerPeerPortIds = useMemo(() => {
    const portKind = new Map(allPorts.map((p) => [p.portId, p.deviceKind]));
    const set = new Set<string>();
    for (const l of doc.links) {
      if (portKind.get(l.ports[0]) === "router" && portKind.get(l.ports[1]) === "router") {
        set.add(l.ports[0]);
        set.add(l.ports[1]);
      }
    }
    return set;
  }, [allPorts, doc.links]);

  const updateDevice = (updater: (d: DeviceSpec) => DeviceSpec) => {
    onChangeDoc({
      ...doc,
      devices: doc.devices.map((d) => (d.id === device.id ? updater(d) : d)),
    });
  };

  const handleAddPort = () => {
    const res = addPortToDevice(doc, device.id);
    if (res) {
      onChangeDoc(res.doc);
      onSelectPort(res.portId);
    }
  };

  if (device.kind === "host") {
    return (
      <HostEditor
        device={device}
        isLegacy={isLegacy}
        connectedLinkId={linkByPort.get(device.port.id) ?? null}
        onChange={(next) => updateDevice(() => next)}
        onDelete={() => onDeleteDevice(device.id)}
        onSaveAndRerun={onSaveAndRerun}
        rerunLabel={rerunLabel}
      />
    );
  }

  if (device.kind === "switch") {
    return (
      <SwitchEditor
        docKey={effectiveDocKey}
        device={device}
        isLegacy={isLegacy}
        selectedPortId={selectedPortId}
        linkByPort={linkByPort}
        topologyVlans={topologyVlans}
        onSelectPort={onSelectPort}
        onAddPort={handleAddPort}
        onChange={(next) => updateDevice(() => next)}
        onDelete={() => onDeleteDevice(device.id)}
        onSaveAndRerun={onSaveAndRerun}
        rerunLabel={rerunLabel}
      />
    );
  }

  return (
    <RouterEditor
      device={device}
      isLegacy={isLegacy}
      selectedPortId={selectedPortId}
      linkByPort={linkByPort}
      routerPeerPortIds={routerPeerPortIds}
      onSelectPort={onSelectPort}
      onAddPort={handleAddPort}
      onChange={(next) => updateDevice(() => next)}
      onDelete={() => onDeleteDevice(device.id)}
      onSaveAndRerun={onSaveAndRerun}
      rerunLabel={rerunLabel}
    />
  );
};

const HostEditor: React.FC<{
  device: HostDeviceSpec;
  isLegacy: boolean;
  connectedLinkId: string | null;
  onChange: (next: HostDeviceSpec) => void;
  onDelete: () => void;
  onSaveAndRerun?: () => void;
  rerunLabel: string;
}> = ({
  device,
  isLegacy,
  connectedLinkId,
  onChange,
  onDelete,
  onSaveAndRerun,
  rerunLabel,
}) => (
  <div>
    <div className="card">
      <div
        style={{
          display: "flex",
          justifyContent: "space-between",
          alignItems: "center",
          marginBottom: 10,
        }}
      >
        <div>
          <span className="pill pill-neutral" style={{ marginRight: 6 }}>
            主机
          </span>
          <strong className="mono">{device.id}</strong>
        </div>
        <button className="btn btn-xs btn-danger" onClick={onDelete}>
          <Trash2 size={12} /> 删除设备
        </button>
      </div>

      <div className="field">
        <label>显示名称</label>
        <input
          value={device.name}
          onChange={(e) => onChange({ ...device, name: e.target.value })}
        />
      </div>

      <div className="form-grid">
        <div className="field">
          <label>IPv4 地址</label>
          <input
            className="mono"
            value={device.ipv4}
            onChange={(e) => onChange({ ...device, ipv4: e.target.value })}
          />
        </div>
        <div className="field">
          <label>前缀长度 (0–32)</label>
          <NumericInput
            min={0}
            max={32}
            value={device.prefix_length}
            onValueChange={(prefix_length) => onChange({ ...device, prefix_length })}
          />
        </div>
      </div>

      <div className="field">
        <label>MAC 地址</label>
        <input
          className="mono"
          value={device.mac}
          onChange={(e) => onChange({ ...device, mac: e.target.value })}
        />
      </div>

      {!isLegacy && (
        <div className="field">
          <label>默认网关（可选，跨网段必填）</label>
          <input
            className="mono"
            placeholder="例如 192.0.2.1"
            value={device.gateway ?? ""}
            onChange={(e) =>
              onChange({
                ...device,
                gateway: e.target.value.trim() === "" ? undefined : e.target.value,
              })
            }
          />
        </div>
      )}
    </div>

    <div className="card">
      <div className="panel-section-title">
        <span>主机网卡端口</span>
        <span className="mono small">{device.port.id}</span>
      </div>
      <div className="form-grid">
        <div className="field">
          <label>端口名</label>
          <input
            value={device.port.name}
            onChange={(e) =>
              onChange({
                ...device,
                port: { ...device.port, name: e.target.value },
              })
            }
          />
        </div>
        {!isLegacy && (
          <div className="field">
            <label>IPv4 MTU（字节，可选）</label>
            <NumericInput
              optional
              min={68}
              max={65535}
              placeholder="省略则不检查"
              value={device.port.ipv4_mtu_bytes}
              onValueChange={(ipv4_mtu_bytes) =>
                onChange({
                  ...device,
                  port: { ...device.port, ipv4_mtu_bytes },
                })
              }
            />
          </div>
        )}
      </div>
      <div
        style={{
          display: "flex",
          alignItems: "center",
          justifyContent: "space-between",
          gap: 8,
          marginTop: 6,
        }}
      >
        <div className="small muted">
          连接链路：{connectedLinkId ? <span className="mono">{connectedLinkId}</span> : "未连接"}
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
  </div>
);

const SwitchEditor: React.FC<{
  docKey: string;
  device: SwitchDeviceSpec;
  isLegacy: boolean;
  selectedPortId?: string;
  linkByPort: Map<string, string | null>;
  topologyVlans: number[];
  onSelectPort: (portId: string | undefined) => void;
  onAddPort: () => void;
  onChange: (next: SwitchDeviceSpec) => void;
  onDelete: () => void;
  onSaveAndRerun?: () => void;
  rerunLabel: string;
}> = ({
  docKey,
  device,
  isLegacy,
  selectedPortId,
  linkByPort,
  topologyVlans,
  onSelectPort,
  onAddPort,
  onChange,
  onDelete,
  onSaveAndRerun,
  rerunLabel,
}) => {
  const activePort =
    device.ports.find((p) => p.id === selectedPortId) ?? device.ports[0];

  const updatePort = (portId: string, updater: (p: SwitchPortSpec) => SwitchPortSpec) => {
    onChange({
      ...device,
      ports: device.ports.map((p) => (p.id === portId ? updater(p) : p)),
    });
  };

  return (
    <div>
      <div className="card">
        <div
          style={{
            display: "flex",
            justifyContent: "space-between",
            alignItems: "center",
            marginBottom: 10,
          }}
        >
          <div>
            <span className="pill pill-neutral" style={{ marginRight: 6 }}>
              交换机
            </span>
            <strong className="mono">{device.id}</strong>
          </div>
          <button className="btn btn-xs btn-danger" onClick={onDelete}>
            <Trash2 size={12} /> 删除设备
          </button>
        </div>

        <div className="form-grid">
          <div className="field">
            <label>显示名称</label>
            <input
              value={device.name}
              onChange={(e) => onChange({ ...device, name: e.target.value })}
            />
          </div>
          {!isLegacy && (
            <div className="field">
              <label>STP 桥优先级（4096 倍数）</label>
              <NumericInput
                optional
                step={4096}
                min={0}
                max={61440}
                placeholder="默认 32768"
                value={device.stp_priority}
                onValueChange={(stp_priority) => onChange({ ...device, stp_priority })}
              />
            </div>
          )}
        </div>
      </div>

      <div className="card">
        <div className="panel-section-title">
          <span>端口列表 ({device.ports.length})</span>
          <button className="btn btn-xs" onClick={onAddPort}>
            <Plus size={12} /> 新增端口
          </button>
        </div>

        <div style={{ display: "flex", flexDirection: "column", gap: 4, marginBottom: 10 }}>
          {device.ports.map((p) => {
            const isSel = activePort?.id === p.id;
            const summary =
              p.mode.kind === "access"
                ? `Access · VLAN ${p.mode.vlan}`
                : `Trunk · [${p.mode.allowed_vlans.join(", ")}]${
                    p.mode.native_vlan !== undefined ? ` · Native ${p.mode.native_vlan}` : ""
                  }`;
            return (
              <button
                key={p.id}
                className={`list-item ${isSel ? "selected" : ""}`}
                onClick={() => onSelectPort(p.id)}
              >
                <span className="mono">{p.name}</span>
                <span className="small muted">{summary}</span>
              </button>
            );
          })}
        </div>

        {activePort && (
          <SwitchPortForm
            key={`${docKey}-${activePort.id}`}
            port={activePort}
            isLegacy={isLegacy}
            connectedLinkId={linkByPort.get(activePort.id) ?? null}
            topologyVlans={topologyVlans}
            onChange={(next) => updatePort(activePort.id, () => next)}
            onSaveAndRerun={onSaveAndRerun}
            rerunLabel={rerunLabel}
          />
        )}
      </div>
    </div>
  );
};

const SwitchPortForm: React.FC<{
  port: SwitchPortSpec;
  isLegacy: boolean;
  connectedLinkId: string | null;
  topologyVlans: number[];
  onChange: (next: SwitchPortSpec) => void;
  onSaveAndRerun?: () => void;
  rerunLabel: string;
}> = ({
  port,
  isLegacy,
  connectedLinkId,
  topologyVlans,
  onChange,
  onSaveAndRerun,
  rerunLabel,
}) => {
  const [vlanText, setVlanText] = useState(
    port.mode.kind === "trunk" ? port.mode.allowed_vlans.join(", ") : ""
  );
  const [vlanFocused, setVlanFocused] = useState(false);
  const [ignoredTokens, setIgnoredTokens] = useState<string[]>([]);
  // Keep any VLAN seen in this port session visible so toggling off a unique VLAN doesn't vanish
  const [seenVlans, setSeenVlans] = useState<number[]>(() => [...topologyVlans]);

  useEffect(() => {
    setSeenVlans((prev) => {
      const merged = new Set([...prev, ...topologyVlans]);
      if (port.mode.kind === "trunk") {
        for (const v of port.mode.allowed_vlans) merged.add(v);
      }
      if (merged.size === prev.length) return prev;
      return Array.from(merged).sort((a, b) => a - b);
    });
  }, [topologyVlans, port.mode]);

  const allowedVlansKey =
    port.mode.kind === "trunk" ? port.mode.allowed_vlans.join(", ") : "";
  useEffect(() => {
    if (!vlanFocused && port.mode.kind === "trunk") {
      setVlanText(allowedVlansKey);
      setIgnoredTokens([]);
    }
  }, [allowedVlansKey, port.mode.kind, vlanFocused]);

  const toggleTrunkVlan = (vlan: number) => {
    if (port.mode.kind !== "trunk") return;
    const exists = port.mode.allowed_vlans.includes(vlan);
    const nextList = exists
      ? port.mode.allowed_vlans.filter((v) => v !== vlan)
      : [...port.mode.allowed_vlans, vlan].sort((a, b) => a - b);
    setVlanText(nextList.join(", "));
    setIgnoredTokens([]);
    onChange({
      ...port,
      mode: { ...port.mode, allowed_vlans: nextList },
    });
  };

  return (
    <div
      style={{
        borderTop: "1px solid var(--border-subtle)",
        paddingTop: 10,
      }}
    >
      <div className="panel-section-title">
        <span>当前端口配置</span>
        <span className="mono">{port.id}</span>
      </div>

      <div className="form-grid">
        <div className="field">
          <label>端口名</label>
          <input
            value={port.name}
            onChange={(e) => onChange({ ...port, name: e.target.value })}
          />
        </div>
        <div className="field">
          <label>端口模式</label>
          <select
            value={port.mode.kind}
            onChange={(e) => {
              if (e.target.value === "access") {
                const preserved =
                  port.mode.kind === "trunk"
                    ? port.mode.native_vlan ?? port.mode.allowed_vlans[0] ?? 10
                    : port.mode.vlan;
                onChange({ ...port, mode: { kind: "access", vlan: preserved } });
              } else {
                const preserved =
                  port.mode.kind === "access" ? [port.mode.vlan] : [10, 20];
                setVlanText(preserved.join(", "));
                setIgnoredTokens([]);
                onChange({
                  ...port,
                  mode: { kind: "trunk", allowed_vlans: preserved },
                });
              }
            }}
          >
            <option value="access">Access（接入）</option>
            <option value="trunk">Trunk（干道）</option>
          </select>
        </div>
      </div>

      {port.mode.kind === "access" ? (
        <div className="field">
          <label>Access VLAN (1–4094)</label>
          <NumericInput
            min={1}
            max={4094}
            value={port.mode.vlan}
            onValueChange={(vlan) =>
              onChange({
                ...port,
                mode: { kind: "access", vlan },
              })
            }
          />
        </div>
      ) : (
        <>
          {seenVlans.length > 0 && (
            <div style={{ marginBottom: 8 }}>
              <div className="small muted" style={{ fontWeight: 700, marginBottom: 5 }}>
                快速开关本拓扑 VLAN（拨动即生效）
              </div>
              <div className="vlan-toggle-list">
                {seenVlans.map((vlan) => {
                  const allowed =
                    port.mode.kind === "trunk" &&
                    port.mode.allowed_vlans.includes(vlan);
                  return (
                    <div key={vlan} className="vlan-toggle-row">
                      <span className="mono" style={{ fontWeight: 800 }}>
                        VLAN {vlan}
                      </span>
                      <button
                        type="button"
                        role="switch"
                        aria-checked={allowed}
                        aria-label={`允许 VLAN ${vlan}`}
                        className={`vlan-toggle-btn ${allowed ? "on" : ""}`}
                        onClick={() => toggleTrunkVlan(vlan)}
                      >
                        <span className="vlan-toggle-knob" />
                      </button>
                    </div>
                  );
                })}
              </div>
            </div>
          )}

          <div className="field">
            <label>Trunk 允许 VLAN 列表（支持自定义，逗号分隔如 10, 20）</label>
            <input
              className="mono"
              value={vlanText}
              onFocus={() => setVlanFocused(true)}
              onBlur={() => {
                setVlanFocused(false);
                if (port.mode.kind === "trunk") {
                  setVlanText(port.mode.allowed_vlans.join(", "));
                  setIgnoredTokens([]);
                }
              }}
              onChange={(e) => {
                const raw = e.target.value;
                setVlanText(raw);
                const parsed = parseVlanCsv(raw);
                setIgnoredTokens(parsed.ignored);
                if (port.mode.kind === "trunk") {
                  onChange({
                    ...port,
                    mode: {
                      ...port.mode,
                      allowed_vlans: parsed.valid,
                    },
                  });
                }
              }}
            />
            <span className="small muted">
              生效列表：[{port.mode.allowed_vlans.join(", ")}]
            </span>
            {ignoredTokens.length > 0 && (
              <span className="small" style={{ color: "var(--status-warn)" }}>
                已忽略无效项：{ignoredTokens.join(", ")}（仅支持 1–4094 或短区间）
              </span>
            )}
          </div>

          {!isLegacy && (
            <div className="field">
              <label>Native VLAN（可选，省略则仅收发带标签帧）</label>
              <NumericInput
                optional
                min={1}
                max={4094}
                placeholder="省略表示无 Native VLAN"
                value={port.mode.native_vlan}
                onValueChange={(native_vlan) => {
                  if (port.mode.kind !== "trunk") return;
                  onChange({
                    ...port,
                    mode: {
                      ...port.mode,
                      native_vlan,
                    },
                  });
                }}
              />
            </div>
          )}
        </>
      )}

      {!isLegacy && (
        <div className="field">
          <label>STP 端口路径开销（stp_cost，正整数，默认 1）</label>
          <NumericInput
            optional
            min={1}
            max={200000000}
            placeholder="默认 1"
            value={port.stp_cost}
            onValueChange={(stp_cost) => onChange({ ...port, stp_cost })}
          />
        </div>
      )}

      <div
        style={{
          display: "flex",
          alignItems: "center",
          justifyContent: "space-between",
          gap: 8,
          marginTop: 6,
        }}
      >
        <div className="small muted">
          连接链路：{connectedLinkId ? <span className="mono">{connectedLinkId}</span> : "未连接"}
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
  );
};

const RouterEditor: React.FC<{
  device: RouterDeviceSpec;
  isLegacy: boolean;
  selectedPortId?: string;
  linkByPort: Map<string, string | null>;
  routerPeerPortIds: Set<string>;
  onSelectPort: (portId: string | undefined) => void;
  onAddPort: () => void;
  onChange: (next: RouterDeviceSpec) => void;
  onDelete: () => void;
  onSaveAndRerun?: () => void;
  rerunLabel: string;
}> = ({
  device,
  isLegacy,
  selectedPortId,
  linkByPort,
  routerPeerPortIds,
  onSelectPort,
  onAddPort,
  onChange,
  onDelete,
  onSaveAndRerun,
  rerunLabel,
}) => {
  const activePort =
    device.ports.find((p) => p.id === selectedPortId) ?? device.ports[0];

  const updatePort = (portId: string, updater: (p: IpPortSpec) => IpPortSpec) => {
    onChange({
      ...device,
      ports: device.ports.map((p) => (p.id === portId ? updater(p) : p)),
    });
  };

  const usedPeerPorts = new Set((device.link_state?.peers ?? []).map((p) => p.port));
  const nextPeerCandidate =
    device.ports.find(
      (p) => routerPeerPortIds.has(p.id) && !usedPeerPorts.has(p.id)
    ) ??
    device.ports.find((p) => Boolean(linkByPort.get(p.id)) && !usedPeerPorts.has(p.id)) ??
    device.ports.find((p) => !usedPeerPorts.has(p.id));

  return (
    <div>
      <div className="card">
        <div
          style={{
            display: "flex",
            justifyContent: "space-between",
            alignItems: "center",
            marginBottom: 10,
          }}
        >
          <div>
            <span className="pill pill-neutral" style={{ marginRight: 6 }}>
              路由器
            </span>
            <strong className="mono">{device.id}</strong>
          </div>
          <button className="btn btn-xs btn-danger" onClick={onDelete}>
            <Trash2 size={12} /> 删除设备
          </button>
        </div>

        <div className="field">
          <label>显示名称</label>
          <input
            value={device.name}
            onChange={(e) => onChange({ ...device, name: e.target.value })}
          />
        </div>
      </div>

      {/* Router Ports */}
      <div className="card">
        <div className="panel-section-title">
          <span>三层接口 ({device.ports.length})</span>
          <button className="btn btn-xs" onClick={onAddPort}>
            <Plus size={12} /> 新增接口
          </button>
        </div>
        <div style={{ display: "flex", flexDirection: "column", gap: 4, marginBottom: 10 }}>
          {device.ports.map((p) => (
            <button
              key={p.id}
              className={`list-item ${activePort?.id === p.id ? "selected" : ""}`}
              onClick={() => onSelectPort(p.id)}
            >
              <span className="mono">{p.name}</span>
              <span className="small muted mono">
                {p.ipv4}/{p.prefix_length}
                {p.ipv4_mtu_bytes ? ` · MTU ${p.ipv4_mtu_bytes}` : ""}
                {p.ipv4_acl ? " · ACL" : ""}
              </span>
            </button>
          ))}
        </div>

        {activePort && (
          <RouterPortForm
            port={activePort}
            isLegacy={isLegacy}
            connectedLinkId={linkByPort.get(activePort.id) ?? null}
            onChange={(next) => updatePort(activePort.id, () => next)}
            onSaveAndRerun={onSaveAndRerun}
            rerunLabel={rerunLabel}
          />
        )}
      </div>

      {/* Static Routes */}
      <div className="card">
        <div className="panel-section-title">
          <span>静态 IPv4 路由 ({device.routes.length})</span>
          <button
            className="btn btn-xs"
            disabled={device.ports.length === 0}
            onClick={() => {
              const egressPort = device.ports[0];
              onChange({
                ...device,
                routes: [
                  ...device.routes,
                  {
                    network: "192.0.2.0",
                    prefix_length: 24,
                    egress: egressPort?.id ?? "",
                    next_hop: suggestNeighborIp(egressPort),
                  },
                ],
              });
            }}
          >
            <Plus size={12} /> 添加路由
          </button>
        </div>
        {device.routes.length === 0 ? (
          <div className="small muted">暂无静态路由（仅使用直连路由或动态路由）。</div>
        ) : (
          device.routes.map((route, idx) => (
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
                  <label>目的网段</label>
                  <input
                    className="mono"
                    value={route.network}
                    onChange={(e) => {
                      const routes = [...device.routes];
                      routes[idx] = { ...route, network: e.target.value };
                      onChange({ ...device, routes });
                    }}
                  />
                </div>
                <div className="field">
                  <label>前缀长度</label>
                  <NumericInput
                    min={0}
                    max={32}
                    value={route.prefix_length}
                    onValueChange={(prefix_length) => {
                      const routes = [...device.routes];
                      routes[idx] = { ...route, prefix_length };
                      onChange({ ...device, routes });
                    }}
                  />
                </div>
                <div className="field">
                  <label>出接口</label>
                  <select
                    value={route.egress}
                    onChange={(e) => {
                      const routes = [...device.routes];
                      routes[idx] = { ...route, egress: e.target.value };
                      onChange({ ...device, routes });
                    }}
                  >
                    {!device.ports.some((p) => p.id === route.egress) && (
                      <option value={route.egress}>
                        {route.egress || "— 请选择接口 —"}
                      </option>
                    )}
                    {device.ports.map((p) => (
                      <option key={p.id} value={p.id}>
                        {p.name} ({p.id})
                      </option>
                    ))}
                  </select>
                </div>
                <div className="field">
                  <label>下一跳 IPv4</label>
                  <input
                    className="mono"
                    value={route.next_hop}
                    onChange={(e) => {
                      const routes = [...device.routes];
                      routes[idx] = { ...route, next_hop: e.target.value };
                      onChange({ ...device, routes });
                    }}
                  />
                </div>
              </div>
              <button
                className="btn btn-xs btn-danger"
                onClick={() =>
                  onChange({
                    ...device,
                    routes: device.routes.filter((_, i) => i !== idx),
                  })
                }
              >
                <Trash2 size={11} /> 删除此路由
              </button>
            </div>
          ))
        )}
      </div>

      {/* Dynamic Link-State Routing */}
      {!isLegacy && (
        <div className="card">
          <div className="panel-section-title">
            <span>点到点动态路由（link_state）</span>
            {device.link_state ? (
              <button
                className="btn btn-xs btn-danger"
                onClick={() => onChange({ ...device, link_state: undefined })}
              >
                关闭
              </button>
            ) : (
              <button
                className="btn btn-xs"
                onClick={() =>
                  onChange({
                    ...device,
                    link_state: { peers: [], advertise: [] },
                  })
                }
              >
                <Plus size={12} /> 启用
              </button>
            )}
          </div>

          {device.link_state ? (
            <>
              <div style={{ marginBottom: 10 }}>
                <div
                  style={{
                    display: "flex",
                    justifyContent: "space-between",
                    alignItems: "center",
                    marginBottom: 4,
                  }}
                >
                  <span className="small" style={{ fontWeight: 600 }}>
                    邻接接口与代价 (peers)
                  </span>
                  <button
                    className="btn btn-xs"
                    disabled={!nextPeerCandidate}
                    title={
                      nextPeerCandidate
                        ? "添加一条新的动态路由邻接接口"
                        : "所有接口均已在邻接列表中"
                    }
                    onClick={() => {
                      if (!nextPeerCandidate) return;
                      onChange({
                        ...device,
                        link_state: {
                          ...device.link_state!,
                          peers: [
                            ...device.link_state!.peers,
                            { port: nextPeerCandidate.id, cost: 10 },
                          ],
                        },
                      });
                    }}
                  >
                    <Plus size={11} /> 添加邻接
                  </button>
                </div>
                {device.link_state.peers.map((peer, idx) => (
                  <div
                    key={idx}
                    style={{
                      display: "grid",
                      gridTemplateColumns: "minmax(0, 1fr) 72px 28px",
                      gap: 6,
                      marginBottom: 6,
                      alignItems: "center",
                    }}
                  >
                    <select
                      style={{
                        width: "100%",
                        minWidth: 0,
                        height: 28,
                        borderRadius: 5,
                        border: "1px solid var(--border-strong)",
                      }}
                      value={peer.port}
                      onChange={(e) => {
                        const peers = [...device.link_state!.peers];
                        peers[idx] = { ...peer, port: e.target.value };
                        onChange({
                          ...device,
                          link_state: { ...device.link_state!, peers },
                        });
                      }}
                    >
                      {!device.ports.some((p) => p.id === peer.port) && (
                        <option value={peer.port}>{peer.port || "—"}</option>
                      )}
                      {device.ports.map((p) => (
                        <option key={p.id} value={p.id}>
                          {p.name} ({p.id})
                        </option>
                      ))}
                    </select>
                    <NumericInput
                      min={1}
                      max={65535}
                      style={{
                        width: "100%",
                        minWidth: 0,
                        height: 28,
                        padding: "0 6px",
                        borderRadius: 5,
                        border: "1px solid var(--border-strong)",
                      }}
                      value={peer.cost}
                      onValueChange={(cost) => {
                        const peers = [...device.link_state!.peers];
                        peers[idx] = { ...peer, cost };
                        onChange({
                          ...device,
                          link_state: { ...device.link_state!, peers },
                        });
                      }}
                    />
                    <button
                      className="btn btn-xs btn-danger"
                      onClick={() =>
                        onChange({
                          ...device,
                          link_state: {
                            ...device.link_state!,
                            peers: device.link_state!.peers.filter((_, i) => i !== idx),
                          },
                        })
                      }
                    >
                      <Trash2 size={11} />
                    </button>
                  </div>
                ))}
              </div>

              <div>
                <div className="small" style={{ fontWeight: 600, marginBottom: 4 }}>
                  发布直连网段接口 (advertise)
                </div>
                <div style={{ display: "flex", flexWrap: "wrap", gap: 6 }}>
                  {device.ports.map((p) => {
                    const checked = device.link_state!.advertise.includes(p.id);
                    const connected = Boolean(linkByPort.get(p.id));
                    const disabled = !connected && !checked;
                    return (
                      <label
                        key={p.id}
                        title={
                          disabled
                            ? "该接口尚未连接链路，连线后可勾选发布"
                            : "通过动态路由向邻接路由器发布此直连网段"
                        }
                        style={{
                          display: "inline-flex",
                          alignItems: "center",
                          gap: 4,
                          padding: "3px 7px",
                          borderRadius: 5,
                          border: "1px solid var(--border-subtle)",
                          background: checked ? "var(--accent-primary-soft)" : "var(--bg-subtle)",
                          opacity: disabled ? 0.5 : 1,
                          cursor: disabled ? "not-allowed" : "pointer",
                          fontSize: 12,
                        }}
                      >
                        <input
                          type="checkbox"
                          disabled={disabled}
                          checked={checked}
                          onChange={(e) => {
                            const adv = e.target.checked
                              ? [...device.link_state!.advertise, p.id]
                              : device.link_state!.advertise.filter((id) => id !== p.id);
                            onChange({
                              ...device,
                              link_state: { ...device.link_state!, advertise: adv },
                            });
                          }}
                        />
                        <span className="mono">{p.name}</span>
                      </label>
                    );
                  })}
                </div>
              </div>
            </>
          ) : (
            <div className="small muted">未启用动态路由。</div>
          )}
        </div>
      )}
    </div>
  );
};

const RouterPortForm: React.FC<{
  port: IpPortSpec;
  isLegacy: boolean;
  connectedLinkId: string | null;
  onChange: (next: IpPortSpec) => void;
  onSaveAndRerun?: () => void;
  rerunLabel: string;
}> = ({ port, isLegacy, connectedLinkId, onChange, onSaveAndRerun, rerunLabel }) => {
  const updateAcl = (
    dir: "ingress" | "egress",
    rules: AclRuleSpec[] | undefined
  ) => {
    const nextAcl = { ...(port.ipv4_acl ?? {}), [dir]: rules };
    if (nextAcl.ingress === undefined && nextAcl.egress === undefined) {
      onChange({ ...port, ipv4_acl: undefined });
    } else {
      onChange({ ...port, ipv4_acl: nextAcl });
    }
  };

  return (
    <div style={{ borderTop: "1px solid var(--border-subtle)", paddingTop: 10 }}>
      <div className="panel-section-title">
        <span>接口详情</span>
        <span className="mono">{port.id}</span>
      </div>
      <div className="form-grid">
        <div className="field">
          <label>接口名</label>
          <input
            value={port.name}
            onChange={(e) => onChange({ ...port, name: e.target.value })}
          />
        </div>
        <div className="field">
          <label>MAC 地址</label>
          <input
            className="mono"
            value={port.mac}
            onChange={(e) => onChange({ ...port, mac: e.target.value })}
          />
        </div>
        <div className="field">
          <label>IPv4 地址</label>
          <input
            className="mono"
            value={port.ipv4}
            onChange={(e) => onChange({ ...port, ipv4: e.target.value })}
          />
        </div>
        <div className="field">
          <label>前缀长度</label>
          <NumericInput
            min={0}
            max={32}
            value={port.prefix_length}
            onValueChange={(prefix_length) => onChange({ ...port, prefix_length })}
          />
        </div>
      </div>

      {!isLegacy && (
        <>
          <div className="field">
            <label>IPv4 出口 MTU（字节，68–65535，可选）</label>
            <NumericInput
              optional
              min={68}
              max={65535}
              placeholder="省略则不检查 MTU"
              value={port.ipv4_mtu_bytes}
              onValueChange={(ipv4_mtu_bytes) => onChange({ ...port, ipv4_mtu_bytes })}
            />
          </div>

          <div style={{ marginTop: 8 }}>
            <div className="panel-section-title">
              <span style={{ display: "inline-flex", alignItems: "center", gap: 4 }}>
                <Shield size={12} /> IPv4 标准 ACL（按源前缀有序匹配）
              </span>
            </div>
            {(["ingress", "egress"] as const).map((dir) => {
              const rules = port.ipv4_acl?.[dir];
              return (
                <div
                  key={dir}
                  style={{
                    border: "1px solid var(--border-subtle)",
                    borderRadius: 6,
                    padding: 8,
                    marginBottom: 6,
                    background: "var(--bg-subtle)",
                  }}
                >
                  <div
                    style={{
                      display: "flex",
                      justifyContent: "space-between",
                      alignItems: "center",
                      marginBottom: 6,
                    }}
                  >
                    <strong className="small">
                      {dir === "ingress" ? "入口 (ingress)" : "出口 (egress)"}
                      {rules !== undefined ? ` · ${rules.length} 条（末尾隐式拒绝）` : " · 未启用"}
                    </strong>
                    <div style={{ display: "flex", gap: 4 }}>
                      {rules !== undefined ? (
                        <>
                          <button
                            className="btn btn-xs"
                            onClick={() =>
                              updateAcl(dir, [
                                ...rules,
                                { action: "permit", network: "0.0.0.0", prefix_length: 0 },
                              ])
                            }
                          >
                            <Plus size={11} /> 规则
                          </button>
                          <button
                            className="btn btn-xs btn-danger"
                            onClick={() => updateAcl(dir, undefined)}
                          >
                            关闭
                          </button>
                        </>
                      ) : (
                        <button
                          className="btn btn-xs"
                          onClick={() =>
                            updateAcl(dir, [
                              { action: "permit", network: "0.0.0.0", prefix_length: 0 },
                            ])
                          }
                        >
                          <Plus size={11} /> 启用
                        </button>
                      )}
                    </div>
                  </div>
                  {rules?.map((rule, idx) => (
                    <div
                      key={idx}
                      style={{
                        display: "grid",
                        gridTemplateColumns: "64px minmax(0, 1fr) 44px auto",
                        gap: 4,
                        marginBottom: 4,
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
                        value={rule.action}
                        onChange={(e) => {
                          const next = [...rules];
                          next[idx] = {
                            ...rule,
                            action: e.target.value as "permit" | "deny",
                          };
                          updateAcl(dir, next);
                        }}
                      >
                        <option value="permit">permit</option>
                        <option value="deny">deny</option>
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
                        value={rule.network}
                        onChange={(e) => {
                          const next = [...rules];
                          next[idx] = { ...rule, network: e.target.value };
                          updateAcl(dir, next);
                        }}
                      />
                      <NumericInput
                        min={0}
                        max={32}
                        style={{
                          width: "100%",
                          minWidth: 0,
                          height: 26,
                          padding: "0 4px",
                          borderRadius: 4,
                          border: "1px solid var(--border-strong)",
                        }}
                        value={rule.prefix_length}
                        onValueChange={(prefix_length) => {
                          const next = [...rules];
                          next[idx] = { ...rule, prefix_length };
                          updateAcl(dir, next);
                        }}
                      />
                      <div style={{ display: "flex", gap: 2 }}>
                        <button
                          className="btn btn-xs"
                          title="上移"
                          disabled={idx === 0}
                          onClick={() => {
                            const next = [...rules];
                            [next[idx - 1], next[idx]] = [next[idx], next[idx - 1]];
                            updateAcl(dir, next);
                          }}
                        >
                          <ArrowUp size={10} />
                        </button>
                        <button
                          className="btn btn-xs"
                          title="下移"
                          disabled={idx === rules.length - 1}
                          onClick={() => {
                            const next = [...rules];
                            [next[idx + 1], next[idx]] = [next[idx], next[idx + 1]];
                            updateAcl(dir, next);
                          }}
                        >
                          <ArrowDown size={10} />
                        </button>
                        <button
                          className="btn btn-xs btn-danger"
                          title="删除"
                          onClick={() =>
                            updateAcl(
                              dir,
                              rules.filter((_, i) => i !== idx)
                            )
                          }
                        >
                          <Trash2 size={10} />
                        </button>
                      </div>
                    </div>
                  ))}
                </div>
              );
            })}
          </div>
        </>
      )}

      <div
        style={{
          display: "flex",
          alignItems: "center",
          justifyContent: "space-between",
          gap: 8,
          marginTop: 6,
        }}
      >
        <div className="small muted">
          连接链路：{connectedLinkId ? <span className="mono">{connectedLinkId}</span> : "未连接"}
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
  );
};
