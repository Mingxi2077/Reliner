import React from "react";
import { RefreshCw } from "lucide-react";
import type { DocumentInfo, HandshakeState } from "../protocol/types";
import type { WorkspaceLayoutMode } from "./TopBar";

interface StatusBarProps {
  handshake: HandshakeState | null;
  connected: boolean;
  error: string | null;
  activeDocInfo: DocumentInfo | null;
  openDocCount: number;
  layoutMode: WorkspaceLayoutMode;
  onRestartBackend: () => void;
}

const MODE_LABELS: Record<WorkspaceLayoutMode, string> = {
  playful: "玩乐模式",
  workbench: "三栏模式",
  focus: "双栏模式",
};

export const StatusBar: React.FC<StatusBarProps> = ({
  handshake,
  connected,
  error,
  activeDocInfo,
  openDocCount,
  layoutMode,
  onRestartBackend,
}) => {
  return (
    <footer className="statusbar">
      <div className="status-group">
        <span>
          <span
            className={`status-dot ${connected ? "ok" : error ? "err" : "warn"}`}
          />
          {connected
            ? `reliner ${handshake?.hello.crate_version ?? ""} 已就绪`
            : error
              ? `后端异常：${error}`
              : "正在连接仿真后端..."}
        </span>
        {handshake && (
          <span className="mono muted">
            会话 {handshake.session_id.slice(0, 8)}…
          </span>
        )}
        <button
          style={{
            background: "transparent",
            border: "none",
            color: "var(--ink-secondary)",
            cursor: "pointer",
            display: "inline-flex",
            alignItems: "center",
            gap: 4,
            fontSize: 11,
            fontWeight: 700,
          }}
          title="重启常驻 reliner 后端工作区"
          onClick={onRestartBackend}
        >
          <RefreshCw size={11} /> 重启工作区
        </button>
      </div>

      <div className="status-group">
        <span>{MODE_LABELS[layoutMode]}</span>
        {activeDocInfo && (
          <span className="mono">
            r{activeDocInfo.native_revision}（生效 {activeDocInfo.effective_revision}）
          </span>
        )}
        <span>
          {openDocCount}/{handshake?.hello.limits.max_documents ?? 128} 份拓扑
        </span>
      </div>
    </footer>
  );
};
