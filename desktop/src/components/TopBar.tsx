import React, { useEffect, useRef, useState } from "react";
import {
  BookOpen,
  CheckCircle2,
  Columns2,
  Columns3,
  FilePlus2,
  FolderOpen,
  GitCompare,
  Pause,
  Play,
  RotateCcw,
  Save,
  SkipForward,
  Sparkles,
  Square,
  X,
} from "lucide-react";
import type { DocumentInfo, RunStatus } from "../protocol/types";
import { SketchBrandMark } from "./SketchIcons";

export type WorkspaceLayoutMode = "playful" | "workbench" | "focus";

export interface HeaderTabItem {
  documentId: string;
  info: DocumentInfo;
  solved?: boolean;
}

interface TopBarProps {
  tabs: HeaderTabItem[];
  activeDocumentId: string | null;
  hasDoc: boolean;
  isDirty: boolean;
  canCompare: boolean;
  activeRunStatus: RunStatus | null;
  connected: boolean;
  layoutMode: WorkspaceLayoutMode;
  onSelectTab: (documentId: string) => void;
  onCloseTab: (documentId: string) => void;
  onChangeLayoutMode: (mode: WorkspaceLayoutMode) => void;
  onNewLab: () => void;
  onOpenFile: () => void;
  onOpenExamplesList: () => void;
  onSave: () => void;
  onSaveAs: () => void;
  onSaveAndRerun: () => void;
  onValidate: () => void;
  onRunAuto: () => void;
  onRunPaused: () => void;
  onControlRun: (control: "pause" | "resume" | "step" | "cancel") => void;
  onOpenCompare: () => void;
}

export const TopBar: React.FC<TopBarProps> = ({
  tabs,
  activeDocumentId,
  hasDoc,
  isDirty,
  canCompare,
  activeRunStatus,
  connected,
  layoutMode,
  onSelectTab,
  onCloseTab,
  onChangeLayoutMode,
  onNewLab,
  onOpenFile,
  onOpenExamplesList,
  onSave,
  onSaveAs,
  onSaveAndRerun,
  onValidate,
  onRunAuto,
  onRunPaused,
  onControlRun,
  onOpenCompare,
}) => {
  const [addMenuOpen, setAddMenuOpen] = useState(false);
  const menuRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    if (!addMenuOpen) return;
    const onDown = (e: MouseEvent) => {
      if (menuRef.current && !menuRef.current.contains(e.target as Node)) {
        setAddMenuOpen(false);
      }
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        setAddMenuOpen(false);
      }
    };
    window.addEventListener("mousedown", onDown);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("mousedown", onDown);
      window.removeEventListener("keydown", onKey);
    };
  }, [addMenuOpen]);

  const isLive =
    activeRunStatus === "queued" ||
    activeRunStatus === "compiling" ||
    activeRunStatus === "ready" ||
    activeRunStatus === "running";
  const isPaused = activeRunStatus === "paused";

  return (
    <header className="topbar">
      {/* Left: Cute Smiling Mascot + Reliner + Integrated Notebook Folder Tabs + "+" */}
      <div className="topbar-left">
        <div className="brand">
          <SketchBrandMark size={28} />
          <span>Reliner</span>
        </div>

        <div className="inline-tabs">
          {tabs.map((tab) => {
            const isActive = tab.documentId === activeDocumentId;
            return (
              <div
                key={tab.documentId}
                className={`tab-item ${isActive ? "active" : ""}`}
                onClick={() => onSelectTab(tab.documentId)}
                title={tab.info.path ?? `未保存草图 · ${tab.info.topology_id}`}
              >
                {tab.solved && (
                  <span className="tab-solved-dot" title="全部探测已连通">
                    ✓
                  </span>
                )}
                {tab.info.dirty && (
                  <span className="tab-dirty-dot" title="有未保存修改" />
                )}
                <span
                  style={{
                    overflow: "hidden",
                    textOverflow: "ellipsis",
                    whiteSpace: "nowrap",
                  }}
                >
                  {tab.info.topology_id}
                </span>
                <span className="tab-rev">r{tab.info.native_revision}</span>
                <button
                  className="tab-close"
                  title="关闭标签页"
                  onClick={(e) => {
                    e.stopPropagation();
                    onCloseTab(tab.documentId);
                  }}
                >
                  <X size={12} />
                </button>
              </div>
            );
          })}
        </div>

        {/* Positioned outside .inline-tabs so overflow-y:hidden NEVER clips the dropdown menu */}
        <div className="dropdown-anchor" ref={menuRef} style={{ flexShrink: 0, paddingTop: 8 }}>
          <button
            className="tab-add-btn"
            disabled={!connected}
            aria-expanded={addMenuOpen}
            aria-haspopup="menu"
            title="新建、打开文件、保存或切换关卡"
            onClick={() => setAddMenuOpen((v) => !v)}
          >
            +
          </button>
          {addMenuOpen && (
            <div className="dropdown-menu" role="menu">
              <button
                className="list-item"
                onClick={() => {
                  setAddMenuOpen(false);
                  onNewLab();
                }}
              >
                <span>
                  <FilePlus2 size={13} /> 新建空白草图
                </span>
              </button>
              <button
                className="list-item"
                onClick={() => {
                  setAddMenuOpen(false);
                  onOpenFile();
                }}
              >
                <span>
                  <FolderOpen size={13} /> 打开本地 JSON...
                </span>
              </button>
              <button
                className="list-item"
                onClick={() => {
                  setAddMenuOpen(false);
                  onOpenExamplesList();
                }}
              >
                <span>
                  <BookOpen size={13} /> 内置排障关卡...
                </span>
              </button>
              <div style={{ borderTop: "1.4px dashed var(--ink-soft)", margin: "4px 0" }} />
              <button
                className="list-item"
                disabled={!hasDoc}
                onClick={() => {
                  setAddMenuOpen(false);
                  onSave();
                }}
              >
                <span>
                  <Save size={13} /> 保存 {isDirty ? "*" : ""}
                </span>
                <span className="small muted mono">Ctrl+S</span>
              </button>
              <button
                className="list-item"
                disabled={!hasDoc}
                onClick={() => {
                  setAddMenuOpen(false);
                  onSaveAs();
                }}
              >
                <span>另存为...</span>
              </button>
              <button
                className="list-item"
                disabled={!hasDoc}
                onClick={() => {
                  setAddMenuOpen(false);
                  onValidate();
                }}
              >
                <span>
                  <CheckCircle2 size={13} /> 校验配置
                </span>
              </button>
            </div>
          )}
        </div>
      </div>

      {/* Right: Layout Mode Pill + Crisp B1-Beta RUN / STEP / RERUN Capsules */}
      <div className="topbar-right">
        <div className="mode-switch" role="group" aria-label="界面布局模式">
          <button
            className={`mode-btn ${layoutMode === "playful" ? "active" : ""}`}
            onClick={() => onChangeLayoutMode("playful")}
            title="玩乐模式：极简全屏画布 + 底部工具胶囊 + 悬浮便签"
          >
            <Sparkles size={11} /> 玩乐
          </button>
          <button
            className={`mode-btn ${layoutMode === "workbench" ? "active" : ""}`}
            onClick={() => onChangeLayoutMode("workbench")}
            title="三栏工程模式：左设备大纲 + 画布 + 右排障配置（可调宽）"
          >
            <Columns3 size={11} /> 三栏
          </button>
          <button
            className={`mode-btn ${layoutMode === "focus" ? "active" : ""}`}
            onClick={() => onChangeLayoutMode("focus")}
            title="双栏排障模式：左图标条 + 画布 + 右排障配置"
          >
            <Columns2 size={11} /> 双栏
          </button>
        </div>

        {canCompare && (
          <button
            className="btn btn-sm"
            onClick={onOpenCompare}
            title="对比当前工作区中的两次运行结果"
          >
            <GitCompare size={13} /> 对比
          </button>
        )}

        {!isPaused && (
          <button
            className="btn btn-sm btn-mint"
            disabled={!hasDoc}
            onClick={onRunAuto}
            title="执行当前实验的全部探测并播放动画"
          >
            <Play size={13} /> 运行实验
          </button>
        )}

        {!isLive && !isPaused ? (
          <button
            className="btn btn-sm btn-butter"
            disabled={!hasDoc}
            onClick={onRunPaused}
            title="以暂停模式启动，按完整事件边界单步推进"
          >
            <Pause size={13} /> 单步
          </button>
        ) : (
          <>
            {isLive && (
              <button
                className="btn btn-sm btn-butter"
                onClick={() => onControlRun("pause")}
                title="暂停仿真"
              >
                <Pause size={13} /> 暂停
              </button>
            )}
            {isPaused && (
              <>
                <button
                  className="btn btn-sm btn-butter"
                  onClick={() => onControlRun("step")}
                  title="向前推进一个完整事件"
                >
                  <SkipForward size={13} /> 步进
                </button>
                <button
                  className="btn btn-sm btn-mint"
                  onClick={() => onControlRun("resume")}
                  title="继续运行至结束"
                >
                  <Play size={13} /> 继续
                </button>
              </>
            )}
            <button
              className="btn btn-sm btn-danger"
              onClick={() => onControlRun("cancel")}
              title="取消运行"
            >
              <Square size={11} /> 取消
            </button>
          </>
        )}

        <button
          className="btn btn-sm"
          disabled={!hasDoc}
          onClick={onSaveAndRerun}
          title="保存文档并重跑（配置变更时自动递增原生修订），便于对比修复前后结果"
        >
          <RotateCcw size={13} /> 保存并重跑{isDirty ? "*" : ""}
        </button>
      </div>
    </header>
  );
};
