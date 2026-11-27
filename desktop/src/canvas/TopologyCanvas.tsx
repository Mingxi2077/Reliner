import React, { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  Crosshair,
  Maximize2,
  Minus,
  Play,
  Plus,
  RotateCcw,
  Sparkles,
  Wrench,
  X,
} from "lucide-react";
import type {
  DeviceSpec,
  LinkSpec,
  NativeLabDocument,
  NodePosition,
  ViewportState,
} from "../model/document";
import { listAllPorts } from "../model/document";
import { buildPortOwnerMap, ensureNodePositions } from "./layout";
import type { ProbeDiagnosis } from "../protocol/diagnosis";
import type {
  InputError,
  LinkEvent,
  StpBridgeState,
  StpLinkState,
  StpPortRole,
} from "../protocol/types";
import {
  drawCuteHostCharacter,
  drawCuteRouterCharacter,
  drawCuteSwitchCharacter,
  drawDoodleSparkles,
  drawFlatPill,
  drawRedCrayonCross,
  drawRoundedRectPath,
  drawWingedPacket,
  fitText,
  INK,
  PAPER,
  PAPER_DOT,
  type DeviceMood,
} from "./sketch";
import { shortFrameLabel, type PacketPlaybackPlan } from "./packetPlayback";

export type CanvasToolMode =
  | { kind: "select" }
  | { kind: "connect"; fromDeviceId?: string; fromPortId?: string }
  | { kind: "probe"; sourceDeviceId?: string };

export type SelectionTarget =
  | { kind: "none" }
  | { kind: "device"; deviceId: string }
  | { kind: "port"; deviceId: string; portId: string }
  | { kind: "link"; linkId: string }
  | { kind: "scenario" };

export interface CanvasOverlayState {
  diagnosis: ProbeDiagnosis | null;
  probeSourceId?: string;
  probeDestId?: string;
  validationError: InputError | null;
  stpBridges?: StpBridgeState[];
  stpLinks?: StpLinkState[];
  linkEvents?: LinkEvent[];
  playback: PacketPlaybackPlan | null;
  playbackKey: string;
}

interface TopologyCanvasProps {
  documentId: string;
  doc: NativeLabDocument;
  persistedNodes: Record<string, NodePosition>;
  persistedViewport?: ViewportState;
  selection: SelectionTarget;
  toolMode: CanvasToolMode;
  overlay: CanvasOverlayState;
  layoutModeKey?: string;
  rightInset?: number;
  bottomDock?: React.ReactNode;
  onSelect: (target: SelectionTarget) => void;
  onCommitLayout: (
    nodes: Record<string, NodePosition>,
    viewport: ViewportState
  ) => void;
  onCreateLink: (portA: string, portB: string) => void;
  onDropNewDevice: (
    kind: DeviceSpec["kind"],
    pos: NodePosition,
    viewport: ViewportState
  ) => void;
  onDeleteDevice?: (deviceId: string) => void;
  onDeleteLink?: (linkId: string) => void;
  onQuickProbeComplete: (sourceDeviceId: string, destDeviceId: string) => void;
  onOpenFaultTarget?: () => void;
  onToolModeChange: (mode: CanvasToolMode) => void;
}

/** Hit-box & character center anchor dimensions for B1-Beta free-standing sprites */
const NODE_W = 112;
const NODE_H = 78;

interface HitRect {
  x: number;
  y: number;
  w: number;
  h: number;
}

interface PortBadgeHit extends HitRect {
  portId: string;
  deviceId: string;
}

interface LinkCurveGeom {
  link: LinkSpec;
  x1: number;
  y1: number;
  cx: number;
  cy: number;
  x2: number;
  y2: number;
  midX: number;
  midY: number;
  portA: { portId: string; deviceId: string; label: string; x: number; y: number; edgeX: number; edgeY: number };
  portB: { portId: string; deviceId: string; label: string; x: number; y: number; edgeX: number; edgeY: number };
}

function quadPoint(
  t: number,
  x1: number,
  y1: number,
  cx: number,
  cy: number,
  x2: number,
  y2: number
) {
  const mt = 1 - t;
  return {
    x: mt * mt * x1 + 2 * mt * t * cx + t * t * x2,
    y: mt * mt * y1 + 2 * mt * t * cy + t * t * y2,
  };
}

function distToSegment(
  px: number,
  py: number,
  ax: number,
  ay: number,
  bx: number,
  by: number
): number {
  const dx = bx - ax;
  const dy = by - ay;
  const lenSq = dx * dx + dy * dy;
  if (lenSq < 1e-6) return Math.hypot(px - ax, py - ay);
  const t = Math.max(0, Math.min(1, ((px - ax) * dx + (py - ay) * dy) / lenSq));
  return Math.hypot(px - (ax + t * dx), py - (ay + t * dy));
}

function distToQuad(
  px: number,
  py: number,
  x1: number,
  y1: number,
  cx: number,
  cy: number,
  x2: number,
  y2: number
): number {
  let best = Infinity;
  const steps = 18;
  let prev = quadPoint(0, x1, y1, cx, cy, x2, y2);
  for (let i = 1; i <= steps; i++) {
    const pt = quadPoint(i / steps, x1, y1, cx, cy, x2, y2);
    const d = distToSegment(px, py, prev.x, prev.y, pt.x, pt.y);
    if (d < best) best = d;
    prev = pt;
  }
  return best;
}

function ellipseBoundaryPoint(
  cx: number,
  cy: number,
  rx: number,
  ry: number,
  tx: number,
  ty: number
) {
  const dx = tx - cx;
  const dy = ty - cy;
  if (Math.abs(dx) < 0.001 && Math.abs(dy) < 0.001) return { x: cx, y: cy };
  const angle = Math.atan2(dy, dx);
  return {
    x: cx + Math.cos(angle) * rx,
    y: cy + Math.sin(angle) * ry,
  };
}

function formatRateShort(rateBps: string): string {
  const n = Number(rateBps);
  if (!Number.isFinite(n)) return `${rateBps}bps`;
  if (n >= 1e9) return `${(n / 1e9).toFixed(n % 1e9 === 0 ? 0 : 1)}G`;
  if (n >= 1e6) return `${(n / 1e6).toFixed(n % 1e6 === 0 ? 0 : 1)}M`;
  if (n >= 1e3) return `${(n / 1e3).toFixed(0)}K`;
  return `${n}b`;
}

export const TopologyCanvas: React.FC<TopologyCanvasProps> = ({
  documentId,
  doc,
  persistedNodes,
  persistedViewport,
  selection,
  toolMode,
  overlay,
  layoutModeKey,
  rightInset = 0,
  bottomDock,
  onSelect,
  onCommitLayout,
  onCreateLink,
  onDropNewDevice,
  onDeleteDevice,
  onDeleteLink,
  onQuickProbeComplete,
  onOpenFaultTarget,
  onToolModeChange,
}) => {
  const containerRef = useRef<HTMLDivElement | null>(null);
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const [size, setSize] = useState({ width: 0, height: 0 });
  const hasMeasuredRef = useRef(false);
  const userMovedViewportRef = useRef(false);
  const autoFitDocRef = useRef<string | null>(null);

  const computedBaseNodes = useMemo(
    () => ensureNodePositions(doc, persistedNodes),
    [doc, persistedNodes]
  );

  const [localNodes, setLocalNodes] =
    useState<Record<string, NodePosition>>(computedBaseNodes);
  const localNodesRef = useRef<Record<string, NodePosition>>(computedBaseNodes);
  localNodesRef.current = localNodes;

  const [viewport, setViewport] = useState<ViewportState>(
    persistedViewport ?? { x: 40, y: 40, zoom: 1 }
  );
  const viewportRef = useRef<ViewportState>(viewport);
  viewportRef.current = viewport;

  const playbackRef = useRef<PacketPlaybackPlan | null>(overlay.playback);
  playbackRef.current = overlay.playback;

  const canvasSizeRef = useRef<{ w: number; h: number; dpr: number }>({
    w: 0,
    h: 0,
    dpr: 1,
  });
  const dotPatternRef = useRef<{
    step: number;
    pattern: CanvasPattern | null;
  }>({ step: 0, pattern: null });
  const portPickerRef = useRef<HTMLDivElement | null>(null);
  const contextMenuRef = useRef<HTMLDivElement | null>(null);
  const appliedViewportDocRef = useRef<string | null>(null);

  useEffect(() => {
    setLocalNodes(computedBaseNodes);
    localNodesRef.current = computedBaseNodes;
  }, [computedBaseNodes]);

  useEffect(() => {
    if (appliedViewportDocRef.current !== documentId) {
      appliedViewportDocRef.current = documentId;
      userMovedViewportRef.current = false;
      if (persistedViewport) {
        setViewport(persistedViewport);
        viewportRef.current = persistedViewport;
      }
    }
  }, [documentId, persistedViewport]);

  const dragRef = useRef<
    | null
    | {
        kind: "node";
        deviceId: string;
        startWorldX: number;
        startWorldY: number;
        origX: number;
        origY: number;
        moved: boolean;
      }
    | {
        kind: "pan";
        startScreenX: number;
        startScreenY: number;
        origVx: number;
        origVy: number;
        moved: boolean;
      }
  >(null);

  const [hoverInfo, setHoverInfo] = useState<string | null>(null);
  const [toolHint, setToolHint] = useState<string | null>(null);
  const toolHintTimerRef = useRef<number | null>(null);
  const showToolHint = useCallback((msg: string) => {
    setToolHint(msg);
    if (toolHintTimerRef.current !== null) {
      window.clearTimeout(toolHintTimerRef.current);
    }
    toolHintTimerRef.current = window.setTimeout(() => {
      setToolHint(null);
      toolHintTimerRef.current = null;
    }, 3400);
  }, []);

  useEffect(
    () => () => {
      if (toolHintTimerRef.current !== null) {
        window.clearTimeout(toolHintTimerRef.current);
      }
    },
    []
  );

  const [hoverDeviceId, setHoverDeviceId] = useState<string | null>(null);
  const [cursorWorld, setCursorWorld] = useState<{ x: number; y: number } | null>(
    null
  );
  const [portPicker, setPortPicker] = useState<{
    device: DeviceSpec;
    screenX: number;
    screenY: number;
    stage: "from" | "to";
  } | null>(null);
  const [contextMenu, setContextMenu] = useState<
    | null
    | {
        screenX: number;
        screenY: number;
        worldX: number;
        worldY: number;
        target:
          | { kind: "empty" }
          | { kind: "device"; device: DeviceSpec }
          | { kind: "link"; link: LinkSpec };
      }
  >(null);

  useEffect(() => {
    setToolHint(null);
    if (toolMode.kind !== "connect") {
      setPortPicker(null);
    }
  }, [toolMode.kind]);

  // Dismiss portPicker / contextMenu on Escape or outside pointerdown
  useEffect(() => {
    if (!portPicker && !contextMenu) return;
    const onKey = (ev: KeyboardEvent) => {
      if (ev.key === "Escape") {
        setPortPicker(null);
        setContextMenu(null);
      }
    };
    const onDown = (ev: PointerEvent) => {
      const target = ev.target as Node | null;
      if (
        portPickerRef.current?.contains(target) ||
        contextMenuRef.current?.contains(target) ||
        canvasRef.current?.contains(target)
      ) {
        return;
      }
      setPortPicker(null);
      setContextMenu(null);
    };
    window.addEventListener("keydown", onKey);
    window.addEventListener("pointerdown", onDown);
    return () => {
      window.removeEventListener("keydown", onKey);
      window.removeEventListener("pointerdown", onDown);
    };
  }, [portPicker, contextMenu]);

  // Discrete 2-3 keyframe flipbook tick (~2.8fps, paused when hidden) keeps sprites gently alive
  const [flipTick, setFlipTick] = useState(0);
  useEffect(() => {
    if (doc.devices.length === 0) return;
    const id = window.setInterval(() => {
      if (!document.hidden) {
        setFlipTick((t) => (t + 1) % 6000);
      }
    }, 360);
    return () => window.clearInterval(id);
  }, [doc.devices.length]);

  // Smooth packet flight animation (depends ONLY on playbackKey + replayNonce so clicks/drags never restart flight!)
  const [animReplayNonce, setAnimReplayNonce] = useState(0);
  const [animState, setAnimState] = useState({
    flightProgress: 1,
    impactAge: 1,
    timeSec: 0,
    playing: false,
  });

  useEffect(() => {
    const plan = playbackRef.current;
    if (!plan || overlay.playbackKey === "none") {
      setAnimState({ flightProgress: 1, impactAge: 1, timeSec: 0, playing: false });
      return;
    }

    const totalHops = plan.hops.length;
    const msPerHop = totalHops <= 4 ? 540 : totalHops <= 8 ? 360 : 250;
    const flightMs = totalHops > 0 ? Math.max(880, totalHops * msPerHop) : 0;
    const startTime = performance.now();
    let rafId = 0;

    const tick = (now: number) => {
      const elapsed = now - startTime;
      const timeSec = elapsed / 1000;
      const flightProgress = flightMs > 0 ? Math.min(1, elapsed / flightMs) : 1;
      const impactAge =
        flightProgress >= 1 ? Math.min(1, (elapsed - flightMs) / 680) : 0;

      setAnimState({
        flightProgress,
        impactAge,
        timeSec,
        playing: flightProgress < 1,
      });

      if (flightProgress < 1 || impactAge < 1) {
        rafId = requestAnimationFrame(tick);
      }
    };

    rafId = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(rafId);
  }, [overlay.playbackKey, animReplayNonce]);

  // ResizeObserver for crisp HiDPI Canvas 2D & accurate initial fit
  useEffect(() => {
    const el = containerRef.current;
    if (!el) return;
    const updateSize = (w: number, h: number) => {
      if (w >= 160 && h >= 160) {
        hasMeasuredRef.current = true;
        setSize((prev) =>
          prev.width === w && prev.height === h ? prev : { width: w, height: h }
        );
      }
    };
    const rect = el.getBoundingClientRect();
    updateSize(Math.floor(rect.width), Math.floor(rect.height));

    const observer = new ResizeObserver((entries) => {
      for (const entry of entries) {
        updateSize(
          Math.floor(entry.contentRect.width),
          Math.floor(entry.contentRect.height)
        );
      }
    });
    observer.observe(el);
    return () => observer.disconnect();
  }, []);

  const allPorts = useMemo(() => listAllPorts(doc), [doc]);
  const portOwnerMap = useMemo(() => buildPortOwnerMap(doc), [doc]);

  const fitToView = useCallback(
    (
      explicitNodes?: Record<string, NodePosition>,
      explicitSize?: { width: number; height: number }
    ) => {
      const dims = explicitSize ?? size;
      if (dims.width < 160 || dims.height < 160) return;
      const nodeSource = explicitNodes ?? localNodesRef.current;
      const positions = doc.devices
        .map((d) => nodeSource[d.id])
        .filter((p): p is NodePosition => Boolean(p));
      if (positions.length === 0) {
        const nextVp = { x: 40, y: 40, zoom: 1 };
        setViewport(nextVp);
        viewportRef.current = nextVp;
        return;
      }
      let minX = Infinity;
      let minY = Infinity;
      let maxX = -Infinity;
      let maxY = -Infinity;
      for (const p of positions) {
        minX = Math.min(minX, p.x - 16);
        minY = Math.min(minY, p.y - 36);
        maxX = Math.max(maxX, p.x + NODE_W + 32);
        maxY = Math.max(maxY, p.y + NODE_H + 34);
      }
      const visibleW = Math.max(240, dims.width - rightInset);
      const padX = 56;
      const padTop = 76;
      const padBottom = bottomDock ? 112 : 76;
      const contentW = Math.max(240, maxX - minX + padX * 2);
      const contentH = Math.max(180, maxY - minY + padTop + padBottom);
      const zoom = Math.max(
        0.34,
        Math.min(1.18, Math.min(visibleW / contentW, dims.height / contentH))
      );
      const cx = (minX + maxX) / 2;
      const cy = (minY + maxY) / 2;
      const verticalBias = bottomDock ? -12 : 0;
      const nextVp = {
        zoom,
        x: Math.round(visibleW / 2 - cx * zoom),
        y: Math.round(dims.height / 2 + verticalBias - cy * zoom),
      };
      setViewport(nextVp);
      viewportRef.current = nextVp;
    },
    [doc.devices, size, bottomDock, rightInset]
  );

  useEffect(() => {
    if (!hasMeasuredRef.current || size.width < 160 || doc.devices.length === 0) return;
    if (!persistedViewport && autoFitDocRef.current !== documentId) {
      autoFitDocRef.current = documentId;
      fitToView(computedBaseNodes, size);
    }
  }, [documentId, persistedViewport, size, doc.devices.length, computedBaseNodes, fitToView]);

  useEffect(() => {
    if (!hasMeasuredRef.current || size.width < 160 || doc.devices.length === 0) return;
    if (!dragRef.current && !userMovedViewportRef.current) {
      fitToView(localNodesRef.current, size);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [layoutModeKey, size.width, size.height]);

  // Compute clean outer-edge link curves + inline port pill coordinates in O(L)
  const linkGeoms = useMemo<LinkCurveGeom[]>(() => {
    const pairCounts = new Map<string, LinkSpec[]>();
    for (const link of doc.links) {
      const a = portOwnerMap.get(link.ports[0])?.device.id;
      const b = portOwnerMap.get(link.ports[1])?.device.id;
      if (!a || !b) continue;
      const key = a < b ? `${a}|${b}` : `${b}|${a}`;
      const list = pairCounts.get(key) ?? [];
      list.push(link);
      pairCounts.set(key, list);
    }

    const geoms: LinkCurveGeom[] = [];
    for (const link of doc.links) {
      const ownerA = portOwnerMap.get(link.ports[0]);
      const ownerB = portOwnerMap.get(link.ports[1]);
      if (!ownerA || !ownerB) continue;
      const posA = localNodes[ownerA.device.id];
      const posB = localNodes[ownerB.device.id];
      if (!posA || !posB) continue;

      const cAx = posA.x + NODE_W / 2;
      const cAy = posA.y + NODE_H / 2;
      const cBx = posB.x + NODE_W / 2;
      const cBy = posB.y + NODE_H / 2;

      const key =
        ownerA.device.id < ownerB.device.id
          ? `${ownerA.device.id}|${ownerB.device.id}`
          : `${ownerB.device.id}|${ownerA.device.id}`;
      const group = pairCounts.get(key) ?? [link];
      const idx = group.indexOf(link);
      const offsetIndex = idx - (group.length - 1) / 2;

      const dx = cBx - cAx;
      const dy = cBy - cAy;
      const len = Math.hypot(dx, dy) || 1;
      const nx = -dy / len;
      const ny = dx / len;
      const bend = offsetIndex * 54;

      const midTargetX = (cAx + cBx) / 2 + nx * bend;
      const midTargetY = (cAy + cBy) / 2 + ny * bend;

      const rxA = ownerA.device.kind === "host" ? 41 : 45;
      const ryA = ownerA.device.kind === "host" ? 31 : 27;
      const rxB = ownerB.device.kind === "host" ? 41 : 45;
      const ryB = ownerB.device.kind === "host" ? 31 : 27;

      const rawEdgeA = ellipseBoundaryPoint(cAx, cAy, rxA, ryA, midTargetX, midTargetY);
      const rawEdgeB = ellipseBoundaryPoint(cBx, cBy, rxB, ryB, midTargetX, midTargetY);
      const centerDist = Math.hypot(cBx - cAx, cBy - cAy);
      const useCenters = centerDist < rxA + rxB + 12;
      const edgeA = useCenters ? { x: cAx, y: cAy } : rawEdgeA;
      const edgeB = useCenters ? { x: cBx, y: cBy } : rawEdgeB;

      const cx = (edgeA.x + edgeB.x) / 2 + nx * (bend * 0.72);
      const cy = (edgeA.y + edgeB.y) / 2 + ny * (bend * 0.72);

      const mid = quadPoint(0.5, edgeA.x, edgeA.y, cx, cy, edgeB.x, edgeB.y);
      const edgeDist = Math.hypot(edgeB.x - edgeA.x, edgeB.y - edgeA.y) || 1;
      const tPort = Math.min(0.28, Math.max(0.16, 27 / Math.max(edgeDist, 48)));
      const pA = quadPoint(tPort, edgeA.x, edgeA.y, cx, cy, edgeB.x, edgeB.y);
      const pB = quadPoint(1 - tPort, edgeA.x, edgeA.y, cx, cy, edgeB.x, edgeB.y);

      geoms.push({
        link,
        x1: edgeA.x,
        y1: edgeA.y,
        cx,
        cy,
        x2: edgeB.x,
        y2: edgeB.y,
        midX: mid.x,
        midY: mid.y,
        portA: {
          portId: link.ports[0],
          deviceId: ownerA.device.id,
          label: ownerA.portName,
          x: pA.x,
          y: pA.y,
          edgeX: edgeA.x,
          edgeY: edgeA.y,
        },
        portB: {
          portId: link.ports[1],
          deviceId: ownerB.device.id,
          label: ownerB.portName,
          x: pB.x,
          y: pB.y,
          edgeX: edgeB.x,
          edgeY: edgeB.y,
        },
      });
    }
    return geoms;
  }, [doc.links, localNodes, portOwnerMap]);

  const linkGeomMap = useMemo(() => {
    const map = new Map<string, LinkCurveGeom>();
    for (const g of linkGeoms) map.set(g.link.id, g);
    return map;
  }, [linkGeoms]);

  const portHitsRef = useRef<PortBadgeHit[]>([]);
  const fixButtonHitRef = useRef<HitRect | null>(null);

  const stpPortRoleMap = useMemo(() => {
    const map = new Map<string, StpPortRole>();
    for (const l of overlay.stpLinks ?? []) {
      for (const p of l.ports) {
        map.set(p.port_id, p.role);
      }
    }
    return map;
  }, [overlay.stpLinks]);

  const stpRootBridgeIds = useMemo(() => {
    const set = new Set<string>();
    for (const b of overlay.stpBridges ?? []) {
      if (b.bridge_id === b.root_bridge_id) {
        set.add(b.bridge_id);
      }
    }
    return set;
  }, [overlay.stpBridges]);

  const screenToWorld = (sx: number, sy: number) => ({
    x: (sx - viewport.x) / viewport.zoom,
    y: (sy - viewport.y) / viewport.zoom,
  });

  // Main B1-Beta Canvas 2D Render Pass
  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas || size.width < 100 || size.height < 100) return;
    const dpr = window.devicePixelRatio || 1;
    const targetW = Math.round(size.width * dpr);
    const targetH = Math.round(size.height * dpr);
    if (
      canvasSizeRef.current.w !== targetW ||
      canvasSizeRef.current.h !== targetH ||
      canvasSizeRef.current.dpr !== dpr
    ) {
      canvas.width = targetW;
      canvas.height = targetH;
      canvasSizeRef.current = { w: targetW, h: targetH, dpr };
    }
    const ctx = canvas.getContext("2d");
    if (!ctx) return;

    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.save();

    // PASS 0: Pure milk-white dot-grid paper (#fdfaf4) using cached tile CanvasPattern
    ctx.fillStyle = PAPER;
    ctx.fillRect(0, 0, size.width, size.height);

    const gridStep = 24 * viewport.zoom;
    if (gridStep >= 10) {
      const stepKey = Math.max(10, Math.round(gridStep * 4) / 4);
      if (dotPatternRef.current.step !== stepKey || !dotPatternRef.current.pattern) {
        const tileSize = Math.max(10, Math.round(stepKey));
        const tile = document.createElement("canvas");
        tile.width = tileSize;
        tile.height = tileSize;
        const tctx = tile.getContext("2d");
        if (tctx) {
          tctx.fillStyle = PAPER_DOT;
          tctx.beginPath();
          tctx.arc(1.5, 1.5, 1.0, 0, Math.PI * 2);
          tctx.fill();
        }
        dotPatternRef.current = {
          step: stepKey,
          pattern: ctx.createPattern(tile, "repeat"),
        };
      }
      if (dotPatternRef.current.pattern) {
        const tileSize = Math.max(10, Math.round(stepKey));
        const ox = ((viewport.x % tileSize) + tileSize) % tileSize;
        const oy = ((viewport.y % tileSize) + tileSize) % tileSize;
        ctx.save();
        ctx.translate(ox - 1.5, oy - 1.5);
        ctx.fillStyle = dotPatternRef.current.pattern;
        ctx.fillRect(
          -tileSize,
          -tileSize,
          size.width + tileSize * 2,
          size.height + tileSize * 2
        );
        ctx.restore();
      }
    }

    ctx.translate(viewport.x, viewport.y);
    ctx.scale(viewport.zoom, viewport.zoom);

    const faultTarget =
      overlay.diagnosis?.severity === "fault" ? overlay.diagnosis.target : undefined;
    const valLoc = overlay.validationError?.location;
    const playback = overlay.playback;
    const traversedLinks = playback?.traversedLinkIds ?? new Set<string>();
    const { flightProgress, impactAge, timeSec } = animState;
    const landed = flightProgress >= 0.96;

    // Determine currently flying hop
    let activeHopIndex = -1;
    let activeHopT = 0;
    const completedHopLinks = new Set<string>();
    if (playback && playback.hops.length > 0) {
      const exactIdx = flightProgress * playback.hops.length;
      activeHopIndex = Math.min(playback.hops.length - 1, Math.floor(exactIdx));
      activeHopT = flightProgress >= 1 ? 1 : exactIdx - Math.floor(exactIdx);
      for (let i = 0; i < activeHopIndex; i++) {
        completedHopLinks.add(playback.hops[i].linkId);
      }
      if (flightProgress >= 1) {
        completedHopLinks.add(playback.hops[playback.hops.length - 1].linkId);
      }
    }

    // PASS 1: Clean flat hand-drawn cables + soft pastel path highlight
    for (const geom of linkGeoms) {
      const { link, x1, y1, cx, cy, x2, y2 } = geom;
      const isSelected = selection.kind === "link" && selection.linkId === link.id;
      const isFaultLink =
        faultTarget?.linkId === link.id ||
        (faultTarget?.portId &&
          (link.ports[0] === faultTarget.portId ||
            link.ports[1] === faultTarget.portId) &&
          overlay.diagnosis?.causeEvidence?.fact.kind === "transmit_queue_full");
      const isValErrorLink = valLoc?.link_id === link.id;
      const isStandby = Boolean(link.standby_for);
      const isTraversed =
        flightProgress >= 0.99
          ? traversedLinks.has(link.id)
          : completedHopLinks.has(link.id);

      ctx.save();
      ctx.lineCap = "round";
      ctx.lineJoin = "round";

      // Soft highlighter marker wash behind traversed cables (strictly real backend trace)
      if (isTraversed) {
        ctx.beginPath();
        ctx.moveTo(x1, y1);
        ctx.quadraticCurveTo(cx, cy, x2, y2);
        ctx.strokeStyle =
          playback?.outcomeKind === "reachable"
            ? "rgba(134, 239, 172, 0.52)"
            : playback?.outcomeKind === "fault"
              ? "rgba(255, 240, 168, 0.72)"
              : "rgba(186, 230, 253, 0.55)";
        ctx.lineWidth = 9;
        ctx.stroke();
      } else if (isSelected) {
        ctx.beginPath();
        ctx.moveTo(x1, y1);
        ctx.quadraticCurveTo(cx, cy, x2, y2);
        ctx.strokeStyle = "rgba(147, 197, 253, 0.5)";
        ctx.lineWidth = 8;
        ctx.stroke();
      }

      ctx.beginPath();
      ctx.moveTo(x1, y1);
      ctx.quadraticCurveTo(cx, cy, x2, y2);
      if (isFaultLink) {
        ctx.strokeStyle = "#e54d42";
        ctx.lineWidth = 2.8;
      } else if (isValErrorLink) {
        ctx.strokeStyle = "#d97706";
        ctx.lineWidth = 2.6;
      } else if (isSelected) {
        ctx.strokeStyle = "#2563eb";
        ctx.lineWidth = 2.6;
      } else if (isStandby) {
        ctx.strokeStyle = "#8a8078";
        ctx.lineWidth = 2.0;
        ctx.setLineDash([6, 5]);
      } else {
        ctx.strokeStyle = INK;
        ctx.lineWidth = 2.2;
      }
      ctx.stroke();
      ctx.setLineDash([]);
      ctx.restore();
    }

    // Live marching-ants rubber-band cable in Connect mode
    if (toolMode.kind === "connect" && toolMode.fromDeviceId && cursorWorld) {
      const fromPos = localNodes[toolMode.fromDeviceId];
      if (fromPos) {
        const edgeStart = ellipseBoundaryPoint(
          fromPos.x + NODE_W / 2,
          fromPos.y + NODE_H / 2,
          44,
          30,
          cursorWorld.x,
          cursorWorld.y
        );
        ctx.save();
        ctx.strokeStyle = "#2563eb";
        ctx.lineWidth = 2.2;
        ctx.setLineDash([6, 5]);
        ctx.lineDashOffset = -(flipTick * 4) % 11;
        ctx.beginPath();
        ctx.moveTo(edgeStart.x, edgeStart.y);
        ctx.lineTo(cursorWorld.x, cursorWorld.y);
        ctx.stroke();
        ctx.setLineDash([]);
        ctx.fillStyle = "#dff2fc";
        ctx.strokeStyle = INK;
        ctx.lineWidth = 1.9;
        ctx.beginPath();
        ctx.arc(cursorWorld.x, cursorWorld.y, 4.5, 0, Math.PI * 2);
        ctx.fill();
        ctx.stroke();
        ctx.restore();
      }
    }

    // PASS 2: Free-standing B1-Beta Cute Device Characters + Clean Labels Underneath
    for (let idx = 0; idx < doc.devices.length; idx++) {
      const device = doc.devices[idx];
      const pos = localNodes[device.id];
      if (!pos) continue;

      const cx = pos.x + NODE_W / 2;
      const cy = pos.y + NODE_H / 2;

      const isSelected =
        (selection.kind === "device" && selection.deviceId === device.id) ||
        (selection.kind === "port" && selection.deviceId === device.id);
      const isHovered = hoverDeviceId === device.id;
      const isFaultDevice = faultTarget?.deviceId === device.id;
      const isValErrorDevice = valLoc?.device_id === device.id;
      const isProbeSource =
        (toolMode.kind === "probe" && toolMode.sourceDeviceId === device.id) ||
        overlay.probeSourceId === device.id;
      const isProbeDest = overlay.probeDestId === device.id;
      const isStpRoot = stpRootBridgeIds.has(device.id);
      const isReachableDone =
        landed && playback?.outcomeKind === "reachable";

      let mood: DeviceMood = "idle";
      if (isFaultDevice && landed) {
        mood = "worried";
      } else if (isReachableDone && (isProbeSource || isProbeDest)) {
        mood = "happy";
      } else if (isProbeSource) {
        mood = "source";
      } else if (isProbeDest) {
        mood = "dest";
      }

      // Discrete staggered cute blink every ~14 flipbook ticks
      const deviceTick = flipTick + idx;
      const blink = deviceTick % 14 === 13;

      // Tiny cartoon shiver right when packet bonks into faulty device!
      const shiverX =
        isFaultDevice && landed && impactAge < 0.65
          ? Math.sin(impactAge * Math.PI * 10) * (1 - impactAge / 0.65) * 4.5
          : 0;

      if (isValErrorDevice) {
        ctx.save();
        drawRoundedRectPath(ctx, cx - 50, cy - 38, 100, 78, 18);
        ctx.strokeStyle = "#d97706";
        ctx.lineWidth = 2.2;
        ctx.setLineDash([5, 4]);
        ctx.stroke();
        ctx.setLineDash([]);
        ctx.restore();
      }

      if (device.kind === "host") {
        drawCuteHostCharacter(ctx, cx, cy, {
          mood,
          selected: isSelected,
          hovered: isHovered,
          hasFlag: isProbeDest,
          blink,
          frameTick: deviceTick,
        });
      } else if (device.kind === "switch") {
        drawCuteSwitchCharacter(ctx, cx, cy, {
          mood,
          selected: isSelected,
          hovered: isHovered,
          isStpRoot,
          blink,
          frameTick: deviceTick,
          shiverX,
        });
      } else {
        drawCuteRouterCharacter(ctx, cx, cy, {
          mood,
          selected: isSelected,
          hovered: isHovered,
          blink,
          frameTick: deviceTick,
          shiverX,
        });
      }

      // Clean bold name right underneath the free-standing character (just like B1-Beta!)
      ctx.save();
      ctx.font = '800 13.5px "Segoe UI", "Microsoft YaHei UI", sans-serif';
      ctx.fillStyle = INK;
      ctx.textAlign = "center";
      ctx.textBaseline = "top";
      ctx.fillText(fitText(ctx, device.name || device.id, 128), cx, cy + 36);

      // Subtle compact secondary info below name
      ctx.font = '600 10px "Cascadia Code", Consolas, monospace';
      ctx.fillStyle = "#7a7169";
      const sub =
        device.kind === "host"
          ? `${device.ipv4}/${device.prefix_length}`
          : device.kind === "switch" && device.stp_priority !== undefined
            ? `STP ${device.stp_priority}`
            : device.kind === "router" && device.link_state
              ? "动态路由"
              : "";
      if (sub) {
        ctx.fillText(fitText(ctx, sub, 134), cx, cy + 53);
      }

      // Celebratory doodle sparkles when reachable!
      if (isReachableDone && isProbeDest && impactAge < 1) {
        drawDoodleSparkles(ctx, cx, cy - 10, "#15803d", impactAge);
      }
      ctx.restore();
    }

    // PASS 3: Inline White-Pill Port Labels + Link Mid-Badges + Red Crayon Cross on Blocked Port!
    const nextPortHits: PortBadgeHit[] = [];
    let faultCrossPt: { x: number; y: number } | null = null;

    for (const geom of linkGeoms) {
      const { link, midX, midY } = geom;
      const isSelected = selection.kind === "link" && selection.linkId === link.id;
      const isStandby = Boolean(link.standby_for);

      // Compact link midpoint tag (only when notable: standby / transmission / non-default delay / selected)
      const parts: string[] = [];
      if (isStandby) parts.push("备用");
      if (link.transmission) {
        parts.push(
          `${formatRateShort(link.transmission.rate_bps)}·Q${link.transmission.queue_frames}`
        );
      } else if (link.delay_ns !== "100") {
        parts.push(`${link.delay_ns}ns`);
      }
      if (isSelected && parts.length === 0) {
        parts.push(link.id);
      }

      if (parts.length > 0) {
        const text = parts.join(" · ");
        ctx.font = '700 9.5px "Cascadia Code", Consolas, monospace';
        const tw = ctx.measureText(text).width + 12;
        const th = 18;
        drawFlatPill(
          ctx,
          midX - tw / 2,
          midY - th / 2,
          tw,
          th,
          isSelected ? "#dff2fc" : "#fffdf8",
          isSelected ? "#2563eb" : "#8a8078",
          1.4
        );
        ctx.fillStyle = isSelected ? "#1d4ed8" : "#5c534c";
        ctx.textAlign = "center";
        ctx.textBaseline = "middle";
        ctx.fillText(text, midX, midY + 0.5);
      }

      // Clean inline port pills (just like B1-Beta's eth0 / G0/2 / G0/1!)
      for (const end of [geom.portA, geom.portB]) {
        const isPortSel =
          selection.kind === "port" && selection.portId === end.portId;
        const isFaultPort = faultTarget?.portId === end.portId;
        const isValPort = valLoc?.port_id === end.portId;
        const stpRole = stpPortRoleMap.get(end.portId);

        let badgeText = end.label;
        if (stpRole === "alternate") {
          badgeText = `⊘ ${end.label}`;
        } else if (stpRole === "root") {
          badgeText = `${end.label}·根`;
        }

        ctx.font = '700 10px "Cascadia Code", Consolas, monospace';
        const fittedBadge = fitText(ctx, badgeText, 92);
        const pw = ctx.measureText(fittedBadge).width + 12;
        const ph = 19;
        const bx = end.x - pw / 2;
        const by = end.y - ph / 2;

        drawFlatPill(
          ctx,
          bx,
          by,
          pw,
          ph,
          isPortSel
            ? "#dff2fc"
            : isFaultPort && landed
              ? "#fee2e2"
              : isValPort || stpRole === "alternate"
                ? "#fff0a8"
                : "#fffdf8",
          isPortSel
            ? "#2563eb"
            : isFaultPort && landed
              ? "#e54d42"
              : INK,
          isFaultPort || isPortSel ? 1.9 : 1.65
        );
        ctx.fillStyle =
          isFaultPort && landed
            ? "#b91c1c"
            : isPortSel
              ? "#1d4ed8"
              : INK;
        ctx.textAlign = "center";
        ctx.textBaseline = "middle";
        ctx.fillText(fittedBadge, end.x, end.y + 0.5);

        // Stamp bold red-crayon "✕" right on the blocked port entrance!
        if (isFaultPort && landed) {
          faultCrossPt = { x: end.edgeX, y: end.edgeY };
          const popScale = impactAge < 0.35 ? 0.6 + (impactAge / 0.35) * 0.5 : 1;
          drawRedCrayonCross(ctx, end.edgeX, end.edgeY, popScale);
          if (impactAge < 0.85) {
            drawDoodleSparkles(ctx, end.edgeX, end.edgeY, "#e54d42", impactAge / 0.85);
          }
        }

        nextPortHits.push({
          portId: end.portId,
          deviceId: end.deviceId,
          x: bx,
          y: by,
          w: pw,
          h: ph,
        });
      }
    }
    portHitsRef.current = nextPortHits;

    // PASS 4: Flapping Winged Packet Arc + Clean B1-Beta Comic Speech Bubble!
    fixButtonHitRef.current = null;

    if (playback && playback.hops.length > 0) {
      const renderHopIndex = !landed
        ? activeHopIndex
        : playback.outcomeKind === "fault"
          ? playback.faultHopIndex ?? -1
          : -1;
      const hop = renderHopIndex >= 0 ? playback.hops[renderHopIndex] : undefined;
      const geom = hop ? linkGeomMap.get(hop.linkId) : undefined;
      if (hop && geom) {
        const forward = geom.portA.portId === hop.fromPortId;
        const sx = forward ? geom.x1 : geom.x2;
        const sy = forward ? geom.y1 : geom.y2;
        const ex = forward ? geom.x2 : geom.x1;
        const ey = forward ? geom.y2 : geom.y1;

        // Curve gracefully along link perpendicular normal (upward on horizontal cables, sideways on vertical)
        const dx = ex - sx;
        const dy = ey - sy;
        const len = Math.hypot(dx, dy) || 1;
        let nx = -dy / len;
        let ny = dx / len;
        if (Math.abs(dx) >= Math.abs(dy)) {
          if (ny > 0) {
            nx = -nx;
            ny = -ny;
          }
        } else if (nx < 0) {
          nx = -nx;
          ny = -ny;
        }
        const lift = Math.min(62, Math.max(38, len * 0.28));
        const p0x = sx + nx * 12;
        const p0y = sy + ny * 12;
        const p2x = ex + nx * 8;
        const p2y = ey + ny * 8;
        const arcCx = (p0x + p2x) / 2 + nx * lift;
        const arcCy = (p0y + p2y) / 2 + ny * lift;

        // Draw dotted flight arc trajectory
        ctx.save();
        ctx.strokeStyle = INK;
        ctx.lineWidth = 1.9;
        ctx.setLineDash([5, 5]);
        ctx.lineCap = "round";
        ctx.beginPath();
        ctx.moveTo(p0x, p0y);
        ctx.quadraticCurveTo(arcCx, arcCy, p2x, p2y);
        ctx.stroke();
        ctx.setLineDash([]);
        ctx.restore();

        const displayT = landed ? 0.5 : activeHopT;
        const pt = quadPoint(displayT, p0x, p0y, arcCx, arcCy, p2x, p2y);
        const label = shortFrameLabel(hop.frameKind);
        const fill =
          hop.frameKind === "echo_reply"
            ? "#cff0df"
            : hop.frameKind === "echo_request"
              ? "#dff2fc"
              : "#fff0a8";
        const wingUp = landed
          ? flipTick % 2 === 0
          : Math.floor(timeSec * 8) % 2 === 0;
        const tilt = (forward ? 1 : -1) * (0.25 - displayT * 0.5) * 0.32;
        drawWingedPacket(ctx, pt.x, pt.y, label, fill, wingUp, tilt);
      }
    }

    // Speech bubble near fault point (obstacle-aware placement + full-bubble click target)
    if (playback && landed) {
      if (playback.outcomeKind === "fault" && playback.faultCallout) {
        let tipX: number | null = faultCrossPt?.x ?? null;
        let tipY: number | null = faultCrossPt ? faultCrossPt.y - 16 : null;

        if (tipX === null && playback.faultDeviceId && localNodes[playback.faultDeviceId]) {
          const p = localNodes[playback.faultDeviceId];
          tipX = p.x + NODE_W / 2;
          tipY = p.y - 4;
        } else if (tipX === null && playback.faultLinkId && linkGeomMap.has(playback.faultLinkId)) {
          const g = linkGeomMap.get(playback.faultLinkId)!;
          tipX = g.midX;
          tipY = g.midY - 12;
        }

        if (tipX !== null && tipY !== null) {
          const fixLabel = "修这里 →";
          ctx.save();
          ctx.font = '800 12px "Segoe UI", "Microsoft YaHei UI", sans-serif';
          const line1 = fitText(ctx, `${playback.faultCallout} ✕`, 210);
          const w1 = ctx.measureText(line1).width;
          ctx.font = '800 11px "Segoe UI", "Microsoft YaHei UI", sans-serif';
          const btnW = onOpenFaultTarget ? ctx.measureText(fixLabel).width + 18 : 0;
          const boxW = Math.max(w1 + 26, btnW + 26, 132);
          const boxH = onOpenFaultTarget ? 54 : 32;

          const overlapsAnyDevice = (rx: number, ry: number, rw: number, rh: number) =>
            doc.devices.some((d) => {
              const p = localNodes[d.id];
              if (!p) return false;
              return (
                rx < p.x + NODE_W + 4 &&
                rx + rw > p.x - 4 &&
                ry < p.y + NODE_H + 22 &&
                ry + rh > p.y - 8
              );
            });

          const bx = tipX - boxW * 0.36;
          let by = tipY - boxH - 16;
          let placedBelow = false;
          if (overlapsAnyDevice(bx, by, boxW, boxH)) {
            const altTipY = (faultCrossPt ? faultCrossPt.y + 18 : tipY + 62);
            const altBy = altTipY + 14;
            if (!overlapsAnyDevice(bx, altBy, boxW, boxH)) {
              by = altBy;
              tipY = altTipY;
              placedBelow = true;
            }
          }

          // Clean B1-Beta white speech bubble with rounded border
          drawRoundedRectPath(ctx, bx, by, boxW, boxH, 12);
          ctx.fillStyle = "#fffdf8";
          ctx.fill();
          ctx.strokeStyle = INK;
          ctx.lineWidth = 2.1;
          ctx.stroke();

          // Tail pointing toward blocked port
          const tailBaseX = Math.max(bx + 18, Math.min(bx + boxW - 22, tipX));
          if (!placedBelow) {
            ctx.beginPath();
            ctx.moveTo(tailBaseX - 6, by + boxH - 1.5);
            ctx.lineTo(tipX, tipY);
            ctx.lineTo(tailBaseX + 7, by + boxH - 1.5);
            ctx.closePath();
            ctx.fillStyle = "#fffdf8";
            ctx.fill();
            ctx.beginPath();
            ctx.moveTo(tailBaseX - 6, by + boxH);
            ctx.lineTo(tipX, tipY);
            ctx.lineTo(tailBaseX + 7, by + boxH);
            ctx.strokeStyle = INK;
            ctx.lineWidth = 2.1;
            ctx.lineJoin = "round";
            ctx.stroke();
          } else {
            ctx.beginPath();
            ctx.moveTo(tailBaseX - 6, by + 1.5);
            ctx.lineTo(tipX, tipY);
            ctx.lineTo(tailBaseX + 7, by + 1.5);
            ctx.closePath();
            ctx.fillStyle = "#fffdf8";
            ctx.fill();
            ctx.beginPath();
            ctx.moveTo(tailBaseX - 6, by);
            ctx.lineTo(tipX, tipY);
            ctx.lineTo(tailBaseX + 7, by);
            ctx.strokeStyle = INK;
            ctx.lineWidth = 2.1;
            ctx.lineJoin = "round";
            ctx.stroke();
          }

          // Fault callout text
          ctx.font = '800 12px "Segoe UI", "Microsoft YaHei UI", sans-serif';
          ctx.fillStyle = "#c9382e";
          ctx.textAlign = "center";
          ctx.textBaseline = "middle";
          ctx.fillText(line1, bx + boxW / 2, by + 16);

          // Clickable cute coral "[修这里 →]" pill button; entire bubble acts as click target
          if (onOpenFaultTarget && btnW > 0) {
            const fbx = bx + (boxW - btnW) / 2;
            const fby = by + 28;
            const fbh = 20;
            drawFlatPill(ctx, fbx, fby, btnW, fbh, "#f38370", INK, 1.6);
            ctx.font = '800 10.5px "Segoe UI", "Microsoft YaHei UI", sans-serif';
            ctx.fillStyle = INK;
            ctx.textAlign = "center";
            ctx.textBaseline = "middle";
            ctx.fillText(fixLabel, fbx + btnW / 2, fby + fbh / 2 + 0.5);
            fixButtonHitRef.current = { x: bx, y: by, w: boxW, h: boxH };
          }
          ctx.restore();
        }
      } else if (playback.outcomeKind === "reachable" && overlay.probeDestId) {
        const destPos = localNodes[overlay.probeDestId];
        if (destPos) {
          const stamp = `✓ 通了! ${playback.reachableNs ?? ""} ns`;
          ctx.save();
          ctx.font = '800 12px "Segoe UI", "Microsoft YaHei UI", sans-serif';
          const sw = ctx.measureText(stamp).width + 22;
          const sh = 26;
          const bx = destPos.x + NODE_W / 2 - sw / 2;
          const by = destPos.y - 36;
          drawFlatPill(ctx, bx, by, sw, sh, "#cff0df", INK, 2.0);
          ctx.fillStyle = "#14532d";
          ctx.textAlign = "center";
          ctx.textBaseline = "middle";
          ctx.fillText(stamp, bx + sw / 2, by + sh / 2 + 0.5);
          ctx.restore();
        }
      } else if (
        (playback.outcomeKind === "unlocated" ||
          playback.outcomeKind === "undetermined" ||
          playback.outcomeKind === "incomplete") &&
        (overlay.probeDestId || overlay.probeSourceId)
      ) {
        const anchorId = overlay.probeDestId ?? overlay.probeSourceId!;
        const destPos = localNodes[anchorId];
        if (destPos) {
          const msg =
            playback.outcomeKind === "undetermined"
              ? "? 覆盖不足 · 未判定"
              : playback.outcomeKind === "incomplete"
                ? "⏸ 未完成 · 触及边界"
                : "? 超时 · 尚未定位拦截点";
          ctx.save();
          ctx.font = '700 11px "Segoe UI", "Microsoft YaHei UI", sans-serif';
          const sw = ctx.measureText(msg).width + 20;
          const sh = 25;
          const bx = destPos.x + NODE_W / 2 - sw / 2;
          const by = destPos.y - 36;
          drawFlatPill(ctx, bx, by, sw, sh, "#fff0a8", INK, 1.9);
          ctx.fillStyle = INK;
          ctx.textAlign = "center";
          ctx.textBaseline = "middle";
          ctx.fillText(msg, bx + sw / 2, by + sh / 2 + 0.5);
          ctx.restore();
        }
      }
    }

    ctx.restore();
  }, [
    size,
    viewport,
    doc,
    localNodes,
    linkGeoms,
    linkGeomMap,
    selection,
    toolMode,
    overlay,
    stpPortRoleMap,
    stpRootBridgeIds,
    hoverDeviceId,
    cursorWorld,
    animState,
    flipTick,
    onOpenFaultTarget,
  ]);

  const hitTest = (sx: number, sy: number) => {
    const w = screenToWorld(sx, sy);

    const fixHit = fixButtonHitRef.current;
    if (
      fixHit &&
      w.x >= fixHit.x &&
      w.x <= fixHit.x + fixHit.w &&
      w.y >= fixHit.y &&
      w.y <= fixHit.y + fixHit.h
    ) {
      return { kind: "fix_fault" as const };
    }

    for (let i = portHitsRef.current.length - 1; i >= 0; i--) {
      const hit = portHitsRef.current[i];
      if (
        w.x >= hit.x &&
        w.x <= hit.x + hit.w &&
        w.y >= hit.y &&
        w.y <= hit.y + hit.h
      ) {
        return { kind: "port" as const, deviceId: hit.deviceId, portId: hit.portId };
      }
    }

    for (let i = doc.devices.length - 1; i >= 0; i--) {
      const d = doc.devices[i];
      const pos = localNodes[d.id];
      if (!pos) continue;
      if (
        w.x >= pos.x + 6 &&
        w.x <= pos.x + NODE_W - 6 &&
        w.y >= pos.y &&
        w.y <= pos.y + NODE_H + 14
      ) {
        return { kind: "device" as const, device: d };
      }
    }

    for (const geom of linkGeoms) {
      const dist = distToQuad(
        w.x,
        w.y,
        geom.x1,
        geom.y1,
        geom.cx,
        geom.cy,
        geom.x2,
        geom.y2
      );
      if (dist <= 10 / viewport.zoom) {
        return { kind: "link" as const, link: geom.link };
      }
    }

    return { kind: "empty" as const, world: w };
  };

  // Native non-passive wheel listener so e.preventDefault() works cleanly without browser warnings
  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const onWheel = (e: WheelEvent) => {
      e.preventDefault();
      userMovedViewportRef.current = true;
      const rect = canvas.getBoundingClientRect();
      const sx = e.clientX - rect.left;
      const sy = e.clientY - rect.top;
      const factor = e.deltaY < 0 ? 1.1 : 0.9;
      setViewport((prev) => {
        const nextZoom = Math.max(0.32, Math.min(2.2, prev.zoom * factor));
        const worldX = (sx - prev.x) / prev.zoom;
        const worldY = (sy - prev.y) / prev.zoom;
        const nextVp = {
          zoom: nextZoom,
          x: Math.round(sx - worldX * nextZoom),
          y: Math.round(sy - worldY * nextZoom),
        };
        viewportRef.current = nextVp;
        return nextVp;
      });
    };
    canvas.addEventListener("wheel", onWheel, { passive: false });
    return () => canvas.removeEventListener("wheel", onWheel);
  }, []);

  const zoomAroundVisibleCenter = (factor: number) => {
    userMovedViewportRef.current = true;
    const cx = Math.max(120, (size.width - rightInset) / 2);
    const cy = Math.max(80, size.height / 2);
    setViewport((prev) => {
      const nextZoom = Math.max(0.32, Math.min(2.2, prev.zoom * factor));
      const worldX = (cx - prev.x) / prev.zoom;
      const worldY = (cy - prev.y) / prev.zoom;
      const nextVp = {
        zoom: nextZoom,
        x: Math.round(cx - worldX * nextZoom),
        y: Math.round(cy - worldY * nextZoom),
      };
      viewportRef.current = nextVp;
      return nextVp;
    });
  };

  const startConnectFromDevice = (
    dev: DeviceSpec,
    sx: number,
    sy: number
  ) => {
    const freePorts = allPorts.filter(
      (p) => p.deviceId === dev.id && !p.connectedLinkId
    );
    if (freePorts.length === 0) {
      showToolHint(`${dev.name} 没有空闲端口，请先选中它并在右侧新增端口。`);
      return;
    }
    if (freePorts.length === 1) {
      onToolModeChange({
        kind: "connect",
        fromDeviceId: dev.id,
        fromPortId: freePorts[0].portId,
      });
    } else {
      onToolModeChange({ kind: "connect" });
      setPortPicker({
        device: dev,
        screenX: sx,
        screenY: sy,
        stage: "from",
      });
    }
  };

  const handlePointerDown = (e: React.PointerEvent<HTMLCanvasElement>) => {
    if (e.button === 2) return;
    e.currentTarget.focus({ preventScroll: true });
    const rect = e.currentTarget.getBoundingClientRect();
    const sx = e.clientX - rect.left;
    const sy = e.clientY - rect.top;
    const hit = hitTest(sx, sy);
    setPortPicker(null);
    setContextMenu(null);

    // Middle-button pan works in any tool mode
    if (e.button === 1) {
      e.preventDefault();
      dragRef.current = {
        kind: "pan",
        startScreenX: sx,
        startScreenY: sy,
        origVx: viewportRef.current.x,
        origVy: viewportRef.current.y,
        moved: false,
      };
      e.currentTarget.setPointerCapture(e.pointerId);
      return;
    }

    if (hit.kind === "fix_fault") {
      onOpenFaultTarget?.();
      return;
    }

    const hitDevice =
      hit.kind === "device"
        ? hit.device
        : hit.kind === "port"
          ? doc.devices.find((d) => d.id === hit.deviceId) ?? null
          : null;

    if (toolMode.kind === "probe") {
      if (hitDevice) {
        if (hitDevice.kind !== "host") {
          showToolHint("测连通仅支持在两台主机（Host）之间发起，请点选主机。");
          return;
        }
        if (!toolMode.sourceDeviceId) {
          onToolModeChange({ kind: "probe", sourceDeviceId: hitDevice.id });
        } else if (toolMode.sourceDeviceId === hitDevice.id) {
          showToolHint("请点选另一台主机作为目的主机。");
        } else {
          onQuickProbeComplete(toolMode.sourceDeviceId, hitDevice.id);
          onToolModeChange({ kind: "select" });
        }
        return;
      }
      if (hit.kind === "empty") {
        dragRef.current = {
          kind: "pan",
          startScreenX: sx,
          startScreenY: sy,
          origVx: viewportRef.current.x,
          origVy: viewportRef.current.y,
          moved: false,
        };
        e.currentTarget.setPointerCapture(e.pointerId);
      }
      return;
    }

    if (toolMode.kind === "connect") {
      if (hitDevice) {
        const freePorts = allPorts.filter(
          (p) => p.deviceId === hitDevice.id && !p.connectedLinkId
        );
        if (freePorts.length === 0) {
          showToolHint(`${hitDevice.name} 没有空闲端口，可在右侧属性中新增端口。`);
          return;
        }
        if (!toolMode.fromPortId) {
          if (freePorts.length === 1) {
            onToolModeChange({
              kind: "connect",
              fromDeviceId: hitDevice.id,
              fromPortId: freePorts[0].portId,
            });
          } else {
            setPortPicker({
              device: hitDevice,
              screenX: sx,
              screenY: sy,
              stage: "from",
            });
          }
        } else if (hitDevice.id === toolMode.fromDeviceId) {
          showToolHint("请点选另一台设备完成连线。");
        } else {
          if (freePorts.length === 1) {
            onCreateLink(toolMode.fromPortId, freePorts[0].portId);
            onToolModeChange({ kind: "select" });
          } else {
            setPortPicker({
              device: hitDevice,
              screenX: sx,
              screenY: sy,
              stage: "to",
            });
          }
        }
        return;
      }
      if (hit.kind === "empty") {
        dragRef.current = {
          kind: "pan",
          startScreenX: sx,
          startScreenY: sy,
          origVx: viewportRef.current.x,
          origVy: viewportRef.current.y,
          moved: false,
        };
        e.currentTarget.setPointerCapture(e.pointerId);
      }
      return;
    }

    if (hit.kind === "port") {
      onSelect({ kind: "port", deviceId: hit.deviceId, portId: hit.portId });
      return;
    }

    if (hit.kind === "device") {
      // Defer onSelect to pointerup if !drag.moved so dragging never rips open the drawer mid-drag
      const world = screenToWorld(sx, sy);
      const orig = localNodesRef.current[hit.device.id] ?? { x: 0, y: 0 };
      dragRef.current = {
        kind: "node",
        deviceId: hit.device.id,
        startWorldX: world.x,
        startWorldY: world.y,
        origX: orig.x,
        origY: orig.y,
        moved: false,
      };
      e.currentTarget.setPointerCapture(e.pointerId);
      return;
    }

    if (hit.kind === "link") {
      onSelect({ kind: "link", linkId: hit.link.id });
      return;
    }

    dragRef.current = {
      kind: "pan",
      startScreenX: sx,
      startScreenY: sy,
      origVx: viewportRef.current.x,
      origVy: viewportRef.current.y,
      moved: false,
    };
    e.currentTarget.setPointerCapture(e.pointerId);
  };

  const handlePointerMove = (e: React.PointerEvent<HTMLCanvasElement>) => {
    const rect = e.currentTarget.getBoundingClientRect();
    const sx = e.clientX - rect.left;
    const sy = e.clientY - rect.top;
    const w = screenToWorld(sx, sy);

    if (toolMode.kind === "connect" && toolMode.fromDeviceId) {
      setCursorWorld(w);
    } else if (cursorWorld) {
      setCursorWorld(null);
    }

    const drag = dragRef.current;
    if (drag) {
      if (drag.kind === "node") {
        const nx = Math.round(drag.origX + (w.x - drag.startWorldX));
        const ny = Math.round(drag.origY + (w.y - drag.startWorldY));
        if (Math.abs(nx - drag.origX) > 3 || Math.abs(ny - drag.origY) > 3) {
          drag.moved = true;
        }
        const updatedNodes = {
          ...localNodesRef.current,
          [drag.deviceId]: { x: nx, y: ny },
        };
        localNodesRef.current = updatedNodes;
        setLocalNodes(updatedNodes);
      } else {
        const dx = sx - drag.startScreenX;
        const dy = sy - drag.startScreenY;
        if (Math.abs(dx) > 2 || Math.abs(dy) > 2) {
          drag.moved = true;
          userMovedViewportRef.current = true;
        }
        const nextVp = {
          ...viewportRef.current,
          x: Math.round(drag.origVx + dx),
          y: Math.round(drag.origVy + dy),
        };
        viewportRef.current = nextVp;
        setViewport(nextVp);
      }
      return;
    }

    const hit = hitTest(sx, sy);
    if (hit.kind === "fix_fault") {
      setHoverDeviceId(null);
      setHoverInfo("点击打开故障点配置");
    } else if (hit.kind === "device") {
      setHoverDeviceId(hit.device.id);
      setHoverInfo(
        hit.device.kind === "host"
          ? `${hit.device.name} · ${hit.device.ipv4}/${hit.device.prefix_length}`
          : `${hit.device.name} (${hit.device.id})`
      );
    } else if (hit.kind === "port") {
      setHoverDeviceId(hit.deviceId);
      setHoverInfo(`点击编辑端口 ${hit.portId}`);
    } else if (hit.kind === "link") {
      setHoverDeviceId(null);
      setHoverInfo(
        `网线 ${hit.link.id} · 时延 ${hit.link.delay_ns} ns${
          hit.link.standby_for ? ` · 备用于 ${hit.link.standby_for}` : ""
        }`
      );
    } else {
      setHoverDeviceId(null);
      setHoverInfo(null);
    }
  };

  const handlePointerUp = (e: React.PointerEvent<HTMLCanvasElement>) => {
    const drag = dragRef.current;
    dragRef.current = null;
    if (e.currentTarget.hasPointerCapture(e.pointerId)) {
      e.currentTarget.releasePointerCapture(e.pointerId);
    }
    if (!drag) return;

    if (drag.kind === "node") {
      if (drag.moved) {
        onCommitLayout(localNodesRef.current, viewportRef.current);
      } else {
        onSelect({ kind: "device", deviceId: drag.deviceId });
      }
    } else if (drag.kind === "pan" && !drag.moved && toolMode.kind === "select") {
      onSelect({ kind: "none" });
    }
  };

  const handleContextMenu = (e: React.MouseEvent<HTMLCanvasElement>) => {
    e.preventDefault();
    const rect = e.currentTarget.getBoundingClientRect();
    const sx = e.clientX - rect.left;
    const sy = e.clientY - rect.top;
    const w = screenToWorld(sx, sy);
    const hit = hitTest(sx, sy);
    setPortPicker(null);

    const hitDev =
      hit.kind === "device"
        ? hit.device
        : hit.kind === "port"
          ? doc.devices.find((d) => d.id === hit.deviceId) ?? null
          : null;

    if (hitDev) {
      setContextMenu({
        screenX: sx,
        screenY: sy,
        worldX: w.x,
        worldY: w.y,
        target: { kind: "device", device: hitDev },
      });
    } else if (hit.kind === "link") {
      setContextMenu({
        screenX: sx,
        screenY: sy,
        worldX: w.x,
        worldY: w.y,
        target: { kind: "link", link: hit.link },
      });
    } else {
      setContextMenu({
        screenX: sx,
        screenY: sy,
        worldX: w.x,
        worldY: w.y,
        target: { kind: "empty" },
      });
    }
  };

  const handleDragOver = (e: React.DragEvent<HTMLDivElement>) => {
    if (e.dataTransfer.types.includes("application/x-reliner-device")) {
      e.preventDefault();
      e.dataTransfer.dropEffect = "copy";
    }
  };

  const handleDrop = (e: React.DragEvent<HTMLDivElement>) => {
    const kind = e.dataTransfer.getData("application/x-reliner-device") as
      | DeviceSpec["kind"]
      | "";
    if (!kind) return;
    e.preventDefault();
    if (kind === "router" && doc.format_version === 1) {
      showToolHint("格式 1 仅支持主机与交换机，无法拖入路由器。");
      return;
    }
    const rect = e.currentTarget.getBoundingClientRect();
    const w = screenToWorld(e.clientX - rect.left, e.clientY - rect.top);
    onDropNewDevice(
      kind,
      {
        x: Math.round(w.x - NODE_W / 2),
        y: Math.round(w.y - NODE_H / 2),
      },
      viewportRef.current
    );
  };

  const pickPortForConnect = (portId: string) => {
    if (!portPicker) return;
    if (portPicker.stage === "from") {
      onToolModeChange({
        kind: "connect",
        fromDeviceId: portPicker.device.id,
        fromPortId: portId,
      });
    } else if (toolMode.kind === "connect" && toolMode.fromPortId) {
      onCreateLink(toolMode.fromPortId, portId);
      onToolModeChange({ kind: "select" });
    }
    setPortPicker(null);
  };

  const freePortsForPicker = portPicker
    ? allPorts.filter(
        (p) => p.deviceId === portPicker.device.id && !p.connectedLinkId
      )
    : [];

  const probeSourceName =
    toolMode.kind === "probe" && toolMode.sourceDeviceId
      ? doc.devices.find((d) => d.id === toolMode.sourceDeviceId)?.name ??
        toolMode.sourceDeviceId
      : null;

  const connectFromLabel =
    toolMode.kind === "connect" && toolMode.fromPortId
      ? (() => {
          const p = allPorts.find((x) => x.portId === toolMode.fromPortId);
          return p ? `${p.deviceName}/${p.portName}` : toolMode.fromPortId;
        })()
      : null;

  return (
    <div
      ref={containerRef}
      className="canvas-stage"
      onDragOver={handleDragOver}
      onDrop={handleDrop}
    >
      <canvas
        ref={canvasRef}
        role="application"
        aria-label="网络拓扑画布"
        tabIndex={0}
        style={{
          width: Math.max(1, size.width),
          height: Math.max(1, size.height),
          display: "block",
          outline: "none",
          cursor:
            toolMode.kind === "probe" || toolMode.kind === "connect"
              ? "crosshair"
              : hoverInfo === "点击打开故障点配置"
                ? "pointer"
                : "default",
        }}
        onPointerDown={handlePointerDown}
        onPointerMove={handlePointerMove}
        onPointerUp={handlePointerUp}
        onPointerCancel={handlePointerUp}
        onContextMenu={handleContextMenu}
      />

      {/* Top-center mode hint / hover pill (centered within visible canvas area) */}
      <div
        className="canvas-hud-top"
        style={{
          left: rightInset > 0 ? `calc((100% - ${rightInset}px) / 2)` : "50%",
        }}
      >
        {toolMode.kind === "probe" && (
          <div className="canvas-hud-pill hud-active">
            <Crosshair size={14} />
            <span>
              {probeSourceName
                ? `已选源主机 ${probeSourceName}，点目的主机即刻发包`
                : "测连通：先点源主机，再点目的主机"}
            </span>
            <button
              className="btn btn-xs"
              onClick={() => onToolModeChange({ kind: "select" })}
            >
              <X size={12} /> 取消
            </button>
          </div>
        )}

        {toolMode.kind === "connect" && (
          <div className="canvas-hud-pill hud-active">
            <Sparkles size={14} />
            <span>
              {connectFromLabel
                ? `已拉出网线（${connectFromLabel}），点目标设备接上`
                : "拉网线：点击第一台设备选择接口"}
            </span>
            <button
              className="btn btn-xs"
              onClick={() => {
                setPortPicker(null);
                onToolModeChange({ kind: "select" });
              }}
            >
              <X size={12} /> 取消
            </button>
          </div>
        )}

        {toolHint && (
          <div
            className="canvas-hud-pill"
            style={{
              background: "var(--status-fault-bg)",
              borderColor: "var(--status-fault-border)",
              color: "var(--status-fault)",
            }}
          >
            {toolHint}
          </div>
        )}

        {toolMode.kind === "select" && !toolHint && hoverInfo && (
          <div className="canvas-hud-pill">{hoverInfo}</div>
        )}
      </div>

      {/* Port picker popover */}
      {portPicker && (
        <div
          ref={portPickerRef}
          className="port-picker-popover"
          style={{
            left: Math.min(portPicker.screenX + 10, Math.max(12, size.width - 235)),
            top: Math.min(portPicker.screenY + 10, Math.max(12, size.height - 240)),
          }}
        >
          <div className="popover-header">
            <span>选择 {portPicker.device.name} 空闲端口</span>
            <button className="tab-close" onClick={() => setPortPicker(null)}>
              <X size={13} />
            </button>
          </div>
          {freePortsForPicker.map((p) => (
            <button
              key={p.portId}
              className="list-item"
              onClick={() => pickPortForConnect(p.portId)}
            >
              <span className="mono">{p.portName}</span>
              <span className="muted small">{p.detail}</span>
            </button>
          ))}
        </div>
      )}

      {/* Right-click sketch menu */}
      {contextMenu && (
        <div
          ref={contextMenuRef}
          className="port-picker-popover"
          style={{
            left: Math.min(contextMenu.screenX + 6, Math.max(12, size.width - 210)),
            top: Math.min(contextMenu.screenY + 6, Math.max(12, size.height - 220)),
            minWidth: 185,
          }}
        >
          {contextMenu.target.kind === "device" && (
            <>
              <div className="popover-header">
                <span>{contextMenu.target.device.name}</span>
                <button className="tab-close" onClick={() => setContextMenu(null)}>
                  <X size={12} />
                </button>
              </div>
              <button
                className="list-item"
                onClick={() => {
                  onSelect({
                    kind: "device",
                    deviceId:
                      contextMenu.target.kind === "device"
                        ? contextMenu.target.device.id
                        : "",
                  });
                  setContextMenu(null);
                }}
              >
                <span>
                  <Wrench size={12} /> 编辑设备配置
                </span>
              </button>
              {contextMenu.target.device.kind === "host" && (
                <button
                  className="list-item"
                  onClick={() => {
                    if (contextMenu.target.kind === "device") {
                      onToolModeChange({
                        kind: "probe",
                        sourceDeviceId: contextMenu.target.device.id,
                      });
                    }
                    setContextMenu(null);
                  }}
                >
                  <span>
                    <Crosshair size={12} /> 以此为源测连通
                  </span>
                </button>
              )}
              <button
                className="list-item"
                onClick={() => {
                  if (contextMenu.target.kind === "device") {
                    startConnectFromDevice(
                      contextMenu.target.device,
                      contextMenu.screenX,
                      contextMenu.screenY
                    );
                  }
                  setContextMenu(null);
                }}
              >
                <span>拉网线连接其他设备</span>
              </button>
              {onDeleteDevice && (
                <button
                  className="list-item danger-text"
                  onClick={() => {
                    if (contextMenu.target.kind === "device") {
                      onDeleteDevice(contextMenu.target.device.id);
                    }
                    setContextMenu(null);
                  }}
                >
                  <span>删除设备</span>
                </button>
              )}
            </>
          )}

          {contextMenu.target.kind === "link" && (
            <>
              <div className="popover-header">
                <span className="mono">{contextMenu.target.link.id}</span>
                <button className="tab-close" onClick={() => setContextMenu(null)}>
                  <X size={12} />
                </button>
              </div>
              <button
                className="list-item"
                onClick={() => {
                  if (contextMenu.target.kind === "link") {
                    onSelect({ kind: "link", linkId: contextMenu.target.link.id });
                  }
                  setContextMenu(null);
                }}
              >
                <span>编辑链路参数</span>
              </button>
              {onDeleteLink && (
                <button
                  className="list-item danger-text"
                  onClick={() => {
                    if (contextMenu.target.kind === "link") {
                      onDeleteLink(contextMenu.target.link.id);
                    }
                    setContextMenu(null);
                  }}
                >
                  <span>剪断并删除网线</span>
                </button>
              )}
            </>
          )}

          {contextMenu.target.kind === "empty" && (
            <>
              <div className="popover-header">
                <span>在此处画一台设备</span>
                <button className="tab-close" onClick={() => setContextMenu(null)}>
                  <X size={12} />
                </button>
              </div>
              <button
                className="list-item"
                onClick={() => {
                  onDropNewDevice(
                    "host",
                    {
                      x: Math.round(contextMenu.worldX - NODE_W / 2),
                      y: Math.round(contextMenu.worldY - NODE_H / 2),
                    },
                    viewportRef.current
                  );
                  setContextMenu(null);
                }}
              >
                <span>+ 主机 (Host)</span>
              </button>
              <button
                className="list-item"
                onClick={() => {
                  onDropNewDevice(
                    "switch",
                    {
                      x: Math.round(contextMenu.worldX - NODE_W / 2),
                      y: Math.round(contextMenu.worldY - NODE_H / 2),
                    },
                    viewportRef.current
                  );
                  setContextMenu(null);
                }}
              >
                <span>+ 交换机 (Switch)</span>
              </button>
              <button
                className="list-item"
                disabled={doc.format_version === 1}
                title={
                  doc.format_version === 1
                    ? "格式 1 仅支持主机与交换机"
                    : "在当前位置新建一台路由器"
                }
                onClick={() => {
                  onDropNewDevice(
                    "router",
                    {
                      x: Math.round(contextMenu.worldX - NODE_W / 2),
                      y: Math.round(contextMenu.worldY - NODE_H / 2),
                    },
                    viewportRef.current
                  );
                  setContextMenu(null);
                }}
              >
                <span>
                  + 路由器 (Router){doc.format_version === 1 ? " (仅v2+)" : ""}
                </span>
              </button>
              <button
                className="list-item"
                onClick={() => {
                  userMovedViewportRef.current = false;
                  fitToView();
                  setContextMenu(null);
                }}
              >
                <span>适应画布</span>
              </button>
            </>
          )}
        </div>
      )}

      {/* Zoom + Packet Replay HUD */}
      <div className={`canvas-hud-bottom ${bottomDock ? "hud-top-left" : ""}`}>
        <div className="canvas-hud-pill" style={{ padding: "3px 6px", gap: 4 }}>
          <button
            className="btn-ghost-icon"
            title="缩小"
            onClick={() => zoomAroundVisibleCenter(1 / 1.15)}
          >
            <Minus size={12} />
          </button>
          <span className="mono small" style={{ minWidth: 38, textAlign: "center", fontWeight: 700 }}>
            {Math.round(viewport.zoom * 100)}%
          </span>
          <button
            className="btn-ghost-icon"
            title="放大"
            onClick={() => zoomAroundVisibleCenter(1.15)}
          >
            <Plus size={12} />
          </button>
          <button
            className="btn-ghost-icon"
            title="适应画布"
            onClick={() => {
              userMovedViewportRef.current = false;
              fitToView();
            }}
          >
            <Maximize2 size={12} />
          </button>
        </div>

        {overlay.playback && overlay.playback.hops.length > 0 && (
          <button
            className="btn btn-xs btn-butter canvas-hud-replay"
            title="按后端真实轨迹重播飞翔数据包"
            onClick={() => setAnimReplayNonce((n) => n + 1)}
          >
            {animState.playing ? <Play size={11} /> : <RotateCcw size={11} />}
            {animState.playing ? "飞行中…" : `重播动画 (${overlay.playback.hops.length} 跳)`}
          </button>
        )}
      </div>

      {/* Floating sleek B1-Beta bottom 5-icon pill dock (Playful mode) */}
      {bottomDock && (
        <div
          className="canvas-dock-wrap"
          style={{
            left: rightInset > 0 ? `calc((100% - ${rightInset}px) / 2)` : "50%",
          }}
        >
          {bottomDock}
        </div>
      )}
    </div>
  );
};
