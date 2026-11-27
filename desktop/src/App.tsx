import React, { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { open as openDialog, save as saveDialog, ask as askDialog } from "@tauri-apps/plugin-dialog";
import {
  FilePlus2,
  FolderOpen,
  PanelRightOpen,
  Play,
  X,
} from "lucide-react";
import { backendClient, BackendError } from "./protocol/client";
import type {
  BridgeRunOrigin,
  DocumentInfo,
  ExampleSummary,
  HandshakeState,
  RunControl,
  RunMode,
} from "./protocol/types";
import { diagnoseProbe } from "./protocol/diagnosis";
import {
  applyQuickProbe,
  bumpNativeRevision,
  createDevice,
  createLink,
  createStarterLab,
  deleteDeviceFromDoc,
  deleteLinkFromDoc,
  listAllPorts,
  parseExperimentJson,
  parsePresentationJson,
  serializeExperimentJson,
  type DeviceSpec,
  type NativeLabDocument,
  type NodePosition,
  type ViewportState,
} from "./model/document";
import {
  TopologyCanvas,
  type CanvasOverlayState,
  type CanvasToolMode,
  type SelectionTarget,
} from "./canvas/TopologyCanvas";
import { buildPacketPlaybackPlan } from "./canvas/packetPlayback";
import { TopBar, type WorkspaceLayoutMode } from "./components/TopBar";
import {
  ExamplesPopover,
  FocusIconRail,
  LeftPalette,
  PlayfulBottomDock,
} from "./components/LeftPalette";
import { RightPanel, type RightPanelTab } from "./components/RightPanel";
import { StatusBar } from "./components/StatusBar";
import {
  getExampleCategory,
  WelcomeSketchBanner,
} from "./components/SketchIcons";
import type { BannerNotice, DocumentTabState, RunRecord } from "./store/types";

const LAYOUT_STORAGE_KEY = "reliner.layoutMode.v1";

function loadInitialLayoutMode(): WorkspaceLayoutMode {
  try {
    const raw = window.localStorage.getItem(LAYOUT_STORAGE_KEY);
    if (raw === "playful" || raw === "workbench" || raw === "focus") {
      return raw;
    }
  } catch {
    // ignore storage errors
  }
  return "playful";
}

function isTerminalStatus(status: string | undefined): boolean {
  return (
    status === "finished" ||
    status === "incomplete" ||
    status === "cancelled" ||
    status === "failed"
  );
}

function isComparableRun(r: RunRecord): boolean {
  return (
    !r.evicted &&
    Boolean(r.summary) &&
    r.summary!.probe_counts !== undefined &&
    (r.summary!.status === "finished" ||
      r.summary!.status === "incomplete" ||
      r.summary!.status === "cancelled")
  );
}

function clampPanelWidths(
  winWidth: number,
  mode: WorkspaceLayoutMode,
  lw: number,
  rw: number
): { leftWidth: number; rightWidth: number } {
  const minCanvas = 320;
  if (mode === "workbench") {
    const maxLeft = Math.max(200, Math.min(380, winWidth - minCanvas - 360));
    const nextLeft = Math.max(200, Math.min(maxLeft, lw));
    const maxRight = Math.max(360, Math.min(720, winWidth - nextLeft - minCanvas));
    const nextRight = Math.max(360, Math.min(maxRight, rw));
    return { leftWidth: nextLeft, rightWidth: nextRight };
  }
  if (mode === "focus") {
    const maxRight = Math.max(360, Math.min(720, winWidth - 58 - minCanvas));
    return { leftWidth: lw, rightWidth: Math.max(360, Math.min(maxRight, rw)) };
  }
  const maxRight = Math.max(360, Math.min(720, winWidth - 48));
  return { leftWidth: lw, rightWidth: Math.max(360, Math.min(maxRight, rw)) };
}

export const App: React.FC = () => {
  const [handshake, setHandshake] = useState<HandshakeState | null>(null);
  const [connectionError, setConnectionError] = useState<string | null>(null);
  const [examples, setExamples] = useState<ExampleSummary[]>([]);
  const [tabs, setTabs] = useState<DocumentTabState[]>([]);
  const [activeDocId, setActiveDocId] = useState<string | null>(null);
  const [toolMode, setToolMode] = useState<CanvasToolMode>({ kind: "select" });
  const [notice, setNotice] = useState<BannerNotice | null>(null);

  // Switchable layout mode + resizable panel widths + playful drawer/popover
  const [layoutMode, setLayoutMode] = useState<WorkspaceLayoutMode>(
    loadInitialLayoutMode
  );
  const [leftSection, setLeftSection] = useState<"palette" | "examples">("palette");
  const [leftWidth, setLeftWidth] = useState(256);
  const [rightWidth, setRightWidth] = useState(448);
  const [committedRightWidth, setCommittedRightWidth] = useState(448);
  const [drawerOpen, setDrawerOpen] = useState(false);
  const [examplesPopoverOpen, setExamplesPopoverOpen] = useState(false);
  const [solvedTopologyIds, setSolvedTopologyIds] = useState<Set<string>>(
    () => new Set()
  );
  const [resizingSide, setResizingSide] = useState<"left" | "right" | null>(null);

  const leftWidthRef = useRef(leftWidth);
  leftWidthRef.current = leftWidth;
  const rightWidthRef = useRef(rightWidth);
  rightWidthRef.current = rightWidth;

  const handleChangeLayoutMode = useCallback((mode: WorkspaceLayoutMode) => {
    setLayoutMode(mode);
    setExamplesPopoverOpen(false);
    const clamped = clampPanelWidths(
      window.innerWidth,
      mode,
      leftWidthRef.current,
      rightWidthRef.current
    );
    setLeftWidth(clamped.leftWidth);
    setRightWidth(clamped.rightWidth);
    setCommittedRightWidth(clamped.rightWidth);
    try {
      window.localStorage.setItem(LAYOUT_STORAGE_KEY, mode);
    } catch {
      // ignore storage errors
    }
  }, []);

  useEffect(() => {
    const onResize = () => {
      const clamped = clampPanelWidths(
        window.innerWidth,
        layoutMode,
        leftWidthRef.current,
        rightWidthRef.current
      );
      setLeftWidth(clamped.leftWidth);
      setRightWidth(clamped.rightWidth);
      setCommittedRightWidth(clamped.rightWidth);
    };
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, [layoutMode]);

  const startPanelResize = useCallback(
    (side: "left" | "right", startX: number) => {
      setResizingSide(side);
      const startW = side === "left" ? leftWidthRef.current : rightWidthRef.current;
      const onMove = (ev: MouseEvent) => {
        const delta = ev.clientX - startX;
        if (side === "left") {
          const clamped = clampPanelWidths(
            window.innerWidth,
            layoutMode,
            startW + delta,
            rightWidthRef.current
          );
          setLeftWidth(clamped.leftWidth);
        } else {
          const clamped = clampPanelWidths(
            window.innerWidth,
            layoutMode,
            leftWidthRef.current,
            startW - delta
          );
          setRightWidth(clamped.rightWidth);
        }
      };
      const onUp = () => {
        setResizingSide(null);
        setCommittedRightWidth(rightWidthRef.current);
        window.removeEventListener("mousemove", onMove);
        window.removeEventListener("mouseup", onUp);
      };
      window.addEventListener("mousemove", onMove);
      window.addEventListener("mouseup", onUp);
    },
    [layoutMode]
  );

  const customLabCounter = useRef(1);
  const tabsRef = useRef<DocumentTabState[]>(tabs);
  tabsRef.current = tabs;
  const activeSessionRef = useRef<string | null>(null);
  const lastAckedDocRef = useRef<Map<string, NativeLabDocument>>(new Map());

  // Per-document serialized operation queue and authoritative revision tracker
  // Prevents rapid keystrokes or layout commits from racing and hitting stale_revision.
  const docQueuesRef = useRef<Map<string, Promise<unknown>>>(new Map());
  const docRevisionRef = useRef<Map<string, string>>(new Map());
  const docEffectiveRevRef = useRef<Map<string, string>>(new Map());

  const enqueueDocOp = useCallback(
    <T,>(docId: string, op: (currentRev: string) => Promise<T>): Promise<T> => {
      const prev = docQueuesRef.current.get(docId) ?? Promise.resolve();
      const run = prev
        .catch(() => undefined)
        .then(async () => {
          const tab = tabsRef.current.find((t) => t.documentId === docId);
          const rev =
            docRevisionRef.current.get(docId) ?? tab?.info.revision ?? "1";
          return op(rev);
        });
      docQueuesRef.current.set(docId, run);
      return run;
    },
    []
  );

  const activeTab = useMemo(
    () => tabs.find((t) => t.documentId === activeDocId) ?? null,
    [tabs, activeDocId]
  );

  const updateTab = useCallback(
    (docId: string, updater: (prev: DocumentTabState) => DocumentTabState) => {
      setTabs((prev) =>
        prev.map((tab) => (tab.documentId === docId ? updater(tab) : tab))
      );
    },
    []
  );

  const updateRun = useCallback(
    (
      docId: string,
      runId: string,
      updater: (prev: RunRecord) => RunRecord
    ) => {
      updateTab(docId, (tab) => ({
        ...tab,
        runs: tab.runs.map((r) => (r.runId === runId ? updater(r) : r)),
      }));
    },
    [updateTab]
  );

  // Initial handshake + examples discovery
  const initSession = useCallback(async () => {
    setConnectionError(null);
    try {
      const state = await backendClient.handshake();
      activeSessionRef.current = state.session_id;
      setHandshake(state);
      const exList = await backendClient.listExamples();
      setExamples(exList);
    } catch (err) {
      setConnectionError(err instanceof Error ? err.message : String(err));
    }
  }, []);

  useEffect(() => {
    void initSession();
  }, [initSession]);

  const handleRestartBackend = useCallback(async (force = false) => {
    const dirtyCount = tabsRef.current.filter((t) => t.info.dirty).length;
    if (!force && dirtyCount > 0) {
      const ok = await askDialog(
        `当前有 ${dirtyCount} 份拓扑包含未保存修改，重启工作区将清空全部打开的标签与运行历史。确定继续吗？`,
        { title: "重启仿真工作区", kind: "warning" }
      );
      if (!ok) return;
    }
    setConnectionError(null);
    try {
      const state = await backendClient.restart();
      activeSessionRef.current = state.session_id;
      docRevisionRef.current.clear();
      docEffectiveRevRef.current.clear();
      docQueuesRef.current.clear();
      lastAckedDocRef.current.clear();
      setHandshake(state);
      setTabs([]);
      setActiveDocId(null);
      setToolMode({ kind: "select" });
      setExamplesPopoverOpen(false);
      setDrawerOpen(false);
      if (examples.length === 0) {
        const exList = await backendClient.listExamples().catch(() => []);
        if (exList.length > 0) setExamples(exList);
      }
      setNotice({
        kind: "info",
        text: `已重启后端工作区，新会话 ID：${state.session_id.slice(0, 12)}…（旧会话的文档和运行已清空）。`,
      });
    } catch (err) {
      setConnectionError(err instanceof Error ? err.message : String(err));
    }
  }, [examples.length]);

  // Load opened document metadata + JSON into a new tab
  const attachOpenedDocument = useCallback(async (info: DocumentInfo) => {
    const data = await backendClient.getDocument(info.id);
    docRevisionRef.current.set(data.info.id, data.info.revision);
    docEffectiveRevRef.current.set(data.info.id, data.info.effective_revision);
    const parsedDoc = parseExperimentJson(data.experiment);
    lastAckedDocRef.current.set(data.info.id, parsedDoc);
    const parsedPresentation = parsePresentationJson(data.presentation);
    const newTab: DocumentTabState = {
      documentId: data.info.id,
      info: data.info,
      doc: parsedDoc,
      presentation: parsedPresentation,
      validation: null,
      effectiveSettings: null,
      selection: { kind: "none" },
      rightTab: "config",
      runs: [],
      activeRunId: null,
      selectedProbeId: null,
      selectedStpEventId: null,
      compareBaselineId: "",
      compareCandidateId: "",
      comparison: null,
      compareLoading: false,
      compareError: null,
    };
    setTabs((prev) => [...prev, newTab]);
    setActiveDocId(newTab.documentId);
    setToolMode({ kind: "select" });
    setExamplesPopoverOpen(false);
    // In playful mode, start with a clean full-bleed canvas view
    setDrawerOpen(false);
    return newTab;
  }, []);

  const refreshDocumentFromBackend = useCallback(
    async (docId: string) => {
      try {
        const data = await backendClient.getDocument(docId);
        docRevisionRef.current.set(docId, data.info.revision);
        docEffectiveRevRef.current.set(docId, data.info.effective_revision);
        const parsedDoc = parseExperimentJson(data.experiment);
        lastAckedDocRef.current.set(docId, parsedDoc);
        updateTab(docId, (tab) => ({
          ...tab,
          info: data.info,
          doc: parsedDoc,
          presentation: parsePresentationJson(data.presentation),
          validation: null,
          effectiveSettings: null,
        }));
        setNotice({ kind: "ok", text: "已从后端工作区重新同步最新文档修订。" });
      } catch (err) {
        setNotice({
          kind: "error",
          text: `同步文档失败：${err instanceof Error ? err.message : String(err)}`,
        });
      }
    },
    [updateTab]
  );

  const retryApplyDraftOnLatestRev = useCallback(
    async (docId: string) => {
      try {
        const freshInfo = await backendClient.getDocumentInfo(docId);
        docRevisionRef.current.set(docId, freshInfo.revision);
        const tab = tabsRef.current.find((t) => t.documentId === docId);
        if (!tab) return;
        const serialized = serializeExperimentJson(tab.doc);
        const nextInfo = await backendClient.replaceExperiment(
          docId,
          freshInfo.revision,
          serialized
        );
        docRevisionRef.current.set(docId, nextInfo.revision);
        lastAckedDocRef.current.set(docId, tab.doc);
        updateTab(docId, (prev) => ({
          ...prev,
          info: nextInfo,
          validation: null,
          effectiveSettings: null,
        }));
        setNotice({ kind: "ok", text: "已基于最新修订重新应用当前草稿修改。" });
      } catch (err) {
        setNotice({
          kind: "error",
          text: `重试应用草稿失败：${err instanceof Error ? err.message : String(err)}`,
        });
      }
    },
    [updateTab]
  );

  const handleBackendError = useCallback(
    (err: unknown, docId?: string) => {
      if (err instanceof BackendError) {
        if (err.code === "stale_revision" && docId) {
          setNotice({
            kind: "warn",
            text: `文档修订已过期（当前最新修订为 ${String(err.details.actual ?? "?")}），你的本地草稿已保留。`,
            actionLabel: "基于最新修订重试草稿",
            onAction: () => void retryApplyDraftOnLatestRev(docId),
            secondaryActionLabel: "放弃草稿并同步",
            onSecondaryAction: () => void refreshDocumentFromBackend(docId),
          });
          return;
        }
        if (err.code === "save_conflict" && docId) {
          setNotice({
            kind: "warn",
            text: `保存冲突：磁盘文件已被其他会话修改。你的草稿修改仍保留在内存中，请另存为新文件。`,
            actionLabel: "另存为新文件...",
            onAction: () => void handleSaveAs(docId),
          });
          return;
        }
        setNotice({
          kind: "error",
          text: `[${err.code}] ${err.message}`,
        });
        return;
      }
      const msg = err instanceof Error ? err.message : String(err);
      if (msg.includes("BACKEND_SESSION_LOST")) {
        setConnectionError("仿真后端会话已中断，请点击重启工作区恢复。");
        setHandshake(null);
        setNotice({
          kind: "error",
          text: "常驻仿真后端已意外退出，旧会话状态不再有效，请重启工作区。",
          actionLabel: "立即重启工作区",
          onAction: () => void handleRestartBackend(true),
        });
        return;
      }
      setNotice({
        kind: "error",
        text: msg,
      });
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [refreshDocumentFromBackend, retryApplyDraftOnLatestRev, handleRestartBackend]
  );

  const handleNewLab = async () => {
    try {
      const starter = createStarterLab(customLabCounter.current++);
      const info = await backendClient.openDocumentInput(
        serializeExperimentJson(starter)
      );
      await attachOpenedDocument(info);
      setNotice({
        kind: "info",
        text: `已新建实验拓扑 ${info.topology_id}。可从左侧拖入设备、连线，再点击「测连通」或「运行实验」。`,
      });
    } catch (err) {
      handleBackendError(err);
    }
  };

  const handleOpenExample = async (fileName: string) => {
    try {
      const content = await backendClient.readExample(fileName);
      const info = await backendClient.openDocumentInput(content);
      const tab = await attachOpenedDocument(info);
      setNotice({
        kind: "info",
        text: `已打开示例副本 ${fileName}（点击「运行实验」即可查看默认故障定位）。`,
      });
      return tab;
    } catch (err) {
      handleBackendError(err);
      return null;
    }
  };

  const handleOpenFile = async () => {
    try {
      const picked = await openDialog({
        multiple: false,
        title: "打开 Reliner 原生实验文件",
        filters: [{ name: "Reliner JSON", extensions: ["json"] }],
      });
      if (!picked || typeof picked !== "string") return;
      const info = await backendClient.openDocumentPath(picked);
      await attachOpenedDocument(info);
      setNotice({ kind: "ok", text: `已打开文件：${picked}` });
    } catch (err) {
      handleBackendError(err);
    }
  };

  // Experiment mutation (serialized per document -> increments revision & effective_revision)
  const commitExperiment = useCallback(
    (docId: string, nextDoc: NativeLabDocument): Promise<DocumentInfo | null> => {
      const serialized = serializeExperimentJson(nextDoc);
      // Immediate optimistic local update for snappy editing
      updateTab(docId, (prev) => ({ ...prev, doc: nextDoc }));
      return enqueueDocOp(docId, async (rev) => {
        try {
          const nextInfo = await backendClient.replaceExperiment(
            docId,
            rev,
            serialized
          );
          docRevisionRef.current.set(docId, nextInfo.revision);
          docEffectiveRevRef.current.set(docId, nextInfo.effective_revision);
          lastAckedDocRef.current.set(docId, nextDoc);
          updateTab(docId, (prev) => ({
            ...prev,
            info: nextInfo,
            validation: null,
            effectiveSettings: null,
          }));
          // Clear previous edit error banner once a subsequent edit succeeds cleanly
          setNotice((prev) => (prev?.kind === "error" ? null : prev));
          return nextInfo;
        } catch (err) {
          handleBackendError(err, docId);
          return null;
        }
      });
    },
    [enqueueDocOp, handleBackendError, updateTab]
  );

  // Presentation mutation (serialized per document -> does NOT increment effective_revision!)
  const commitPresentation = useCallback(
    (
      docId: string,
      nodes: Record<string, NodePosition>,
      viewport: ViewportState
    ) => {
      const current = tabsRef.current.find((t) => t.documentId === docId);
      const nextPresentation = {
        ...(current?.presentation ?? { version: 1, nodes: {} }),
        version: current?.presentation.version ?? 1,
        nodes,
        viewport,
      };
      updateTab(docId, (prev) => ({ ...prev, presentation: nextPresentation }));
      return enqueueDocOp(docId, async (rev) => {
        try {
          const nextInfo = await backendClient.replacePresentation(
            docId,
            rev,
            JSON.stringify(nextPresentation)
          );
          docRevisionRef.current.set(docId, nextInfo.revision);
          // Do NOT overwrite prev.presentation with stale nextPresentation after async IPC
          updateTab(docId, (prev) => ({
            ...prev,
            info: nextInfo,
          }));
        } catch (err) {
          handleBackendError(err, docId);
        }
      });
    },
    [enqueueDocOp, handleBackendError, updateTab]
  );

  const handleValidate = async (docId?: string) => {
    const targetId = docId ?? activeDocId;
    if (!targetId) return;
    await enqueueDocOp(targetId, async (rev) => {
      try {
        const res = await backendClient.validateDocument(targetId, rev);
        updateTab(targetId, (prev) => ({ ...prev, validation: res }));
        if (res.status === "valid") {
          setNotice({
            kind: "ok",
            text: `配置校验通过：${res.summary.devices} 台设备、${res.summary.ports} 个端口、${res.summary.links} 条链路、${res.summary.probes} 发探测。`,
          });
        } else {
          setNotice({
            kind: "error",
            text: `配置校验失败 [${res.error.code}]：${res.error.message}`,
          });
        }
      } catch (err) {
        handleBackendError(err, targetId);
      }
    });
  };

  const handleLoadEffectiveSettings = async (docId: string) => {
    await enqueueDocOp(docId, async (rev) => {
      try {
        const page = await backendClient.effectiveSettings(docId, rev, 0, 128);
        updateTab(docId, (prev) => ({ ...prev, effectiveSettings: page.entries }));
      } catch (err) {
        handleBackendError(err, docId);
      }
    });
  };

  const pickSavePath = async (tab: DocumentTabState): Promise<string | null> => {
    const picked = await saveDialog({
      title: "保存 Reliner 实验文件",
      defaultPath: tab.info.path ?? `${tab.info.topology_id}.json`,
      filters: [{ name: "Reliner JSON", extensions: ["json"] }],
    });
    return typeof picked === "string" ? picked : null;
  };

  const handleSave = async (docId?: string) => {
    const targetId = docId ?? activeDocId;
    if (!targetId) return;
    const tab = tabsRef.current.find((t) => t.documentId === targetId);
    if (!tab) return;
    const targetPath = tab.info.path ?? (await pickSavePath(tab));
    if (!targetPath) return;
    await enqueueDocOp(targetId, async (rev) => {
      try {
        const res = await backendClient.saveDocument(targetId, rev, targetPath);
        const freshInfo = await backendClient.getDocumentInfo(targetId);
        docRevisionRef.current.set(targetId, freshInfo.revision);
        updateTab(targetId, (prev) => ({ ...prev, info: freshInfo }));
        setNotice({
          kind: "ok",
          text: `已保存到 ${freshInfo.path ?? targetPath}（工作区修订 ${res.saved_revision}）。`,
        });
      } catch (err) {
        handleBackendError(err, targetId);
      }
    });
  };

  const handleSaveAs = async (docId?: string) => {
    const targetId = docId ?? activeDocId;
    if (!targetId) return;
    const tab = tabsRef.current.find((t) => t.documentId === targetId);
    if (!tab) return;
    const targetPath = await pickSavePath(tab);
    if (!targetPath) return;
    await enqueueDocOp(targetId, async (rev) => {
      try {
        const res = await backendClient.saveDocument(targetId, rev, targetPath);
        const freshInfo = await backendClient.getDocumentInfo(targetId);
        docRevisionRef.current.set(targetId, freshInfo.revision);
        updateTab(targetId, (prev) => ({ ...prev, info: freshInfo }));
        setNotice({
          kind: "ok",
          text: `已另存为 ${freshInfo.path ?? targetPath}（工作区修订 ${res.saved_revision}）。`,
        });
      } catch (err) {
        handleBackendError(err, targetId);
      }
    });
  };

  const drainDocQueue = useCallback(async (docId: string) => {
    const pending = docQueuesRef.current.get(docId);
    if (pending) {
      await pending.catch(() => undefined);
    }
  }, []);

  const handleCloseTab = async (docId: string) => {
    await drainDocQueue(docId);
    const tab = tabsRef.current.find((t) => t.documentId === docId);
    if (!tab) return;
    let discardUnsaved = false;
    if (tab.info.dirty) {
      const ok = await askDialog(
        `文档 ${tab.info.topology_id} 有未保存修改，确定放弃修改并关闭吗？`,
        { title: "关闭未保存的拓扑", kind: "warning" }
      );
      if (!ok) return;
      discardUnsaved = true;
    }
    const finalizeClose = () => {
      docRevisionRef.current.delete(docId);
      docEffectiveRevRef.current.delete(docId);
      docQueuesRef.current.delete(docId);
      lastAckedDocRef.current.delete(docId);
      const remaining = tabsRef.current.filter((t) => t.documentId !== docId);
      setTabs(remaining);
      setActiveDocId((curr) =>
        curr === docId ? remaining[0]?.documentId ?? null : curr
      );
    };
    await enqueueDocOp(docId, async (rev) => {
      try {
        await backendClient.closeDocument(docId, rev, discardUnsaved);
        finalizeClose();
      } catch (err) {
        if (err instanceof BackendError && err.code === "unsaved_document" && !discardUnsaved) {
          const ok = await askDialog(
            `文档 ${tab.info.topology_id} 有未保存修改，确定放弃修改并关闭吗？`,
            { title: "关闭未保存的拓扑", kind: "warning" }
          );
          if (!ok) return;
          try {
            const latestRev = docRevisionRef.current.get(docId) ?? rev;
            await backendClient.closeDocument(docId, latestRev, true);
            finalizeClose();
          } catch (retryErr) {
            handleBackendError(retryErr, docId);
          }
          return;
        }
        handleBackendError(err, docId);
      }
    });
  };

  const markRunEvicted = useCallback(
    (docId: string, runId: string) => {
      updateRun(docId, runId, (prev) => ({ ...prev, evicted: true }));
    },
    [updateRun]
  );

  // Fetch specific cause/reply/request evidence items when a probe's causal evidence ID > 128
  const ensureProbeEvidenceLoaded = useCallback(
    async (docId: string, runId: string, probeId: string) => {
      const tab = tabsRef.current.find((t) => t.documentId === docId);
      const run = tab?.runs.find((r) => r.runId === runId);
      if (!run || run.evicted) return;
      const probe = run.probes.find((p) => p.id === probeId);
      if (!probe) return;

      const wantedIds = new Set<string>();
      if (probe.request_evidence_id) wantedIds.add(probe.request_evidence_id);
      if (probe.outcome.status === "timeout" && probe.outcome.cause) {
        wantedIds.add(probe.outcome.cause);
      } else if (probe.outcome.status === "reachable") {
        wantedIds.add(probe.outcome.evidence_id);
      }

      const existingIds = new Set(run.evidence.map((e) => e.id));
      const missingIds = [...wantedIds].filter((id) => !existingIds.has(id));
      if (missingIds.length === 0) return;

      const fetched: typeof run.evidence = [];
      for (const id of missingIds) {
        const numeric = Number(id);
        if (!Number.isInteger(numeric) || numeric < 1) continue;
        try {
          const page = await backendClient.runEvidence(runId, numeric - 1, 1);
          for (const entry of page.entries) {
            if (!existingIds.has(entry.id)) {
              existingIds.add(entry.id);
              fetched.push(entry);
            }
          }
        } catch (err) {
          if (err instanceof BackendError && err.code === "unknown_run") {
            markRunEvicted(docId, runId);
            return;
          }
        }
      }
      if (fetched.length > 0) {
        updateRun(docId, runId, (prev) => {
          const seen = new Set(prev.evidence.map((e) => e.id));
          const extra = fetched.filter((e) => !seen.has(e.id));
          return extra.length > 0
            ? { ...prev, evidence: [...prev.evidence, ...extra] }
            : prev;
        });
      }
    },
    [markRunEvicted, updateRun]
  );

  // Load details for a terminal run (including bounded trace slice for Canvas packet playback)
  const loadRunDetails = useCallback(
    async (docId: string, runId: string, summary: RunRecord["summary"]) => {
      try {
        const [probesPage, evidencePage, linksPage, routesPage, stpPage, tracePage] =
          await Promise.all([
            backendClient.runProbes(runId, 0, 64),
            backendClient.runEvidence(runId, 0, 128),
            backendClient.runLinks(runId, 0, 64),
            backendClient.runRoutes(runId, 0, 64),
            backendClient.runStp(runId, 0, 64),
            backendClient.runTrace(runId, 0, 128).catch(() => null),
          ]);

        // Auto-pick first faulty probe (or first probe) for immediate first-screen diagnosis
        const firstFault = probesPage.entries.find(
          (p) => p.outcome.status === "timeout" || p.outcome.status === "undetermined"
        );
        const autoProbe = firstFault ?? probesPage.entries[0] ?? null;
        const autoProbeId = autoProbe?.id ?? null;

        // On-demand fetch cause/request evidence for the selected probe if its ID is beyond page 1 (> 128)
        const initialEvidence = [...evidencePage.entries];
        if (autoProbe) {
          const seenEv = new Set(initialEvidence.map((e) => e.id));
          const needed: string[] = [];
          if (autoProbe.request_evidence_id && !seenEv.has(autoProbe.request_evidence_id)) {
            needed.push(autoProbe.request_evidence_id);
          }
          if (
            autoProbe.outcome.status === "timeout" &&
            autoProbe.outcome.cause &&
            !seenEv.has(autoProbe.outcome.cause)
          ) {
            needed.push(autoProbe.outcome.cause);
          } else if (
            autoProbe.outcome.status === "reachable" &&
            !seenEv.has(autoProbe.outcome.evidence_id)
          ) {
            needed.push(autoProbe.outcome.evidence_id);
          }
          for (const evId of needed) {
            const num = Number(evId);
            if (!Number.isInteger(num) || num < 1) continue;
            const single = await backendClient.runEvidence(runId, num - 1, 1).catch(() => null);
            if (single) {
              for (const item of single.entries) {
                if (!seenEv.has(item.id)) {
                  seenEv.add(item.id);
                  initialEvidence.push(item);
                }
              }
            }
          }
        }

        // If STP events exist, also preload the latest STP event for canvas badges
        let autoStpEventId: string | null = null;
        let autoStpDetail:
          | Record<string, { bridges: RunRecord["stpDetailsByEventId"][string]["bridges"]; links: RunRecord["stpDetailsByEventId"][string]["links"] }>
          = {};
        if (stpPage.entries.length > 0) {
          const evId = stpPage.entries[stpPage.entries.length - 1].event_id;
          autoStpEventId = evId;
          const [bPage, lPage] = await Promise.all([
            backendClient.runStpBridges(runId, evId, 0, 128),
            backendClient.runStpLinks(runId, evId, 0, 128),
          ]);
          autoStpDetail = {
            [evId]: { bridges: bPage.entries, links: lPage.entries },
          };
        }

        updateRun(docId, runId, (prev) => ({
          ...prev,
          probes: probesPage.entries,
          probesNextOffset: probesPage.next_offset,
          evidence: initialEvidence,
          evidenceNextOffset: evidencePage.next_offset,
          linkEvents: linksPage.entries,
          linkEventsNextOffset: linksPage.next_offset,
          routingEvents: routesPage.entries,
          routingEventsNextOffset: routesPage.next_offset,
          stpSummaries: stpPage.entries,
          stpDetailsByEventId: { ...prev.stpDetailsByEventId, ...autoStpDetail },
          trace: tracePage
            ? {
                status: tracePage.status,
                entries: tracePage.entries,
                nextOffset: tracePage.next_offset,
              }
            : prev.trace,
        }));

        // Stamp solved badge ONLY when the authoritative run summary confirms finished & 100% reachable
        const counts = summary?.probe_counts;
        if (
          summary?.status === "finished" &&
          counts &&
          counts.reachable > 0 &&
          counts.timed_out === 0 &&
          counts.rejected_before_send === 0 &&
          counts.incomplete === 0 &&
          (counts.undetermined ?? 0) === 0
        ) {
          const tab = tabsRef.current.find((t) => t.documentId === docId);
          if (tab) {
            setSolvedTopologyIds((prev) => {
              const next = new Set(prev);
              next.add(tab.info.topology_id);
              return next;
            });
          }
        }

        updateTab(docId, (tab) => {
          const comparableIds = tab.runs
            .filter(isComparableRun)
            .map((r) => r.runId);
          const candidateId = comparableIds[0] ?? tab.compareCandidateId;
          const baselineId =
            comparableIds.find((id) => id !== candidateId) ?? tab.compareBaselineId;
          return {
            ...tab,
            selectedProbeId:
              tab.activeRunId === runId ? autoProbeId : tab.selectedProbeId,
            selectedStpEventId:
              tab.activeRunId === runId ? autoStpEventId : tab.selectedStpEventId,
            compareCandidateId: candidateId,
            compareBaselineId: baselineId,
          };
        });
        setDrawerOpen(true);
      } catch (err) {
        if (err instanceof BackendError && err.code === "unknown_run") {
          markRunEvicted(docId, runId);
          return;
        }
        handleBackendError(err, docId);
      }
    },
    [handleBackendError, markRunEvicted, updateRun, updateTab]
  );

  // Poll a live run until paused or terminal (guarded against backend session changes)
  const pollRunUntilSettled = useCallback(
    async (docId: string, runId: string, sessionId: string) => {
      const start = Date.now();
      let delayMs = 25;
      while (Date.now() - start < 30_000) {
        if (activeSessionRef.current !== sessionId) return;
        try {
          const data = await backendClient.runSummary(runId);
          if (activeSessionRef.current !== sessionId) return;
          updateRun(docId, runId, (prev) => ({
            ...prev,
            summary: data.summary,
          }));
          if (data.summary.status === "paused") {
            const tracePage = await backendClient.runTrace(runId, 0, 128).catch(() => null);
            if (tracePage && activeSessionRef.current === sessionId) {
              updateRun(docId, runId, (prev) => ({
                ...prev,
                trace: {
                  status: tracePage.status,
                  entries: tracePage.entries,
                  nextOffset: tracePage.next_offset,
                },
              }));
            }
            setDrawerOpen(true);
            return;
          }
          if (isTerminalStatus(data.summary.status)) {
            if (
              data.summary.status === "finished" ||
              data.summary.status === "incomplete" ||
              (data.summary.status === "cancelled" &&
                data.summary.probe_counts !== undefined)
            ) {
              await loadRunDetails(docId, runId, data.summary);
            } else {
              setDrawerOpen(true);
            }
            return;
          }
        } catch (err) {
          if (activeSessionRef.current !== sessionId) return;
          if (err instanceof BackendError && err.code === "unknown_run") {
            markRunEvicted(docId, runId);
            return;
          }
          handleBackendError(err, docId);
          return;
        }
        await new Promise((r) => setTimeout(r, delayMs));
        delayMs = Math.min(180, Math.round(delayMs * 1.3));
      }
      if (activeSessionRef.current === sessionId) {
        setNotice({
          kind: "warn",
          text: `运行 #${runId} 执行超过 30 秒仍在后台运行。`,
          actionLabel: "继续刷新状态",
          onAction: () => void pollRunUntilSettled(docId, runId, sessionId),
        });
      }
    },
    [handleBackendError, loadRunDetails, markRunEvicted, updateRun]
  );

  const registerSubmittedRun = useCallback(
    (docId: string, origin: BridgeRunOrigin) => {
      const sessionId =
        activeSessionRef.current ?? backendClient.sessionId ?? handshake?.session_id ?? "";
      const record: RunRecord = {
        sessionId,
        runId: origin.run_id,
        origin,
        summary: null,
        evicted: false,
        probes: [],
        probesNextOffset: null,
        evidence: [],
        evidenceNextOffset: null,
        linkEvents: [],
        linkEventsNextOffset: null,
        routingEvents: [],
        routingEventsNextOffset: null,
        stpSummaries: [],
        stpDetailsByEventId: {},
        trace: null,
        importGaps: null,
      };
      updateTab(docId, (tab) => {
        const previousComparableId =
          tab.runs.find(isComparableRun)?.runId ?? tab.compareBaselineId;
        return {
          ...tab,
          runs: [record, ...tab.runs],
          activeRunId: record.runId,
          rightTab: "result",
          compareBaselineId: previousComparableId,
          compareCandidateId: record.runId,
          comparison: null,
          compareError: null,
        };
      });
      void pollRunUntilSettled(docId, origin.run_id, sessionId);
    },
    [handshake?.session_id, pollRunUntilSettled, updateTab]
  );

  const handleSubmitRun = async (mode: RunMode, docId?: string) => {
    const targetId = docId ?? activeDocId;
    if (!targetId) return;
    // Clear stale "open example -> click run" helper banner once user starts running
    setNotice((prev) => (prev?.kind === "info" ? null : prev));
    await enqueueDocOp(targetId, async (rev) => {
      try {
        const submitted = await backendClient.submitRun(targetId, rev, mode);
        registerSubmittedRun(targetId, submitted.origin);
      } catch (err) {
        handleBackendError(err, targetId);
      }
    });
  };

  // Auto-bump native revision if effective config changed since last run and user kept the same revision string
  const autoBumpRevisionIfNeeded = async (docId: string): Promise<boolean> => {
    await drainDocQueue(docId);
    const latestTab = tabsRef.current.find((t) => t.documentId === docId);
    const lastRun = latestTab?.runs[0];
    const currentEffectiveRev =
      docEffectiveRevRef.current.get(docId) ?? latestTab?.info.effective_revision;
    if (
      latestTab &&
      lastRun &&
      currentEffectiveRev &&
      currentEffectiveRev !== lastRun.origin.effective_revision &&
      latestTab.doc.revision === lastRun.origin.native_revision
    ) {
      const bumpedDoc: NativeLabDocument = {
        ...latestTab.doc,
        revision: bumpNativeRevision(latestTab.doc.revision),
      };
      const updatedInfo = await commitExperiment(docId, bumpedDoc);
      if (!updatedInfo) return false;
    }
    return true;
  };

  // Save & Rerun (for explicit TopBar Save & Rerun, or file-backed inline CTA)
  const handleSaveAndRerun = async () => {
    if (!activeDocId) return;
    const docId = activeDocId;
    await drainDocQueue(docId);
    const tab = tabsRef.current.find((t) => t.documentId === docId);
    if (!tab) return;

    const targetPath = tab.info.path ?? (await pickSavePath(tab));
    if (!targetPath) return;

    if (!(await autoBumpRevisionIfNeeded(docId))) return;

    await enqueueDocOp(docId, async (rev) => {
      try {
        const result = await backendClient.saveAndRerun(
          docId,
          rev,
          targetPath,
          "automatic"
        );
        const freshInfo = await backendClient.getDocumentInfo(docId);
        docRevisionRef.current.set(docId, freshInfo.revision);
        updateTab(docId, (prev) => ({ ...prev, info: freshInfo }));

        if (result.rerun.status === "accepted") {
          setNotice({
            kind: "ok",
            text: `修订 r${freshInfo.native_revision} 已保存（工作区修订 ${result.saved_revision}），已提交运行 #${result.rerun.origin.run_id}。`,
          });
          registerSubmittedRun(docId, result.rerun.origin);
        } else {
          // Explicit dual feedback per README & backend contract!
          setNotice({
            kind: "warn",
            text: `修订 r${freshInfo.native_revision} 已保存（工作区修订 ${result.saved_revision}） · 重跑未受理：[${result.rerun.error.code}] ${result.rerun.error.message}`,
          });
        }
      } catch (err) {
        handleBackendError(err, docId);
      }
    });
  };

  // Inline config CTA: for unsaved example copies, bump revision & rerun directly without OS Save dialog;
  // for file-backed documents, execute full Save & Rerun.
  const handleQuickRerun = async () => {
    if (!activeDocId) return;
    const docId = activeDocId;
    const tab = tabsRef.current.find((t) => t.documentId === docId);
    if (!tab) return;
    if (tab.info.path) {
      await handleSaveAndRerun();
      return;
    }
    if (!(await autoBumpRevisionIfNeeded(docId))) return;
    await handleSubmitRun("automatic", docId);
  };

  const handleControlRun = async (control: RunControl) => {
    const curTab = tabsRef.current.find((t) => t.documentId === activeDocId);
    if (!curTab?.activeRunId) return;
    const docId = curTab.documentId;
    const runId = curTab.activeRunId;
    const sessionId = activeSessionRef.current ?? "";
    try {
      await backendClient.controlRun(runId, control);
      void pollRunUntilSettled(docId, runId, sessionId);
    } catch (err) {
      if (err instanceof BackendError && err.code === "unknown_run") {
        markRunEvicted(docId, runId);
        return;
      }
      handleBackendError(err, docId);
    }
  };

  const handleRunCompare = async (
    docId: string,
    baselineId: string,
    candidateId: string,
    appendOffset = 0
  ) => {
    updateTab(docId, (tab) => ({
      ...tab,
      compareBaselineId: baselineId,
      compareCandidateId: candidateId,
      compareLoading: true,
      compareError: null,
    }));
    try {
      const data = await backendClient.compareRuns(
        baselineId,
        candidateId,
        appendOffset,
        64
      );
      updateTab(docId, (tab) => ({
        ...tab,
        compareLoading: false,
        comparison:
          appendOffset > 0 && tab.comparison
            ? {
                ...data,
                entries: [...tab.comparison.entries, ...data.entries],
              }
            : data,
      }));
    } catch (err) {
      if (err instanceof BackendError && err.code === "unknown_run") {
        const missingRunId =
          typeof err.details.run_id === "string" ? err.details.run_id : null;
        if (missingRunId) {
          markRunEvicted(docId, missingRunId);
        }
      }
      const msg =
        err instanceof BackendError && err.code === "unknown_run"
          ? "其中一次运行已从历史池淘汰，无法对比。"
          : err instanceof Error
            ? err.message
            : String(err);
      updateTab(docId, (tab) => ({
        ...tab,
        compareLoading: false,
        compareError: msg,
      }));
    }
  };

  const handleQuickProbeTrigger = () => {
    const curTab = tabsRef.current.find((t) => t.documentId === activeDocId);
    if (!curTab) return;
    if (toolMode.kind === "probe") {
      setToolMode({ kind: "select" });
      return;
    }
    // README flow: if source host is already selected, jump directly to picking destination host
    const sel = curTab.selection;
    const selectedHost =
      sel.kind === "device" || sel.kind === "port"
        ? curTab.doc.devices.find(
            (d) => d.id === sel.deviceId && d.kind === "host"
          )
        : undefined;
    setToolMode({
      kind: "probe",
      sourceDeviceId: selectedHost?.id,
    });
  };

  const handleQuickProbeComplete = async (
    sourceDeviceId: string,
    destDeviceId: string
  ) => {
    const curTab = tabsRef.current.find((t) => t.documentId === activeDocId);
    if (!curTab) return;
    const hasSrc = curTab.doc.devices.some(
      (d) => d.id === sourceDeviceId && d.kind === "host"
    );
    const hasDst = curTab.doc.devices.some(
      (d) => d.id === destDeviceId && d.kind === "host"
    );
    if (!hasSrc || !hasDst) return;
    const docId = curTab.documentId;
    const nextDoc = applyQuickProbe(curTab.doc, sourceDeviceId, destDeviceId);
    if (nextDoc !== curTab.doc) {
      const updated = await commitExperiment(docId, nextDoc);
      if (!updated) return;
    }
    await handleSubmitRun("automatic", docId);
  };

  const handleAddDevice = async (
    kind: DeviceSpec["kind"],
    pos?: NodePosition,
    liveViewport?: ViewportState
  ) => {
    const curTab = tabsRef.current.find((t) => t.documentId === activeDocId);
    if (!curTab) return;
    if (kind === "router" && curTab.doc.format_version === 1) {
      setNotice({
        kind: "warn",
        text: "格式 1 实验仅支持主机与交换机，无法添加路由器。",
      });
      return;
    }
    const docId = curTab.documentId;
    const device = createDevice(curTab.doc, kind);
    const nextDoc: NativeLabDocument = {
      ...curTab.doc,
      devices: [...curTab.doc.devices, device],
    };
    const updatedInfo = await commitExperiment(docId, nextDoc);
    if (!updatedInfo) return;
    if (pos) {
      const latest = tabsRef.current.find((t) => t.documentId === docId);
      const vp =
        liveViewport ?? latest?.presentation.viewport ?? { x: 40, y: 40, zoom: 1 };
      await commitPresentation(
        docId,
        { ...(latest?.presentation.nodes ?? {}), [device.id]: pos },
        vp
      );
    }
    updateTab(docId, (tab) => ({
      ...tab,
      selection: { kind: "device", deviceId: device.id },
      rightTab: "config",
    }));
  };

  const handleCreateLink = async (portA: string, portB: string) => {
    const curTab = tabsRef.current.find((t) => t.documentId === activeDocId);
    if (!curTab) return;
    const ports = listAllPorts(curTab.doc);
    const infoA = ports.find((p) => p.portId === portA);
    const infoB = ports.find((p) => p.portId === portB);
    if (
      !infoA ||
      !infoB ||
      infoA.connectedLinkId !== null ||
      infoB.connectedLinkId !== null ||
      infoA.deviceId === infoB.deviceId
    ) {
      return;
    }
    const docId = curTab.documentId;
    const link = createLink(curTab.doc, portA, portB);
    const nextDoc: NativeLabDocument = {
      ...curTab.doc,
      links: [...curTab.doc.links, link],
    };
    const updated = await commitExperiment(docId, nextDoc);
    if (updated) {
      updateTab(docId, (tab) => ({
        ...tab,
        selection: { kind: "link", linkId: link.id },
        rightTab: "config",
      }));
    }
  };

  const handleDeleteDevice = async (deviceId: string) => {
    const curTab = tabsRef.current.find((t) => t.documentId === activeDocId);
    if (!curTab) return;
    const docId = curTab.documentId;
    setToolMode({ kind: "select" });
    const nextDoc = deleteDeviceFromDoc(curTab.doc, deviceId);
    const updated = await commitExperiment(docId, nextDoc);
    if (updated) {
      updateTab(docId, (tab) => {
        const nextNodes = { ...tab.presentation.nodes };
        delete nextNodes[deviceId];
        return {
          ...tab,
          presentation: { ...tab.presentation, nodes: nextNodes },
          selection: { kind: "none" },
        };
      });
    }
  };

  const handleDeleteLink = async (linkId: string) => {
    const curTab = tabsRef.current.find((t) => t.documentId === activeDocId);
    if (!curTab) return;
    const docId = curTab.documentId;
    setToolMode({ kind: "select" });
    const nextDoc = deleteLinkFromDoc(curTab.doc, linkId);
    const updated = await commitExperiment(docId, nextDoc);
    if (updated) {
      updateTab(docId, (tab) => ({
        ...tab,
        selection: { kind: "none" },
      }));
    }
  };

  const handleLoadTrace = async (
    docId: string,
    runId: string,
    reset = false
  ) => {
    const tab = tabsRef.current.find((t) => t.documentId === docId);
    const run = tab?.runs.find((r) => r.runId === runId);
    if (!run) return;
    const offset = reset ? 0 : run.trace?.nextOffset ?? 0;
    try {
      const page = await backendClient.runTrace(runId, offset, 128);
      updateRun(docId, runId, (prev) => ({
        ...prev,
        trace: {
          status: page.status,
          entries: reset
            ? page.entries
            : [...(prev.trace?.entries ?? []), ...page.entries],
          nextOffset: page.next_offset,
        },
      }));
    } catch (err) {
      if (err instanceof BackendError && err.code === "unknown_run") {
        markRunEvicted(docId, runId);
        return;
      }
      handleBackendError(err, docId);
    }
  };

  const handleSelectStpEvent = async (
    docId: string,
    runId: string,
    eventId: string
  ) => {
    updateTab(docId, (tab) => ({ ...tab, selectedStpEventId: eventId }));
    const tab = tabsRef.current.find((t) => t.documentId === docId);
    const run = tab?.runs.find((r) => r.runId === runId);
    if (!run || run.stpDetailsByEventId[eventId]) return;
    try {
      const [bridges, links] = await Promise.all([
        backendClient.runStpBridges(runId, eventId, 0, 128),
        backendClient.runStpLinks(runId, eventId, 0, 128),
      ]);
      updateRun(docId, runId, (prev) => ({
        ...prev,
        stpDetailsByEventId: {
          ...prev.stpDetailsByEventId,
          [eventId]: { bridges: bridges.entries, links: links.entries },
        },
      }));
    } catch (err) {
      if (err instanceof BackendError && err.code === "unknown_run") {
        markRunEvicted(docId, runId);
        return;
      }
      handleBackendError(err, docId);
    }
  };

  const handleLoadImportGaps = async (
    docId: string,
    runId: string,
    probeId: string
  ) => {
    const tab = tabsRef.current.find((t) => t.documentId === docId);
    const run = tab?.runs.find((r) => r.runId === runId);
    const probe = run?.probes.find((p) => p.id === probeId);
    const ref =
      probe?.outcome.status === "undetermined" ? probe.outcome.coverage : null;
    try {
      const page = await backendClient.runImportGaps(runId, ref, 0, 64);
      updateRun(docId, runId, (prev) => ({ ...prev, importGaps: page.entries }));
    } catch (err) {
      if (err instanceof BackendError && err.code === "unknown_run") {
        markRunEvicted(docId, runId);
        return;
      }
      handleBackendError(err, docId);
    }
  };

  // Compute active Canvas 2D overlay for the CURRENT active tab only
  const activeRun = useMemo(() => {
    if (!activeTab || !activeTab.activeRunId) return null;
    return (
      activeTab.runs.find((r) => r.runId === activeTab.activeRunId) ?? null
    );
  }, [activeTab]);

  const activeDoc = activeTab?.doc ?? null;
  const activeValidation = activeTab?.validation ?? null;
  const activeSelectedProbeId = activeTab?.selectedProbeId ?? null;
  const activeSelectedStpEventId = activeTab?.selectedStpEventId ?? null;

  const canvasOverlay = useMemo<CanvasOverlayState>(() => {
    const validationError =
      activeValidation?.status === "invalid"
        ? activeValidation.error
        : activeRun?.summary?.input_error ?? null;

    if (!activeDoc || !activeRun || activeRun.probes.length === 0) {
      return {
        diagnosis: null,
        validationError,
        playback: null,
        playbackKey: "none",
      };
    }
    const probe =
      activeRun.probes.find((p) => p.id === activeSelectedProbeId) ??
      activeRun.probes[0];
    const evMap = new Map(activeRun.evidence.map((e) => [e.id, e]));
    const diag = diagnoseProbe(probe, evMap, activeRun.routingEvents);
    const stpDetail = activeSelectedStpEventId
      ? activeRun.stpDetailsByEventId[activeSelectedStpEventId]
      : undefined;
    const playback = buildPacketPlaybackPlan(
      activeDoc,
      probe,
      diag,
      activeRun.trace?.entries ?? [],
      activeRun.evidence
    );

    return {
      diagnosis: diag,
      probeSourceId: probe.source.device_id,
      probeDestId: probe.destination.device_id,
      validationError,
      stpBridges: stpDetail?.bridges,
      stpLinks: stpDetail?.links,
      linkEvents: activeRun.linkEvents,
      playback,
      playbackKey: `${activeRun.runId}:${probe.id}:${playback?.hops.length ?? 0}:${diag.severity}:${diag.causeEvidence?.id ?? ""}`,
    };
  }, [
    activeDoc,
    activeValidation,
    activeSelectedProbeId,
    activeSelectedStpEventId,
    activeRun,
  ]);

  const handleOpenFaultTargetFromCanvas = useCallback(() => {
    if (!activeTab) return;
    const target = canvasOverlay.diagnosis?.target;
    if (!target) return;
    if (target.kind === "port" && target.deviceId && target.portId) {
      updateTab(activeTab.documentId, (t) => ({
        ...t,
        selection: { kind: "port", deviceId: target.deviceId!, portId: target.portId! },
        rightTab: "config",
      }));
    } else if (target.kind === "device" && target.deviceId) {
      updateTab(activeTab.documentId, (t) => ({
        ...t,
        selection: { kind: "device", deviceId: target.deviceId! },
        rightTab: "config",
      }));
    } else if (target.kind === "link" && target.linkId) {
      updateTab(activeTab.documentId, (t) => ({
        ...t,
        selection: { kind: "link", linkId: target.linkId! },
        rightTab: "config",
      }));
    }
    setDrawerOpen(true);
  }, [activeTab, canvasOverlay.diagnosis, updateTab]);

  // Global keyboard shortcuts (Escape cancels tool mode, Ctrl+S saves, Delete removes canvas selection)
  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.isComposing) return;
      if (e.key === "Escape") {
        setToolMode({ kind: "select" });
        setExamplesPopoverOpen(false);
        return;
      }
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "s") {
        e.preventDefault();
        void handleSave();
        return;
      }
      if (e.key === "Delete") {
        const targetEl = e.target as HTMLElement | null;
        // Strictly allow Delete only when focus is on the topology canvas or document body
        // (never while editing inputs, toggling switches, clicking panel controls, or navigating menus)
        const isCanvasOrBody =
          !targetEl ||
          targetEl === document.body ||
          targetEl.tagName === "CANVAS";
        if (!isCanvasOrBody) return;
        const cur = tabsRef.current.find((t) => t.documentId === activeDocId);
        if (!cur) return;
        if (cur.selection.kind === "device") {
          void handleDeleteDevice(cur.selection.deviceId);
        } else if (cur.selection.kind === "link") {
          void handleDeleteLink(cur.selection.linkId);
        }
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [activeDocId]);

  const selectAndRevealConfig = (target: SelectionTarget) => {
    if (!activeTab) return;
    updateTab(activeTab.documentId, (t) => ({
      ...t,
      selection: target,
      rightTab: target.kind === "none" ? t.rightTab : "config",
    }));
    if (target.kind !== "none") {
      setDrawerOpen(true);
    }
  };

  const showRightPanel =
    Boolean(activeTab) && (layoutMode !== "playful" || drawerOpen);
  const canCompareActive = Boolean(
    activeTab && activeTab.runs.filter(isComparableRun).length >= 2
  );
  const solvedExamplesCount = useMemo(
    () => examples.filter((ex) => solvedTopologyIds.has(ex.id)).length,
    [examples, solvedTopologyIds]
  );

  return (
    <div className="app-shell">
      <TopBar
        tabs={tabs.map((t) => ({
          documentId: t.documentId,
          info: t.info,
          solved: solvedTopologyIds.has(t.info.topology_id),
        }))}
        activeDocumentId={activeDocId}
        hasDoc={Boolean(activeTab)}
        isDirty={Boolean(activeTab?.info.dirty)}
        canCompare={canCompareActive}
        activeRunStatus={activeRun?.summary?.status ?? null}
        connected={Boolean(handshake)}
        layoutMode={layoutMode}
        onSelectTab={(id) => {
          setActiveDocId(id);
          setToolMode({ kind: "select" });
        }}
        onCloseTab={(id) => void handleCloseTab(id)}
        onChangeLayoutMode={handleChangeLayoutMode}
        onNewLab={() => void handleNewLab()}
        onOpenFile={() => void handleOpenFile()}
        onOpenExamplesList={() => {
          if (layoutMode === "workbench") {
            setLeftSection("examples");
          } else if (activeTab) {
            setExamplesPopoverOpen((v) => !v);
          }
        }}
        onSave={() => void handleSave()}
        onSaveAs={() => void handleSaveAs()}
        onSaveAndRerun={() => void handleSaveAndRerun()}
        onValidate={() => {
          void handleValidate();
          setDrawerOpen(true);
        }}
        onRunAuto={() => void handleSubmitRun("automatic")}
        onRunPaused={() => void handleSubmitRun("paused")}
        onControlRun={(c) => void handleControlRun(c)}
        onOpenCompare={() => {
          if (!activeTab) return;
          updateTab(activeTab.documentId, (t) => ({ ...t, rightTab: "compare" }));
          setDrawerOpen(true);
          if (
            activeTab.compareBaselineId &&
            activeTab.compareCandidateId &&
            activeTab.compareBaselineId !== activeTab.compareCandidateId &&
            !activeTab.comparison
          ) {
            void handleRunCompare(
              activeTab.documentId,
              activeTab.compareBaselineId,
              activeTab.compareCandidateId
            );
          }
        }}
      />

      {notice && (
        <div className={`notice-banner ${notice.kind}`}>
          <span>{notice.text}</span>
          <div style={{ display: "flex", alignItems: "center", gap: 6 }}>
            {notice.actionLabel && notice.onAction && (
              <button className="btn btn-xs" onClick={notice.onAction}>
                {notice.actionLabel}
              </button>
            )}
            {notice.secondaryActionLabel && notice.onSecondaryAction && (
              <button className="btn btn-xs" onClick={notice.onSecondaryAction}>
                {notice.secondaryActionLabel}
              </button>
            )}
            <button className="tab-close" onClick={() => setNotice(null)}>
              <X size={13} />
            </button>
          </div>
        </div>
      )}

      <div className="workspace-body">
        {/* Left Sidebar: full in Workbench, icon rail in Focus, omitted in Playful */}
        {layoutMode === "workbench" && (
          <>
            <LeftPalette
              width={leftWidth}
              doc={activeTab?.doc ?? null}
              section={leftSection}
              onChangeSection={setLeftSection}
              selection={activeTab?.selection ?? { kind: "none" }}
              toolMode={toolMode}
              examples={examples}
              solvedExampleIds={solvedTopologyIds}
              onAddDevice={(kind) => void handleAddDevice(kind)}
              onToggleConnect={() =>
                setToolMode((m) =>
                  m.kind === "connect" ? { kind: "select" } : { kind: "connect" }
                )
              }
              onQuickProbe={handleQuickProbeTrigger}
              onSelect={selectAndRevealConfig}
              onOpenExample={(file) => void handleOpenExample(file)}
            />
            <div
              className={`resize-handle ${resizingSide === "left" ? "dragging" : ""}`}
              title="拖拽调整左栏宽度"
              onMouseDown={(e) => startPanelResize("left", e.clientX)}
            />
          </>
        )}

        {layoutMode === "focus" && (
          <FocusIconRail
            hasDoc={Boolean(activeTab)}
            isLegacy={activeTab?.doc.format_version === 1}
            examplesOpen={examplesPopoverOpen}
            toolMode={toolMode}
            onAddDevice={(kind) => void handleAddDevice(kind)}
            onToggleConnect={() =>
              setToolMode((m) =>
                m.kind === "connect" ? { kind: "select" } : { kind: "connect" }
              )
            }
            onQuickProbe={handleQuickProbeTrigger}
            onOpenScenario={() => selectAndRevealConfig({ kind: "scenario" })}
            onToggleExamples={() => setExamplesPopoverOpen((v) => !v)}
          />
        )}

        {/* Floating Examples Popover for Playful & Focus modes (only when a canvas tab is open) */}
        {activeTab && examplesPopoverOpen && layoutMode !== "workbench" && (
          <ExamplesPopover
            layoutMode={layoutMode}
            examples={examples}
            solvedExampleIds={solvedTopologyIds}
            onOpenExample={(file) => void handleOpenExample(file)}
            onClose={() => setExamplesPopoverOpen(false)}
          />
        )}

        {/* Single Canvas 2D instance strictly for the active tab */}
        {activeTab ? (
          <TopologyCanvas
            key={activeTab.documentId}
            documentId={activeTab.documentId}
            doc={activeTab.doc}
            persistedNodes={activeTab.presentation.nodes}
            persistedViewport={activeTab.presentation.viewport}
            selection={activeTab.selection}
            toolMode={toolMode}
            overlay={canvasOverlay}
            layoutModeKey={`${layoutMode}-${drawerOpen ? "open" : "closed"}-${committedRightWidth}`}
            rightInset={layoutMode === "playful" && drawerOpen ? rightWidth + 24 : 0}
            bottomDock={
              layoutMode === "playful" ? (
                <PlayfulBottomDock
                  hasDoc={Boolean(activeTab)}
                  isLegacy={activeTab.doc.format_version === 1}
                  toolMode={toolMode}
                  examplesCount={examples.length}
                  examplesOpen={examplesPopoverOpen}
                  onAddDevice={(kind) => void handleAddDevice(kind)}
                  onToggleConnect={() =>
                    setToolMode((m) =>
                      m.kind === "connect" ? { kind: "select" } : { kind: "connect" }
                    )
                  }
                  onQuickProbe={handleQuickProbeTrigger}
                  onOpenScenario={() => selectAndRevealConfig({ kind: "scenario" })}
                  onToggleExamples={() => setExamplesPopoverOpen((v) => !v)}
                />
              ) : undefined
            }
            onSelect={selectAndRevealConfig}
            onCommitLayout={(nodes, viewport) =>
              void commitPresentation(activeTab.documentId, nodes, viewport)
            }
            onCreateLink={(a, b) => void handleCreateLink(a, b)}
            onDropNewDevice={(kind, pos, vp) =>
              void handleAddDevice(kind, pos, vp)
            }
            onDeleteDevice={(id) => void handleDeleteDevice(id)}
            onDeleteLink={(id) => void handleDeleteLink(id)}
            onQuickProbeComplete={(s, d) => void handleQuickProbeComplete(s, d)}
            onOpenFaultTarget={handleOpenFaultTargetFromCanvas}
            onToolModeChange={setToolMode}
          />
        ) : (
          <main className="canvas-stage welcome-stage">
            <div className="welcome-book">
              <div className="welcome-hero">
                <WelcomeSketchBanner />
                <h2 className="welcome-title">
                  Reliner · 轻量简笔网络排障手账
                </h2>
                <p className="muted" style={{ margin: "0 auto", maxWidth: 520 }}>
                  搭出草图 → 发出数据包 → 看它在哪一口被拦 → 改配置并重跑验证。
                  所有画布共用一个常驻 <code className="mono">reliner.exe</code> 离散事件引擎。
                </p>
                <div className="welcome-actions">
                  {examples.some((e) => e.file_name === "vlan-lab.json") && (
                    <button
                      className="btn btn-mint"
                      disabled={!handshake}
                      onClick={() => void handleOpenExample("vlan-lab.json")}
                    >
                      <Play size={14} /> 开始第 1 关：VLAN 漏放排障
                    </button>
                  )}
                  <button
                    className="btn"
                    disabled={!handshake}
                    onClick={() => void handleNewLab()}
                  >
                    <FilePlus2 size={14} /> 新建空白草图
                  </button>
                  <button
                    className="btn"
                    disabled={!handshake}
                    onClick={() => void handleOpenFile()}
                  >
                    <FolderOpen size={14} /> 打开本地 JSON
                  </button>
                </div>
              </div>

              {examples.length > 0 && (
                <div>
                  <div className="panel-section-title" style={{ marginBottom: 10 }}>
                    <span>选择内置排障关卡（{solvedExamplesCount}/{examples.length} 已通关）</span>
                    <span className="small muted">点开即玩 · 副本编辑不改原文件</span>
                  </div>
                  <div className="level-grid">
                    {examples.map((ex) => {
                      const meta = getExampleCategory(ex.id);
                      const solved = solvedTopologyIds.has(ex.id);
                      return (
                        <button
                          key={ex.file_name}
                          className={`example-card ${solved ? "solved" : ""}`}
                          style={{ marginBottom: 0 }}
                          disabled={!handshake}
                          onClick={() => void handleOpenExample(ex.file_name)}
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
                          <div className="small muted mono" style={{ marginTop: 5 }}>
                            {ex.device_count} 设备 · {ex.probe_count} 探测
                          </div>
                        </button>
                      );
                    })}
                  </div>
                </div>
              )}
            </div>
          </main>
        )}

        {/* Playful mode floating button to re-open the contextual drawer when closed */}
        {activeTab && layoutMode === "playful" && !drawerOpen && (
          <button
            className="btn btn-sm drawer-reopen-btn"
            onClick={() => setDrawerOpen(true)}
            title="展开排障结果与配置抽屉"
          >
            <PanelRightOpen size={14} /> 排障手账
          </button>
        )}

        {/* Right Panel: resizable docked in Workbench/Focus, floating sketchbook drawer in Playful */}
        {activeTab && showRightPanel && (
          <>
            {layoutMode !== "playful" && (
              <div
                className={`resize-handle ${resizingSide === "right" ? "dragging" : ""}`}
                title="拖拽调整右栏宽度"
                onMouseDown={(e) => startPanelResize("right", e.clientX)}
              />
            )}
            <RightPanel
              width={rightWidth}
              isWide={rightWidth >= 540}
              floating={layoutMode === "playful"}
              onStartResize={(startX) => startPanelResize("right", startX)}
              onToggleWide={() => {
                const desired = rightWidthRef.current >= 540 ? 448 : 600;
                const clamped = clampPanelWidths(
                  window.innerWidth,
                  layoutMode,
                  leftWidthRef.current,
                  desired
                );
                setRightWidth(clamped.rightWidth);
                setCommittedRightWidth(clamped.rightWidth);
              }}
              onCloseDrawer={() => setDrawerOpen(false)}
              docKey={activeTab.documentId}
              doc={activeTab.doc}
              activeTab={activeTab.rightTab}
              onChangeTab={(rt: RightPanelTab) => {
                updateTab(activeTab.documentId, (t) => ({ ...t, rightTab: rt }));
                if (
                  rt === "compare" &&
                  activeTab.compareBaselineId &&
                  activeTab.compareCandidateId &&
                  activeTab.compareBaselineId !== activeTab.compareCandidateId &&
                  !activeTab.comparison
                ) {
                  void handleRunCompare(
                    activeTab.documentId,
                    activeTab.compareBaselineId,
                    activeTab.compareCandidateId
                  );
                }
              }}
              selection={activeTab.selection}
              onSelect={(target) =>
                updateTab(activeTab.documentId, (t) => ({ ...t, selection: target }))
              }
              onChangeDoc={(nextDoc) =>
                void commitExperiment(activeTab.documentId, nextDoc)
              }
              onDeleteDevice={(id) => void handleDeleteDevice(id)}
              onDeleteLink={(id) => void handleDeleteLink(id)}
              onSaveAndRerun={() => void handleQuickRerun()}
              rerunLabel={activeTab.info.path ? "保存并重跑" : "重跑验证"}
              validation={activeTab.validation}
              effectiveSettings={activeTab.effectiveSettings}
              onLoadEffectiveSettings={() =>
                void handleLoadEffectiveSettings(activeTab.documentId)
              }
              runs={activeTab.runs}
              activeRun={activeRun}
              selectedProbeId={activeTab.selectedProbeId}
              selectedStpEventId={activeTab.selectedStpEventId}
              onSelectRun={(runId) => {
                const chosen = activeTab.runs.find((r) => r.runId === runId);
                const latestStp =
                  chosen && chosen.stpSummaries.length > 0
                    ? chosen.stpSummaries[chosen.stpSummaries.length - 1].event_id
                    : null;
                updateTab(activeTab.documentId, (t) => ({
                  ...t,
                  activeRunId: runId,
                  selectedProbeId: chosen?.probes[0]?.id ?? null,
                  selectedStpEventId: latestStp,
                }));
              }}
              onSelectProbe={(probeId) => {
                updateTab(activeTab.documentId, (t) => ({
                  ...t,
                  selectedProbeId: probeId,
                }));
                if (activeTab.activeRunId) {
                  void ensureProbeEvidenceLoaded(
                    activeTab.documentId,
                    activeTab.activeRunId,
                    probeId
                  );
                }
              }}
              onLoadMoreProbes={(runId) => {
                const run = activeTab.runs.find((r) => r.runId === runId);
                if (!run || run.probesNextOffset === null) return;
                void backendClient
                  .runProbes(runId, run.probesNextOffset, 64)
                  .then((page) =>
                    updateRun(activeTab.documentId, runId, (prev) => ({
                      ...prev,
                      probes: [...prev.probes, ...page.entries],
                      probesNextOffset: page.next_offset,
                    }))
                  )
                  .catch((err) => {
                    if (err instanceof BackendError && err.code === "unknown_run") {
                      markRunEvicted(activeTab.documentId, runId);
                      return;
                    }
                    handleBackendError(err, activeTab.documentId);
                  });
              }}
              onLoadMoreEvidence={(runId) => {
                const run = activeTab.runs.find((r) => r.runId === runId);
                if (!run || run.evidenceNextOffset === null) return;
                void backendClient
                  .runEvidence(runId, run.evidenceNextOffset, 128)
                  .then((page) =>
                    updateRun(activeTab.documentId, runId, (prev) => {
                      const seen = new Set(prev.evidence.map((e) => e.id));
                      const extra = page.entries.filter((e) => !seen.has(e.id));
                      return {
                        ...prev,
                        evidence: [...prev.evidence, ...extra],
                        evidenceNextOffset: page.next_offset,
                      };
                    })
                  )
                  .catch((err) => {
                    if (err instanceof BackendError && err.code === "unknown_run") {
                      markRunEvicted(activeTab.documentId, runId);
                      return;
                    }
                    handleBackendError(err, activeTab.documentId);
                  });
              }}
              onLoadTrace={(runId, reset) =>
                void handleLoadTrace(activeTab.documentId, runId, reset)
              }
              onSelectStpEvent={(runId, eventId) =>
                void handleSelectStpEvent(activeTab.documentId, runId, eventId)
              }
              onLoadImportGaps={(runId, probeId) =>
                void handleLoadImportGaps(activeTab.documentId, runId, probeId)
              }
              compareBaselineId={activeTab.compareBaselineId}
              compareCandidateId={activeTab.compareCandidateId}
              comparison={activeTab.comparison}
              compareLoading={activeTab.compareLoading}
              compareError={activeTab.compareError}
              onChangeCompareBaseline={(runId) =>
                updateTab(activeTab.documentId, (t) => ({
                  ...t,
                  compareBaselineId: runId,
                  comparison: null,
                }))
              }
              onChangeCompareCandidate={(runId) =>
                updateTab(activeTab.documentId, (t) => ({
                  ...t,
                  compareCandidateId: runId,
                  comparison: null,
                }))
              }
              onRunCompare={(b, c) =>
                void handleRunCompare(activeTab.documentId, b, c, 0)
              }
              onLoadMoreCompare={() => {
                if (!activeTab.comparison?.next_offset) return;
                void handleRunCompare(
                  activeTab.documentId,
                  activeTab.compareBaselineId,
                  activeTab.compareCandidateId,
                  activeTab.comparison.next_offset
                );
              }}
              onInspectRunEvidence={async (runId, evidenceId) => {
                const docId = activeTab.documentId;
                const run = tabsRef.current
                  .find((t) => t.documentId === docId)
                  ?.runs.find((r) => r.runId === runId);
                if (!run || run.evicted) {
                  setNotice({
                    kind: "warn",
                    text: `运行 #${runId} 已从工作区历史池淘汰，无法回查证据 #${evidenceId}。`,
                  });
                  return;
                }
                let ev = run.evidence.find((e) => e.id === evidenceId);
                if (!ev) {
                  const num = Number(evidenceId);
                  if (Number.isInteger(num) && num >= 1) {
                    try {
                      const page = await backendClient.runEvidence(runId, num - 1, 1);
                      ev = page.entries.find((e) => e.id === evidenceId);
                      if (ev) {
                        const found = ev;
                        updateRun(docId, runId, (prev) =>
                          prev.evidence.some((x) => x.id === found.id)
                            ? prev
                            : { ...prev, evidence: [...prev.evidence, found] }
                        );
                      }
                    } catch (err) {
                      if (err instanceof BackendError && err.code === "unknown_run") {
                        markRunEvicted(docId, runId);
                        setNotice({
                          kind: "warn",
                          text: `运行 #${runId} 已从工作区历史池淘汰，无法回查证据 #${evidenceId}。`,
                        });
                        return;
                      }
                    }
                  }
                }
                updateTab(docId, (t) => ({
                  ...t,
                  activeRunId: runId,
                  rightTab: "result",
                  selectedProbeId: ev?.probe_id ?? t.selectedProbeId,
                }));
                setDrawerOpen(true);
                setNotice(
                  ev
                    ? {
                        kind: "info",
                        text: `已定位到运行 #${runId} 的证据 #${evidenceId}（使用该次运行当时的配置与位置事实）。`,
                      }
                    : {
                        kind: "warn",
                        text: `已切回运行 #${runId}，但未能加载证据 #${evidenceId}。`,
                      }
                );
              }}
            />
          </>
        )}
      </div>

      <StatusBar
        handshake={handshake}
        connected={Boolean(handshake)}
        error={connectionError}
        activeDocInfo={activeTab?.info ?? null}
        openDocCount={tabs.length}
        layoutMode={layoutMode}
        onRestartBackend={() => void handleRestartBackend(false)}
      />
    </div>
  );
};
