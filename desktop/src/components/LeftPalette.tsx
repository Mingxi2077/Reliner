import React, { useEffect, useRef } from "react";
import { BookOpen, Cpu, Plus, Sliders, X } from "lucide-react";
import type { DeviceSpec, NativeLabDocument } from "../model/document";
import type { ExampleSummary } from "../protocol/types";
import type { CanvasToolMode, SelectionTarget } from "../canvas/TopologyCanvas";
import {
  getExampleCategory,
  SketchHostSvg,
  SketchProbeSvg,
  SketchRouterSvg,
  SketchSwitchSvg,
  SketchWireSvg,
} from "./SketchIcons";

interface LeftPaletteProps {
  width: number;
  doc: NativeLabDocument | null;
  section: "palette" | "examples";
  onChangeSection: (next: "palette" | "examples") => void;
  selection: SelectionTarget;
  toolMode: CanvasToolMode;
  examples: ExampleSummary[];
  solvedExampleIds: Set<string>;
  onAddDevice: (kind: DeviceSpec["kind"]) => void;
  onToggleConnect: () => void;
  onQuickProbe: () => void;
  onSelect: (target: SelectionTarget) => void;
  onOpenExample: (fileName: string) => void;
}

function startDeviceDrag(e: React.DragEvent, kind: DeviceSpec["kind"]) {
  e.dataTransfer.setData("application/x-reliner-device", kind);
  e.dataTransfer.effectAllowed = "copy";
}

export const LeftPalette: React.FC<LeftPaletteProps> = ({
  width,
  doc,
  section,
  onChangeSection,
  selection,
  toolMode,
  examples,
  solvedExampleIds,
  onAddDevice,
  onToggleConnect,
  onQuickProbe,
  onSelect,
  onOpenExample,
}) => {
  const isLegacy = doc?.format_version === 1;

  return (
    <aside className="left-panel" style={{ width }}>
      <div className="right-tabs">
        <button
          className={`right-tab ${section === "palette" ? "active" : ""}`}
          style={{ flex: 1, justifyContent: "center" }}
          onClick={() => onChangeSection("palette")}
        >
          <Cpu size={13} /> 绘件箱
        </button>
        <button
          className={`right-tab ${section === "examples" ? "active" : ""}`}
          style={{ flex: 1, justifyContent: "center" }}
          onClick={() => onChangeSection("examples")}
        >
          <BookOpen size={13} /> 关卡 ({examples.length})
        </button>
      </div>

      <div className="panel-scroll">
        {section === "palette" ? (
          <>
            <div className="panel-section">
              <div className="panel-section-title">
                <span>拖上画纸或点击画入</span>
              </div>
              <div className="palette-grid">
                <button
                  className="palette-card"
                  disabled={!doc}
                  draggable={Boolean(doc)}
                  onDragStart={(e) => startDeviceDrag(e, "host")}
                  onClick={() => onAddDevice("host")}
                >
                  <span className="palette-sketch-icon">
                    <SketchHostSvg size={24} />
                  </span>
                  <div style={{ flex: 1 }}>
                    <div style={{ fontWeight: 800 }}>主机 Host</div>
                    <div className="small muted">IPv4 · 网关 · MTU</div>
                  </div>
                  <Plus size={14} className="muted" />
                </button>

                <button
                  className="palette-card"
                  disabled={!doc}
                  draggable={Boolean(doc)}
                  onDragStart={(e) => startDeviceDrag(e, "switch")}
                  onClick={() => onAddDevice("switch")}
                >
                  <span className="palette-sketch-icon">
                    <SketchSwitchSvg size={24} />
                  </span>
                  <div style={{ flex: 1 }}>
                    <div style={{ fontWeight: 800 }}>交换机 Switch</div>
                    <div className="small muted">VLAN · Trunk · STP</div>
                  </div>
                  <Plus size={14} className="muted" />
                </button>

                <button
                  className="palette-card"
                  disabled={!doc || isLegacy}
                  draggable={Boolean(doc) && !isLegacy}
                  title={isLegacy ? "格式 1 仅支持主机与交换机" : "拖上画纸或点击放置路由器"}
                  onDragStart={(e) => startDeviceDrag(e, "router")}
                  onClick={() => onAddDevice("router")}
                >
                  <span className="palette-sketch-icon">
                    <SketchRouterSvg size={24} />
                  </span>
                  <div style={{ flex: 1 }}>
                    <div style={{ fontWeight: 800 }}>路由器 Router</div>
                    <div className="small muted">
                      {isLegacy ? "格式 1 不支持路由器" : "静态/动态路由 · ACL"}
                    </div>
                  </div>
                  <Plus size={14} className="muted" />
                </button>
              </div>
            </div>

            {doc && (
              <>
                <div className="panel-section">
                  <div className="panel-section-title">
                    <span>画布交互工具</span>
                  </div>
                  <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr", gap: 6 }}>
                    <button
                      className={`btn btn-sm ${toolMode.kind === "connect" ? "btn-butter" : ""}`}
                      onClick={onToggleConnect}
                      title="拉网线连接两台设备"
                    >
                      <SketchWireSvg size={16} /> 拉网线
                    </button>
                    <button
                      className={`btn btn-sm ${toolMode.kind === "probe" ? "btn-butter" : ""}`}
                      onClick={onQuickProbe}
                      title="在画布上点选源/目的主机测连通"
                    >
                      <SketchProbeSvg size={16} /> 测连通
                    </button>
                  </div>
                </div>

                <div className="panel-section">
                  <div className="panel-section-title">
                    <span>场景与探测</span>
                  </div>
                  <button
                    className={`list-item ${selection.kind === "scenario" ? "selected" : ""}`}
                    onClick={() => onSelect({ kind: "scenario" })}
                  >
                    <span>
                      <Sliders size={13} /> 探测与定时事件
                    </span>
                    <span className="small muted mono">
                      {doc.format_version === 1
                        ? "1 发"
                        : `${doc.scenario?.probes.length ?? 0} 发`}
                    </span>
                  </button>
                </div>

                <div className="panel-section">
                  <div className="panel-section-title">
                    <span>画布设备 ({doc.devices.length})</span>
                  </div>
                  {doc.devices.map((d) => {
                    const isSel =
                      (selection.kind === "device" || selection.kind === "port") &&
                      selection.deviceId === d.id;
                    return (
                      <button
                        key={d.id}
                        className={`list-item ${isSel ? "selected" : ""}`}
                        onClick={() => onSelect({ kind: "device", deviceId: d.id })}
                      >
                        <span>{d.name || d.id}</span>
                        <span className="small muted mono">{d.kind}</span>
                      </button>
                    );
                  })}
                </div>

                <div className="panel-section">
                  <div className="panel-section-title">
                    <span>物理网线 ({doc.links.length})</span>
                  </div>
                  {doc.links.map((l) => {
                    const isSel =
                      selection.kind === "link" && selection.linkId === l.id;
                    return (
                      <button
                        key={l.id}
                        className={`list-item ${isSel ? "selected" : ""}`}
                        onClick={() => onSelect({ kind: "link", linkId: l.id })}
                      >
                        <span className="mono">{l.id}</span>
                        <span className="small muted">
                          {l.standby_for ? "备用" : `${l.delay_ns}ns`}
                        </span>
                      </button>
                    );
                  })}
                </div>
              </>
            )}
          </>
        ) : (
          <ExamplesListBody
            examples={examples}
            solvedExampleIds={solvedExampleIds}
            onOpenExample={onOpenExample}
          />
        )}
      </div>
    </aside>
  );
};

export const ExamplesListBody: React.FC<{
  examples: ExampleSummary[];
  solvedExampleIds: Set<string>;
  onOpenExample: (fileName: string) => void;
}> = ({ examples, solvedExampleIds, onOpenExample }) => (
  <div>
    <div className="small muted" style={{ marginBottom: 8 }}>
      点击打开排障关卡副本（不会覆盖原示例文件）：
    </div>
    {examples.length === 0 ? (
      <div className="small muted">未发现内置示例目录。</div>
    ) : (
      examples.map((ex) => {
        const meta = getExampleCategory(ex.id);
        const solved = solvedExampleIds.has(ex.id);
        return (
          <button
            key={ex.file_name}
            className={`example-card ${solved ? "solved" : ""}`}
            onClick={() => onOpenExample(ex.file_name)}
          >
            <div className="example-head">
              <span style={{ display: "inline-flex", gap: 5, alignItems: "center" }}>
                <span className="level-num">#{meta.num}</span>
                <span className={`level-tag ${meta.tone}`}>{meta.tag}</span>
              </span>
              {solved && <span className="solved-stamp">✓ 通关</span>}
            </div>
            <div className="example-title">{ex.title}</div>
            <div className="example-desc">{ex.summary}</div>
            <div className="small muted mono" style={{ marginTop: 4 }}>
              {ex.device_count} 设备 · {ex.probe_count} 探测 · v{ex.format_version}
            </div>
          </button>
        );
      })
    )}
  </div>
);

/** Iconic B1-Beta Floating Bottom Pill Dock (cute line-art tools + scenario + levels) */
export const PlayfulBottomDock: React.FC<{
  hasDoc: boolean;
  isLegacy?: boolean;
  toolMode: CanvasToolMode;
  examplesCount: number;
  examplesOpen: boolean;
  onAddDevice: (kind: DeviceSpec["kind"]) => void;
  onToggleConnect: () => void;
  onQuickProbe: () => void;
  onOpenScenario: () => void;
  onToggleExamples: () => void;
}> = ({
  hasDoc,
  isLegacy = false,
  toolMode,
  examplesCount,
  examplesOpen,
  onAddDevice,
  onToggleConnect,
  onQuickProbe,
  onOpenScenario,
  onToggleExamples,
}) => (
  <div className="sketch-dock" role="toolbar" aria-label="简笔工具栏">
    <button
      className="dock-icon-btn"
      disabled={!hasDoc}
      draggable={hasDoc}
      onDragStart={(e) => startDeviceDrag(e, "host")}
      onClick={() => onAddDevice("host")}
      title="拖上画纸或点击放置主机"
    >
      <SketchHostSvg size={24} />
      <span>主机</span>
    </button>
    <button
      className="dock-icon-btn"
      disabled={!hasDoc}
      draggable={hasDoc}
      onDragStart={(e) => startDeviceDrag(e, "switch")}
      onClick={() => onAddDevice("switch")}
      title="拖上画纸或点击放置交换机"
    >
      <SketchSwitchSvg size={24} />
      <span>交换机</span>
    </button>
    <button
      className="dock-icon-btn"
      disabled={!hasDoc || isLegacy}
      draggable={hasDoc && !isLegacy}
      onDragStart={(e) => startDeviceDrag(e, "router")}
      onClick={() => onAddDevice("router")}
      title={isLegacy ? "格式 1 仅支持主机与交换机" : "拖上画纸或点击放置路由器"}
    >
      <SketchRouterSvg size={24} />
      <span>路由器</span>
    </button>
    <button
      className={`dock-icon-btn ${toolMode.kind === "connect" ? "active" : ""}`}
      disabled={!hasDoc}
      onClick={onToggleConnect}
      title="拉网线连接两台设备"
    >
      <SketchWireSvg size={24} />
      <span>网线</span>
    </button>
    <button
      className={`dock-icon-btn ${toolMode.kind === "probe" ? "active" : ""}`}
      disabled={!hasDoc}
      onClick={onQuickProbe}
      title="点选源主机和目的主机测连通"
    >
      <SketchProbeSvg size={24} />
      <span>测连通</span>
    </button>
    <div className="dock-sep" />
    <button
      className="dock-icon-btn"
      disabled={!hasDoc}
      onClick={onOpenScenario}
      title="编辑场景时钟、探测清单与定时链路动作"
    >
      <Sliders size={19} strokeWidth={2.1} />
      <span>场景</span>
    </button>
    <button
      className={`dock-icon-btn ${examplesOpen ? "active" : ""}`}
      onClick={onToggleExamples}
      title={`浏览 ${examplesCount} 个内置排障关卡`}
    >
      <BookOpen size={19} strokeWidth={2.1} />
      <span>关卡</span>
    </button>
  </div>
);

/** Slim Left Icon Rail for Focus (2-column) mode */
export const FocusIconRail: React.FC<{
  hasDoc: boolean;
  isLegacy?: boolean;
  examplesOpen: boolean;
  toolMode: CanvasToolMode;
  onAddDevice: (kind: DeviceSpec["kind"]) => void;
  onToggleConnect: () => void;
  onQuickProbe: () => void;
  onOpenScenario: () => void;
  onToggleExamples: () => void;
}> = ({
  hasDoc,
  isLegacy = false,
  examplesOpen,
  toolMode,
  onAddDevice,
  onToggleConnect,
  onQuickProbe,
  onOpenScenario,
  onToggleExamples,
}) => (
  <aside className="left-rail" aria-label="快速简笔工具栏">
    <button
      className="rail-btn"
      disabled={!hasDoc}
      draggable={hasDoc}
      onDragStart={(e) => startDeviceDrag(e, "host")}
      onClick={() => onAddDevice("host")}
      title="放置主机"
    >
      <SketchHostSvg size={24} />
    </button>
    <button
      className="rail-btn"
      disabled={!hasDoc}
      draggable={hasDoc}
      onDragStart={(e) => startDeviceDrag(e, "switch")}
      onClick={() => onAddDevice("switch")}
      title="放置交换机"
    >
      <SketchSwitchSvg size={24} />
    </button>
    <button
      className="rail-btn"
      disabled={!hasDoc || isLegacy}
      draggable={hasDoc && !isLegacy}
      onDragStart={(e) => startDeviceDrag(e, "router")}
      onClick={() => onAddDevice("router")}
      title={isLegacy ? "格式 1 仅支持主机与交换机" : "放置路由器"}
    >
      <SketchRouterSvg size={24} />
    </button>
    <div className="rail-divider" />
    <button
      className="rail-btn"
      disabled={!hasDoc}
      style={toolMode.kind === "connect" ? { background: "var(--pastel-sky-deep)" } : undefined}
      onClick={onToggleConnect}
      title="拉网线"
    >
      <SketchWireSvg size={22} />
    </button>
    <button
      className="rail-btn"
      disabled={!hasDoc}
      style={toolMode.kind === "probe" ? { background: "var(--pastel-butter)" } : undefined}
      onClick={onQuickProbe}
      title="测连通"
    >
      <SketchProbeSvg size={22} />
    </button>
    <button
      className="rail-btn"
      disabled={!hasDoc}
      onClick={onOpenScenario}
      title="场景与探测参数"
    >
      <Sliders size={18} />
    </button>
    <button
      className="rail-btn"
      style={examplesOpen ? { background: "var(--pastel-butter)" } : undefined}
      onClick={onToggleExamples}
      title="内置排障关卡"
    >
      <BookOpen size={18} />
    </button>
  </aside>
);

export const ExamplesPopover: React.FC<{
  layoutMode: "playful" | "focus";
  examples: ExampleSummary[];
  solvedExampleIds: Set<string>;
  onOpenExample: (fileName: string) => void;
  onClose: () => void;
}> = ({ layoutMode, examples, solvedExampleIds, onOpenExample, onClose }) => {
  const popoverRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    const onDown = (e: MouseEvent) => {
      if (popoverRef.current && !popoverRef.current.contains(e.target as Node)) {
        onClose();
      }
    };
    // Defer listener attach one tick so the opening toggle click doesn't immediately close it
    const timer = window.setTimeout(() => {
      window.addEventListener("mousedown", onDown);
    }, 0);
    return () => {
      window.clearTimeout(timer);
      window.removeEventListener("mousedown", onDown);
    };
  }, [onClose]);

  return (
    <div
      ref={popoverRef}
      className="examples-popover"
      style={{
        left: layoutMode === "focus" ? 70 : 14,
        top: layoutMode === "playful" ? 56 : 14,
      }}
    >
      <div
        style={{
          display: "flex",
          alignItems: "center",
          justifyContent: "space-between",
          padding: "10px 13px",
          background: "var(--paper-bg)",
          borderBottom: "1.5px solid #e4dbcb",
          fontWeight: 800,
        }}
      >
        <span style={{ display: "inline-flex", alignItems: "center", gap: 6 }}>
          <BookOpen size={14} /> 排障关卡 ({examples.length})
        </span>
        <button className="tab-close" onClick={onClose}>
          <X size={14} />
        </button>
      </div>
      <div className="panel-scroll">
        <ExamplesListBody
          examples={examples}
          solvedExampleIds={solvedExampleIds}
          onOpenExample={(f) => {
            onOpenExample(f);
            onClose();
          }}
        />
      </div>
    </div>
  );
};
