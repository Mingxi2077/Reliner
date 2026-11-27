// Native Reliner JSON experiment document model (format_version 1, 2, 3)
// Strictly respects serde(deny_unknown_fields) and deserialize_with="provided" (omit unset keys, never emit null).

export type SwitchPortMode =
  | { kind: "access"; vlan: number }
  | { kind: "trunk"; allowed_vlans: number[]; native_vlan?: number };

export interface HostPortSpec {
  id: string;
  name: string;
  ipv4_mtu_bytes?: number;
}

export interface SwitchPortSpec {
  id: string;
  name: string;
  mode: SwitchPortMode;
  stp_cost?: number;
}

export interface AclRuleSpec {
  action: "permit" | "deny";
  network: string;
  prefix_length: number;
}

export interface Ipv4AclSpec {
  ingress?: AclRuleSpec[];
  egress?: AclRuleSpec[];
}

export interface IpPortSpec {
  id: string;
  name: string;
  ipv4: string;
  prefix_length: number;
  mac: string;
  ipv4_mtu_bytes?: number;
  ipv4_acl?: Ipv4AclSpec;
}

export interface RouteSpec {
  network: string;
  prefix_length: number;
  egress: string;
  next_hop: string;
}

export interface LinkStatePeerSpec {
  port: string;
  cost: number;
}

export interface LinkStateSpec {
  peers: LinkStatePeerSpec[];
  advertise: string[];
}

export interface HostDeviceSpec {
  kind: "host";
  id: string;
  name: string;
  ipv4: string;
  prefix_length: number;
  mac: string;
  port: HostPortSpec;
  gateway?: string;
}

export interface SwitchDeviceSpec {
  kind: "switch";
  id: string;
  name: string;
  stp_priority?: number;
  ports: SwitchPortSpec[];
}

export interface RouterDeviceSpec {
  kind: "router";
  id: string;
  name: string;
  ports: IpPortSpec[];
  routes: RouteSpec[];
  link_state?: LinkStateSpec;
}

export type DeviceSpec = HostDeviceSpec | SwitchDeviceSpec | RouterDeviceSpec;

export interface TransmissionSpec {
  rate_bps: string;
  queue_frames: number;
}

export interface LinkSpec {
  id: string;
  ports: [string, string];
  delay_ns: string;
  standby_for?: string;
  transmission?: TransmissionSpec;
}

export interface LegacyProbeSpec {
  source: string;
  destination: string;
  deadline_ns: string;
}

export interface LinkActionSpec {
  link: string;
  state: "up" | "down";
  at_ns: string;
}

export interface WarmupSpec {
  port: string;
  neighbor: string;
  at_ns: string;
}

export interface TimedProbeSpec {
  id: string;
  source: string;
  destination: string;
  at_ns: string;
  deadline_ns: string;
  ttl?: number;
  payload_bytes?: number;
}

export interface ScenarioSpec {
  end_ns: string;
  arp_lifetime_ns: string;
  mac_lifetime_ns: string;
  trace_limit?: number;
  routing_convergence_ns?: string;
  stp_convergence_ns?: string;
  link_actions?: LinkActionSpec[];
  warmup?: WarmupSpec[];
  probes: TimedProbeSpec[];
}

export interface NativeLabDocument {
  format_version: 1 | 2 | 3;
  id: string;
  revision: string;
  devices: DeviceSpec[];
  links: LinkSpec[];
  probe?: LegacyProbeSpec;
  scenario?: ScenarioSpec;
  import_source?: Record<string, unknown>;
}

export interface NodePosition {
  x: number;
  y: number;
}

export interface ViewportState {
  x: number;
  y: number;
  zoom: number;
}

export interface PresentationData {
  version: number;
  nodes: Record<string, NodePosition>;
  viewport?: ViewportState;
  [key: string]: unknown;
}

export interface FlatPortInfo {
  portId: string;
  portName: string;
  deviceId: string;
  deviceName: string;
  deviceKind: DeviceSpec["kind"];
  connectedLinkId: string | null;
  detail: string;
}

export function parseExperimentJson(raw: string): NativeLabDocument {
  const cleaned = raw.replace(/^\uFEFF/, "");
  const parsed = JSON.parse(cleaned) as Partial<NativeLabDocument> & {
    presentation?: unknown;
  };
  if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) {
    throw new Error("实验文档格式无效：顶层必须是 JSON 对象。");
  }
  if (
    parsed.format_version !== 1 &&
    parsed.format_version !== 2 &&
    parsed.format_version !== 3
  ) {
    throw new Error(
      `不支持的实验文档格式版本：${String(parsed.format_version)}（仅支持 1 / 2 / 3）。`
    );
  }
  if (typeof parsed.id !== "string" || parsed.id.trim() === "") {
    throw new Error("实验文档格式无效：缺少有效的字符串拓扑 ID（id）。");
  }
  if (typeof parsed.revision !== "string") {
    throw new Error("实验文档格式无效：缺少字符串修订号（revision）。");
  }
  if (!Array.isArray(parsed.devices) || !Array.isArray(parsed.links)) {
    throw new Error("实验文档格式无效：缺少 devices 或 links 数组。");
  }
  delete parsed.presentation;
  return parsed as NativeLabDocument;
}

function finiteNumber(value: unknown): number | undefined {
  return typeof value === "number" && Number.isFinite(value) ? value : undefined;
}

function nonEmptyString(value: unknown): string | undefined {
  return typeof value === "string" && value.trim() !== "" ? value.trim() : undefined;
}

function stripUndefined<T>(value: T): T {
  if (Array.isArray(value)) {
    return value.map((item) => stripUndefined(item)) as unknown as T;
  }
  if (value !== null && typeof value === "object") {
    const out: Record<string, unknown> = {};
    for (const [k, v] of Object.entries(value as Record<string, unknown>)) {
      if (v !== undefined) {
        out[k] = stripUndefined(v);
      }
    }
    return out as T;
  }
  return value;
}

export function serializeExperimentJson(doc: NativeLabDocument): string {
  const copy = stripUndefined({ ...doc });
  delete (copy as Record<string, unknown>).presentation;
  if (copy.format_version === 1) {
    delete copy.scenario;
    delete copy.import_source;
  } else {
    delete copy.probe;
    if (copy.format_version === 2) {
      delete copy.import_source;
    }
  }
  // Ensure `provided`-annotated & optional numeric fields are never null, NaN, or ""
  copy.devices = copy.devices.map((device) => {
    if (device.kind === "host") {
      const port: HostPortSpec = { id: device.port.id, name: device.port.name };
      const mtu = finiteNumber(device.port.ipv4_mtu_bytes);
      if (mtu !== undefined) {
        port.ipv4_mtu_bytes = mtu;
      }
      const next: HostDeviceSpec = {
        kind: "host",
        id: device.id,
        name: device.name,
        ipv4: device.ipv4,
        prefix_length: finiteNumber(device.prefix_length) ?? 0,
        mac: device.mac,
        port,
      };
      const gw = nonEmptyString(device.gateway);
      if (gw !== undefined) {
        next.gateway = gw;
      }
      return next;
    }
    if (device.kind === "switch") {
      const next: SwitchDeviceSpec = {
        kind: "switch",
        id: device.id,
        name: device.name,
        ports: device.ports.map((p) => {
          const nativeVlan =
            p.mode.kind === "trunk" ? finiteNumber(p.mode.native_vlan) : undefined;
          const mode: SwitchPortMode =
            p.mode.kind === "access"
              ? { kind: "access", vlan: finiteNumber(p.mode.vlan) ?? 1 }
              : {
                  kind: "trunk",
                  allowed_vlans: p.mode.allowed_vlans.filter(
                    (v) => typeof v === "number" && Number.isFinite(v)
                  ),
                  ...(nativeVlan !== undefined ? { native_vlan: nativeVlan } : {}),
                };
          const stpCost = finiteNumber(p.stp_cost);
          return {
            id: p.id,
            name: p.name,
            mode,
            ...(stpCost !== undefined ? { stp_cost: stpCost } : {}),
          };
        }),
      };
      const stpPrio = finiteNumber(device.stp_priority);
      if (stpPrio !== undefined) {
        next.stp_priority = stpPrio;
      }
      return next;
    }
    const next: RouterDeviceSpec = {
      kind: "router",
      id: device.id,
      name: device.name,
      ports: device.ports.map((p) => {
        const portOut: IpPortSpec = {
          id: p.id,
          name: p.name,
          ipv4: p.ipv4,
          prefix_length: finiteNumber(p.prefix_length) ?? 0,
          mac: p.mac,
        };
        const mtu = finiteNumber(p.ipv4_mtu_bytes);
        if (mtu !== undefined) {
          portOut.ipv4_mtu_bytes = mtu;
        }
        if (
          p.ipv4_acl &&
          (p.ipv4_acl.ingress !== undefined || p.ipv4_acl.egress !== undefined)
        ) {
          portOut.ipv4_acl = {};
          if (p.ipv4_acl.ingress !== undefined) {
            portOut.ipv4_acl.ingress = p.ipv4_acl.ingress.map((r) => ({
              ...r,
              prefix_length: finiteNumber(r.prefix_length) ?? 0,
            }));
          }
          if (p.ipv4_acl.egress !== undefined) {
            portOut.ipv4_acl.egress = p.ipv4_acl.egress.map((r) => ({
              ...r,
              prefix_length: finiteNumber(r.prefix_length) ?? 0,
            }));
          }
        }
        return portOut;
      }),
      routes: device.routes.map((r) => ({
        ...r,
        prefix_length: finiteNumber(r.prefix_length) ?? 0,
      })),
    };
    if (device.link_state) {
      next.link_state = {
        peers: device.link_state.peers.map((peer) => ({
          port: peer.port,
          cost: finiteNumber(peer.cost) ?? 1,
        })),
        advertise: [...device.link_state.advertise],
      };
    }
    return next;
  });

  copy.links = copy.links.map((link) => {
    const out: LinkSpec = {
      id: link.id,
      ports: link.ports,
      delay_ns: link.delay_ns,
    };
    const standby = nonEmptyString(link.standby_for);
    if (standby !== undefined) {
      out.standby_for = standby;
    }
    if (link.transmission) {
      out.transmission = {
        rate_bps: link.transmission.rate_bps,
        queue_frames: Math.max(0, Math.floor(finiteNumber(link.transmission.queue_frames) ?? 0)),
      };
    }
    return out;
  });

  if (copy.scenario) {
    const s = copy.scenario;
    const cleanScenario: ScenarioSpec = {
      end_ns: s.end_ns,
      arp_lifetime_ns: s.arp_lifetime_ns,
      mac_lifetime_ns: s.mac_lifetime_ns,
      probes: (s.probes ?? []).map((p) => {
        const probeOut: TimedProbeSpec = {
          id: p.id,
          source: p.source,
          destination: p.destination,
          at_ns: p.at_ns,
          deadline_ns: p.deadline_ns,
        };
        const ttl = finiteNumber(p.ttl);
        if (ttl !== undefined) probeOut.ttl = ttl;
        const payload = finiteNumber(p.payload_bytes);
        if (payload !== undefined) probeOut.payload_bytes = payload;
        return probeOut;
      }),
    };
    const traceLimit = finiteNumber(s.trace_limit);
    if (traceLimit !== undefined) cleanScenario.trace_limit = traceLimit;
    const routingConv = nonEmptyString(s.routing_convergence_ns);
    if (routingConv !== undefined) cleanScenario.routing_convergence_ns = routingConv;
    const stpConv = nonEmptyString(s.stp_convergence_ns);
    if (stpConv !== undefined) cleanScenario.stp_convergence_ns = stpConv;
    if (Array.isArray(s.link_actions)) cleanScenario.link_actions = s.link_actions;
    if (Array.isArray(s.warmup)) cleanScenario.warmup = s.warmup;
    copy.scenario = cleanScenario;
  }

  return JSON.stringify(copy, null, 2);
}

export function parsePresentationJson(raw: string | null): PresentationData {
  if (!raw) {
    return { version: 1, nodes: {} };
  }
  try {
    const parsed = JSON.parse(raw) as Record<string, unknown>;
    if (parsed && typeof parsed === "object" && !Array.isArray(parsed)) {
      const rawNodes = parsed.nodes;
      const nodes: Record<string, NodePosition> = {};
      if (rawNodes && typeof rawNodes === "object" && !Array.isArray(rawNodes)) {
        for (const [id, pos] of Object.entries(rawNodes as Record<string, unknown>)) {
          if (
            pos &&
            typeof pos === "object" &&
            typeof (pos as NodePosition).x === "number" &&
            typeof (pos as NodePosition).y === "number"
          ) {
            nodes[id] = { x: (pos as NodePosition).x, y: (pos as NodePosition).y };
          }
        }
      }
      const viewport =
        parsed.viewport &&
        typeof parsed.viewport === "object" &&
        typeof (parsed.viewport as ViewportState).x === "number" &&
        typeof (parsed.viewport as ViewportState).y === "number" &&
        typeof (parsed.viewport as ViewportState).zoom === "number"
          ? (parsed.viewport as ViewportState)
          : undefined;
      return {
        ...parsed,
        version: typeof parsed.version === "number" ? parsed.version : 1,
        nodes,
        ...(viewport ? { viewport } : {}),
      };
    }
  } catch {
    // Fallback to clean empty presentation
  }
  return { version: 1, nodes: {} };
}

export function listAllPorts(doc: NativeLabDocument): FlatPortInfo[] {
  const linkByPort = new Map<string, string>();
  for (const link of doc.links) {
    linkByPort.set(link.ports[0], link.id);
    linkByPort.set(link.ports[1], link.id);
  }
  const ports: FlatPortInfo[] = [];
  for (const device of doc.devices) {
    if (device.kind === "host") {
      ports.push({
        portId: device.port.id,
        portName: device.port.name,
        deviceId: device.id,
        deviceName: device.name,
        deviceKind: "host",
        connectedLinkId: linkByPort.get(device.port.id) ?? null,
        detail: `${device.ipv4}/${device.prefix_length}`,
      });
    } else if (device.kind === "switch") {
      for (const p of device.ports) {
        const detail =
          p.mode.kind === "access"
            ? `Access VLAN ${p.mode.vlan}`
            : `Trunk [${p.mode.allowed_vlans.join(",")}]${
                p.mode.native_vlan !== undefined ? ` Native ${p.mode.native_vlan}` : ""
              }`;
        ports.push({
          portId: p.id,
          portName: p.name,
          deviceId: device.id,
          deviceName: device.name,
          deviceKind: "switch",
          connectedLinkId: linkByPort.get(p.id) ?? null,
          detail,
        });
      }
    } else {
      for (const p of device.ports) {
        ports.push({
          portId: p.id,
          portName: p.name,
          deviceId: device.id,
          deviceName: device.name,
          deviceKind: "router",
          connectedLinkId: linkByPort.get(p.id) ?? null,
          detail: `${p.ipv4}/${p.prefix_length}`,
        });
      }
    }
  }
  return ports;
}

export function findPortOwner(
  doc: NativeLabDocument,
  portId: string
): { device: DeviceSpec; portName: string } | null {
  for (const device of doc.devices) {
    if (device.kind === "host" && device.port.id === portId) {
      return { device, portName: device.port.name };
    }
    if (device.kind === "switch") {
      const p = device.ports.find((item) => item.id === portId);
      if (p) return { device, portName: p.name };
    }
    if (device.kind === "router") {
      const p = device.ports.find((item) => item.id === portId);
      if (p) return { device, portName: p.name };
    }
  }
  return null;
}

function collectUsedMacs(doc: NativeLabDocument): Set<string> {
  const used = new Set<string>();
  for (const d of doc.devices) {
    if (d.kind === "host") used.add(d.mac.toLowerCase());
    if (d.kind === "router") {
      for (const p of d.ports) used.add(p.mac.toLowerCase());
    }
  }
  return used;
}

export function nextUniqueMac(doc: NativeLabDocument): string {
  const used = collectUsedMacs(doc);
  for (let i = 1; i < 65000; i++) {
    const hi = ((i >> 8) & 0xff).toString(16).padStart(2, "0");
    const lo = (i & 0xff).toString(16).padStart(2, "0");
    const mac = `02:00:00:00:${hi}:${lo}`;
    if (!used.has(mac)) return mac;
  }
  return "02:00:00:ff:ff:fe";
}

function collectUsedIps(doc: NativeLabDocument): Set<string> {
  const used = new Set<string>();
  for (const d of doc.devices) {
    if (d.kind === "host") used.add(d.ipv4);
    if (d.kind === "router") {
      for (const p of d.ports) used.add(p.ipv4);
    }
  }
  return used;
}

function nextUniqueIp(doc: NativeLabDocument, thirdOctet = 2, startHost = 10): string {
  const used = collectUsedIps(doc);
  for (let h = startHost; h < 250; h++) {
    const ip = `192.0.${thirdOctet}.${h}`;
    if (!used.has(ip)) return ip;
  }
  for (let o = 10; o < 250; o++) {
    for (let h = 1; h < 250; h++) {
      const ip = `10.0.${o}.${h}`;
      if (!used.has(ip)) return ip;
    }
  }
  return "10.250.0.1";
}

export function nextUniqueId(prefix: string, used: Set<string>): string {
  for (let i = 1; i < 9999; i++) {
    const candidate = `${prefix}-${i}`;
    if (!used.has(candidate)) return candidate;
  }
  return `${prefix}-${Date.now()}`;
}

function nextUniqueName(
  prefix: string,
  usedLower: Set<string>,
  startIndex = 1
): string {
  for (let i = startIndex; i < 9999; i++) {
    const candidate = `${prefix}${i}`;
    if (!usedLower.has(candidate.toLowerCase())) return candidate;
  }
  return `${prefix}${Date.now()}`;
}

function extractIpv4ThirdOctet(ipv4: string): number | null {
  const parts = ipv4.trim().split(".");
  if (parts.length !== 4) return null;
  const third = Number(parts[2]);
  return Number.isInteger(third) && third >= 0 && third <= 255 ? third : null;
}

export function createDevice(
  doc: NativeLabDocument,
  kind: DeviceSpec["kind"]
): DeviceSpec {
  const usedDevIds = new Set(doc.devices.map((d) => d.id));
  const usedNames = new Set(doc.devices.map((d) => d.name.trim().toLowerCase()));
  const usedPortIds = new Set(listAllPorts(doc).map((p) => p.portId));

  if (kind === "host") {
    const id = nextUniqueId("pc", usedDevIds);
    const num = Number(id.replace(/^pc-/, "")) || 1;
    const name = nextUniqueName("PC-", usedNames, num);
    const portId = nextUniqueId(`${id}-eth`, usedPortIds);
    return {
      kind: "host",
      id,
      name,
      ipv4: nextUniqueIp(doc, 2, 10),
      prefix_length: 24,
      mac: nextUniqueMac(doc),
      port: { id: portId, name: "eth0" },
    };
  }

  if (kind === "switch") {
    const id = nextUniqueId("sw", usedDevIds);
    const num = Number(id.replace(/^sw-/, "")) || 1;
    const name = nextUniqueName("SW", usedNames, num);
    const ports: SwitchPortSpec[] = [1, 2, 3, 4].map((idx) => {
      const pid = nextUniqueId(`${id}-g0-${idx}`, usedPortIds);
      usedPortIds.add(pid);
      return {
        id: pid,
        name: `G0/${idx}`,
        mode: { kind: "access", vlan: 10 },
      };
    });
    return {
      kind: "switch",
      id,
      name,
      ports,
    };
  }

  const id = nextUniqueId("r", usedDevIds);
  const num = Number(id.replace(/^r-/, "")) || 1;
  const name = nextUniqueName("R", usedNames, num);
  const fakeDoc: NativeLabDocument = { ...doc, devices: [...doc.devices] };
  const ports: IpPortSpec[] = [0, 1].map((idx) => {
    const pid = nextUniqueId(`${id}-g0-${idx}`, usedPortIds);
    usedPortIds.add(pid);
    const ip = nextUniqueIp(fakeDoc, 2 + idx, 1);
    const mac = nextUniqueMac(fakeDoc);
    const port: IpPortSpec = {
      id: pid,
      name: `G0/${idx}`,
      ipv4: ip,
      prefix_length: 24,
      mac,
    };
    fakeDoc.devices.push({
      kind: "router",
      id: `__tmp_${idx}`,
      name: "",
      ports: [port],
      routes: [],
    });
    return port;
  });
  return {
    kind: "router",
    id,
    name,
    ports,
    routes: [],
  };
}

export function addPortToDevice(
  doc: NativeLabDocument,
  deviceId: string
): { doc: NativeLabDocument; portId: string } | null {
  const usedPortIds = new Set(listAllPorts(doc).map((p) => p.portId));
  let createdPortId: string | null = null;
  const devices = doc.devices.map((device) => {
    if (device.id !== deviceId) return device;
    if (device.kind === "switch") {
      const idx = device.ports.length + 1;
      const portId = nextUniqueId(`${device.id}-p${idx}`, usedPortIds);
      createdPortId = portId;
      return {
        ...device,
        ports: [
          ...device.ports,
          {
            id: portId,
            name: `G0/${idx}`,
            mode: { kind: "access" as const, vlan: 10 },
          },
        ],
      };
    }
    if (device.kind === "router") {
      const idx = device.ports.length;
      const portId = nextUniqueId(`${device.id}-p${idx}`, usedPortIds);
      createdPortId = portId;
      // Avoid reusing any /24 subnet prefix already assigned to another port on this router
      const routerThirdOctets = new Set<number>();
      for (const existingPort of device.ports) {
        const oct = extractIpv4ThirdOctet(existingPort.ipv4);
        if (oct !== null) routerThirdOctets.add(oct);
      }
      let chosenOctet = 10 + idx;
      for (let cand = 10; cand < 250; cand++) {
        if (!routerThirdOctets.has(cand)) {
          chosenOctet = cand;
          break;
        }
      }
      return {
        ...device,
        ports: [
          ...device.ports,
          {
            id: portId,
            name: `G0/${idx}`,
            ipv4: nextUniqueIp(doc, chosenOctet, 1),
            prefix_length: 24,
            mac: nextUniqueMac(doc),
          },
        ],
      };
    }
    return device;
  });
  if (!createdPortId) return null;
  return { doc: { ...doc, devices }, portId: createdPortId };
}

export function createLink(
  doc: NativeLabDocument,
  portA: string,
  portB: string
): LinkSpec {
  const used = new Set(doc.links.map((l) => l.id));
  const id = nextUniqueId("link", used);
  return {
    id,
    ports: [portA, portB],
    delay_ns: "100",
  };
}

function pruneImportSourceGaps(
  importSource: Record<string, unknown> | undefined,
  removedDeviceIds: Set<string>,
  removedPorts: Set<string>,
  removedLinks: Set<string>
): Record<string, unknown> | undefined {
  if (!importSource || !Array.isArray(importSource.gaps)) {
    return importSource;
  }
  const filteredGaps = importSource.gaps.filter((gap) => {
    if (!gap || typeof gap !== "object") return true;
    const rec = gap as Record<string, unknown>;
    const loc =
      rec.location && typeof rec.location === "object"
        ? (rec.location as Record<string, unknown>)
        : rec;
    const devId =
      typeof loc.device_id === "string"
        ? loc.device_id
        : typeof rec.device_id === "string"
          ? rec.device_id
          : undefined;
    const portId =
      typeof loc.port_id === "string"
        ? loc.port_id
        : typeof rec.port_id === "string"
          ? rec.port_id
          : undefined;
    const linkId =
      typeof loc.link_id === "string"
        ? loc.link_id
        : typeof rec.link_id === "string"
          ? rec.link_id
          : undefined;
    if (devId && removedDeviceIds.has(devId)) return false;
    if (portId && removedPorts.has(portId)) return false;
    if (linkId && removedLinks.has(linkId)) return false;
    return true;
  });
  return { ...importSource, gaps: filteredGaps };
}

function finalizeTopologyCleanup(
  doc: NativeLabDocument,
  devicesInput: DeviceSpec[],
  links: LinkSpec[],
  removedDeviceIds: Set<string>,
  removedPorts: Set<string>,
  removedLinks: Set<string>,
  detachedPorts: Set<string>
): NativeLabDocument {
  // Clean surviving device references to detached ports (link_state peers/advertise, switch stp_cost)
  const devices: DeviceSpec[] = devicesInput.map((device) => {
    if (device.kind === "switch") {
      const hasDetached = device.ports.some(
        (p) => detachedPorts.has(p.id) && p.stp_cost !== undefined
      );
      if (!hasDetached) return device;
      return {
        ...device,
        ports: device.ports.map((p) =>
          detachedPorts.has(p.id) && p.stp_cost !== undefined
            ? { ...p, stp_cost: undefined }
            : p
        ),
      };
    }
    if (device.kind === "router" && device.link_state) {
      const peers = device.link_state.peers.filter(
        (peer) => !detachedPorts.has(peer.port) && !removedPorts.has(peer.port)
      );
      const advertise = device.link_state.advertise.filter(
        (pid) => !detachedPorts.has(pid) && !removedPorts.has(pid)
      );
      if (peers.length === 0 && advertise.length === 0) {
        return { ...device, link_state: undefined };
      }
      if (
        peers.length !== device.link_state.peers.length ||
        advertise.length !== device.link_state.advertise.length
      ) {
        return { ...device, link_state: { peers, advertise } };
      }
    }
    return device;
  });

  const next: NativeLabDocument = { ...doc, devices, links };

  // Clean format_version 1 legacy probe if source/destination host was deleted
  if (next.format_version === 1 && next.probe && removedDeviceIds.size > 0) {
    const hosts = devices.filter((d) => d.kind === "host");
    let source = next.probe.source;
    let destination = next.probe.destination;
    if (removedDeviceIds.has(source)) {
      source = hosts.find((h) => h.id !== destination)?.id ?? "";
    }
    if (removedDeviceIds.has(destination)) {
      destination = hosts.find((h) => h.id !== source)?.id ?? "";
    }
    next.probe = { ...next.probe, source, destination };
  }

  // Clean scenario references & convergence timers that require active routers/switches
  if (next.scenario) {
    const hasLinkStateRouter = devices.some(
      (d) => d.kind === "router" && Boolean(d.link_state)
    );
    const switchPortIds = new Set<string>();
    for (const d of devices) {
      if (d.kind === "switch") {
        for (const p of d.ports) switchPortIds.add(p.id);
      }
    }
    const hasSwitchInterconnect = links.some(
      (l) => switchPortIds.has(l.ports[0]) && switchPortIds.has(l.ports[1])
    );

    next.scenario = {
      ...next.scenario,
      routing_convergence_ns: hasLinkStateRouter
        ? next.scenario.routing_convergence_ns
        : undefined,
      stp_convergence_ns: hasSwitchInterconnect
        ? next.scenario.stp_convergence_ns
        : undefined,
      link_actions: next.scenario.link_actions?.filter(
        (a) => !removedLinks.has(a.link)
      ),
      warmup: next.scenario.warmup?.filter(
        (w) => !removedPorts.has(w.port) && !detachedPorts.has(w.port)
      ),
      probes: next.scenario.probes.filter(
        (p) => !removedDeviceIds.has(p.source) && !removedDeviceIds.has(p.destination)
      ),
    };
  }

  if (next.format_version === 3 && next.import_source) {
    next.import_source = pruneImportSourceGaps(
      next.import_source,
      removedDeviceIds,
      removedPorts,
      removedLinks
    );
  }

  return next;
}

export function deleteDeviceFromDoc(
  doc: NativeLabDocument,
  deviceId: string
): NativeLabDocument {
  const target = doc.devices.find((d) => d.id === deviceId);
  if (!target) return doc;
  const removedDeviceIds = new Set<string>([deviceId]);
  const removedPorts = new Set<string>();
  if (target.kind === "host") {
    removedPorts.add(target.port.id);
  } else {
    for (const p of target.ports) removedPorts.add(p.id);
  }
  const removedLinks = new Set<string>();
  const detachedPorts = new Set<string>();
  const links = doc.links
    .filter((l) => {
      const aRemoved = removedPorts.has(l.ports[0]);
      const bRemoved = removedPorts.has(l.ports[1]);
      if (aRemoved || bRemoved) {
        removedLinks.add(l.id);
        if (!aRemoved) detachedPorts.add(l.ports[0]);
        if (!bRemoved) detachedPorts.add(l.ports[1]);
        return false;
      }
      return true;
    })
    .map((l) =>
      l.standby_for && removedLinks.has(l.standby_for)
        ? { ...l, standby_for: undefined }
        : l
    );

  const survivingDevices = doc.devices.filter((d) => d.id !== deviceId);
  return finalizeTopologyCleanup(
    doc,
    survivingDevices,
    links,
    removedDeviceIds,
    removedPorts,
    removedLinks,
    detachedPorts
  );
}

export function deleteLinkFromDoc(
  doc: NativeLabDocument,
  linkId: string
): NativeLabDocument {
  const target = doc.links.find((l) => l.id === linkId);
  if (!target) return doc;
  const removedLinks = new Set<string>([linkId]);
  const detachedPorts = new Set<string>([target.ports[0], target.ports[1]]);
  const links = doc.links
    .filter((l) => l.id !== linkId)
    .map((l) => (l.standby_for === linkId ? { ...l, standby_for: undefined } : l));

  return finalizeTopologyCleanup(
    doc,
    doc.devices,
    links,
    new Set<string>(),
    new Set<string>(),
    removedLinks,
    detachedPorts
  );
}

export function createStarterLab(labIndex = 1): NativeLabDocument {
  return {
    format_version: 2,
    id: `custom-lab-${labIndex}`,
    revision: "1",
    devices: [
      {
        kind: "host",
        id: "pc-a",
        name: "PC-A",
        ipv4: "192.0.2.10",
        prefix_length: 24,
        mac: "02:00:00:00:00:0a",
        port: { id: "pc-a-eth0", name: "eth0" },
      },
      {
        kind: "switch",
        id: "sw1",
        name: "SW1",
        ports: [
          { id: "sw1-p1", name: "G0/1", mode: { kind: "access", vlan: 10 } },
          { id: "sw1-p2", name: "G0/2", mode: { kind: "access", vlan: 10 } },
          {
            id: "sw1-p3",
            name: "G0/3",
            mode: { kind: "trunk", allowed_vlans: [10] },
          },
        ],
      },
      {
        kind: "host",
        id: "pc-b",
        name: "PC-B",
        ipv4: "192.0.2.20",
        prefix_length: 24,
        mac: "02:00:00:00:00:0b",
        port: { id: "pc-b-eth0", name: "eth0" },
      },
    ],
    links: [
      { id: "a-sw1", ports: ["pc-a-eth0", "sw1-p1"], delay_ns: "100" },
      { id: "sw1-b", ports: ["sw1-p2", "pc-b-eth0"], delay_ns: "100" },
    ],
    scenario: {
      end_ns: "5000",
      arp_lifetime_ns: "1000000000",
      mac_lifetime_ns: "1000000000",
      trace_limit: 1024,
      probes: [
        {
          id: "ping-1",
          source: "pc-a",
          destination: "pc-b",
          at_ns: "0",
          deadline_ns: "2000",
        },
      ],
    },
  };
}

export function bumpNativeRevision(revision: string): string {
  const trimmed = revision.trim();
  if (/^\d+$/.test(trimmed)) {
    return (BigInt(trimmed) + 1n).toString();
  }
  const match = /^(.*?)-(\d+)$/.exec(trimmed);
  if (match) {
    return `${match[1]}-${(BigInt(match[2]) + 1n).toString()}`;
  }
  return `${trimmed || "1"}-2`;
}

export function applyQuickProbe(
  doc: NativeLabDocument,
  sourceId: string,
  destinationId: string
): NativeLabDocument {
  if (doc.format_version === 1) {
    return {
      ...doc,
      probe: {
        source: sourceId,
        destination: destinationId,
        deadline_ns: doc.probe?.deadline_ns ?? "2000",
      },
    };
  }
  const scenario: ScenarioSpec = doc.scenario
    ? { ...doc.scenario, probes: [...doc.scenario.probes] }
    : {
        end_ns: "5000",
        arp_lifetime_ns: "1000000000",
        mac_lifetime_ns: "1000000000",
        trace_limit: 1024,
        probes: [],
      };
  const existingIdx = scenario.probes.findIndex(
    (p) => p.source === sourceId && p.destination === destinationId
  );
  if (existingIdx >= 0) {
    return doc;
  }
  const usedIds = new Set(scenario.probes.map((p) => p.id));
  const id = nextUniqueId("probe", usedIds);
  const rawEnd = (scenario.end_ns ?? "").trim();
  const currentEnd = /^\d+$/.test(rawEnd) ? BigInt(rawEnd) : 5000n;
  const deadline = currentEnd > 2000n ? currentEnd : 2000n;
  scenario.end_ns = deadline.toString();
  scenario.probes.push({
    id,
    source: sourceId,
    destination: destinationId,
    at_ns: "0",
    deadline_ns: deadline.toString(),
  });
  return { ...doc, scenario };
}
