import type { DeviceSpec, NativeLabDocument, NodePosition } from "../model/document";

export function buildPortOwnerMap(
  doc: NativeLabDocument
): Map<string, { device: DeviceSpec; portName: string }> {
  const map = new Map<string, { device: DeviceSpec; portName: string }>();
  for (const device of doc.devices) {
    if (device.kind === "host") {
      map.set(device.port.id, { device, portName: device.port.name });
    } else {
      for (const p of device.ports) {
        map.set(p.id, { device, portName: p.name });
      }
    }
  }
  return map;
}

/**
 * Computes a clean, deterministic left-to-right layered topology layout for devices
 * that do not yet have persisted coordinates in `presentation.nodes`.
 */
export function ensureNodePositions(
  doc: NativeLabDocument,
  existing: Record<string, NodePosition>
): Record<string, NodePosition> {
  const result: Record<string, NodePosition> = {};
  for (const d of doc.devices) {
    if (existing[d.id]) {
      result[d.id] = existing[d.id];
    }
  }
  const missing = doc.devices.filter((d) => !result[d.id]);
  if (missing.length === 0) {
    return result;
  }

  const portOwner = buildPortOwnerMap(doc);

  // Build adjacency between devices in O(L)
  const adj = new Map<string, Set<string>>();
  for (const d of doc.devices) adj.set(d.id, new Set());
  for (const link of doc.links) {
    const a = portOwner.get(link.ports[0])?.device.id;
    const b = portOwner.get(link.ports[1])?.device.id;
    if (a && b && a !== b) {
      adj.get(a)?.add(b);
      adj.get(b)?.add(a);
    }
  }

  // Pick a left anchor: first probe source, else first host, else first device
  const probeSource =
    doc.format_version === 1
      ? doc.probe?.source
      : doc.scenario?.probes[0]?.source;
  const anchor =
    (probeSource && doc.devices.some((d) => d.id === probeSource)
      ? probeSource
      : undefined) ??
    doc.devices.find((d) => d.kind === "host")?.id ??
    doc.devices[0]?.id;

  const layer = new Map<string, number>();
  if (anchor) {
    const queue: string[] = [anchor];
    layer.set(anchor, 0);
    while (queue.length > 0) {
      const curr = queue.shift()!;
      const currLayer = layer.get(curr) ?? 0;
      const neighbors = [...(adj.get(curr) ?? [])].sort();
      for (const next of neighbors) {
        if (!layer.has(next)) {
          layer.set(next, currLayer + 1);
          queue.push(next);
        }
      }
    }
  }

  // Place disconnected devices in subsequent columns
  let maxLayer = 0;
  for (const v of layer.values()) maxLayer = Math.max(maxLayer, v);
  for (const d of doc.devices) {
    if (!layer.has(d.id)) {
      maxLayer += 1;
      layer.set(d.id, maxLayer);
    }
  }

  // Group by layer and spread vertically
  const columns = new Map<number, string[]>();
  for (const d of doc.devices) {
    const col = layer.get(d.id) ?? 0;
    const list = columns.get(col) ?? [];
    list.push(d.id);
    columns.set(col, list);
  }

  const colSpacing = 250;
  const rowSpacing = 215;
  const originX = 110;
  const centerY = 285;

  const sortedCols = [...columns.keys()].sort((a, b) => a - b);
  for (const col of sortedCols) {
    const ids = columns.get(col)!;
    const totalH = (ids.length - 1) * rowSpacing;
    ids.forEach((id, idx) => {
      if (!result[id]) {
        result[id] = {
          x: originX + col * colSpacing,
          y: Math.round(centerY - totalH / 2 + idx * rowSpacing),
        };
      }
    });
  }

  return result;
}
