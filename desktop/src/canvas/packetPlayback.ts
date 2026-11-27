import type { NativeLabDocument } from "../model/document";
import type {
  EvidenceFact,
  EvidenceItem,
  FrameKind,
  ProbeRow,
  TraceEntry,
} from "../protocol/types";
import type { ProbeDiagnosis } from "../protocol/diagnosis";
import { buildPortOwnerMap } from "./layout";

export interface PacketHop {
  linkId: string;
  fromPortId: string;
  toPortId: string;
  fromDeviceId: string;
  toDeviceId: string;
  frameKind: FrameKind;
  frameId: string;
  atNs: string;
}

export interface PacketPlaybackPlan {
  probeId: string;
  hops: PacketHop[];
  /** Index of the hop associated with the located fault (if any) */
  faultHopIndex: number | null;
  /** Traversed link IDs derived strictly from backend trace or ingress evidence */
  traversedLinkIds: Set<string>;
  outcomeKind: "reachable" | "fault" | "unlocated" | "undetermined" | "incomplete";
  faultCallout: string | null;
  faultPortId?: string;
  faultDeviceId?: string;
  faultLinkId?: string;
  endDeviceId?: string;
  reachableNs?: string;
}

export function shortFrameLabel(kind: FrameKind): string {
  switch (kind) {
    case "arp_request":
      return "ARP?";
    case "arp_reply":
      return "ARP!";
    case "echo_request":
      return "PING";
    case "echo_reply":
      return "PONG";
  }
}

export function shortFaultCallout(diagnosis: ProbeDiagnosis | null): string | null {
  if (!diagnosis || diagnosis.severity !== "fault") return null;
  const ev = diagnosis.causeEvidence;
  if (!ev) return "已拦截";
  const fact = ev.fact;
  switch (fact.kind) {
    case "discarded":
      switch (fact.reason.rule) {
        case "trunk_vlan_not_allowed":
          return `VLAN ${fact.reason.vlan} 未放行!`;
        case "trunk_requires_tag":
          return "Trunk 拒收无标签帧!";
        case "untagged_port_rejects_tag":
          return `Access口拒收 VLAN ${fact.reason.vlan}!`;
        case "access_vlan_mismatch":
          return `VLAN ${fact.reason.vlan}≠Access ${fact.reason.access_vlan}!`;
      }
      break;
    case "ipv4_acl_decision":
      return fact.matched_rule
        ? `ACL #${fact.matched_rule.rule_number} 拦截!`
        : "ACL 隐式拒绝!";
    case "ipv4_mtu_exceeded":
      return `超出口 MTU (${fact.mtu_bytes}B)!`;
    case "ipv4_route_miss":
      return "无匹配静态路由!";
    case "dynamic_route_miss":
      return fact.reason === "awaiting_initial_convergence"
        ? "动态路由尚未收敛!"
        : fact.reason === "no_advertised_prefix"
          ? "未发布目标网段!"
          : "动态路由不可达!";
    case "ipv4_ttl_expired":
      return "TTL 归零丢弃!";
    case "ipv4_neighbor_not_unicast":
      return "下一跳地址无效!";
    case "transmit_queue_full":
      return `队列满尾丢 (Q=${fact.queue_frames})!`;
    case "link_down":
      return `链路 ${fact.link_id} 已断开!`;
    case "in_flight_link_loss":
      return `在途遇 ${fact.link_id} 断开!`;
    case "in_flight_protection_loss":
      return "主备切换在途丢包!";
    case "stp_awaiting_convergence":
      return "STP 正在收敛阻塞!";
    case "in_flight_stp_loss":
      return "STP 重收敛在途丢包!";
    case "protection_unavailable":
      return "主备链路均不可用!";
    default:
      return diagnosis.badgeText;
  }
  return diagnosis.badgeText;
}

/**
 * Returns true strictly for evidence facts that prove a frame arrived INGRESS
 * via the port's attached cable. Never true for egress rejections/drops!
 */
function isIngressArrivalFact(fact: EvidenceFact): boolean {
  switch (fact.kind) {
    case "discarded":
      return fact.stage === "ingress";
    case "ipv4_acl_decision":
      return fact.direction === "ingress";
    case "arp_resolved":
    case "echo_request_received":
    case "echo_reply_received":
    case "ipv4_route_miss":
    case "dynamic_route_miss":
    case "ipv4_ttl_expired":
      return true;
    default:
      return false;
  }
}

/**
 * Constructs a strictly evidence-backed packet playback sequence from backend trace
 * and ingress-only evidence records. Never invents backwards or un-traversed hops.
 */
export function buildPacketPlaybackPlan(
  doc: NativeLabDocument,
  probe: ProbeRow | null,
  diagnosis: ProbeDiagnosis | null,
  traceEntries: TraceEntry[],
  evidence: EvidenceItem[]
): PacketPlaybackPlan | null {
  if (!probe || !diagnosis) return null;

  const portOwner = buildPortOwnerMap(doc);
  const linkByPort = new Map<string, { linkId: string; peerPortId: string }>();
  for (const link of doc.links) {
    linkByPort.set(link.ports[0], { linkId: link.id, peerPortId: link.ports[1] });
    linkByPort.set(link.ports[1], { linkId: link.id, peerPortId: link.ports[0] });
  }

  const isSingleProbeDoc = doc.format_version === 1;
  const matchesProbe = (entryProbeId: string | null) =>
    entryProbeId === probe.id || (isSingleProbeDoc && entryProbeId === null);

  const probeTrace = traceEntries.filter((t) => matchesProbe(t.probe_id));
  let hops: PacketHop[] = [];
  const traversedLinkIds = new Set<string>();

  for (const entry of probeTrace) {
    if (entry.event !== "frame_arrival" || !entry.port || !entry.frame) continue;
    const toPortId = entry.port.port_id;
    const linkInfo = linkByPort.get(toPortId);
    if (!linkInfo) continue;
    const fromOwner = portOwner.get(linkInfo.peerPortId);
    const toOwner = portOwner.get(toPortId);
    if (!fromOwner || !toOwner) continue;

    hops.push({
      linkId: linkInfo.linkId,
      fromPortId: linkInfo.peerPortId,
      toPortId,
      fromDeviceId: fromOwner.device.id,
      toDeviceId: toOwner.device.id,
      frameKind: entry.frame.kind,
      frameId: entry.frame.id,
      atNs: entry.at_ns,
    });
    traversedLinkIds.add(linkInfo.linkId);
  }

  // Fallback when trace is disabled/empty: ONLY synthesize ingress-arrival fault hop
  // (never fabricate backwards hops on egress drops or partial-stub green washes on reachable runs!)
  if (hops.length === 0 && diagnosis.severity === "fault" && diagnosis.causeEvidence) {
    const ev = diagnosis.causeEvidence;
    if (isIngressArrivalFact(ev.fact) && ev.port) {
      const linkInfo = linkByPort.get(ev.port.port_id);
      if (linkInfo) {
        const fromOwner = portOwner.get(linkInfo.peerPortId);
        const toOwner = portOwner.get(ev.port.port_id);
        if (fromOwner && toOwner) {
          hops.push({
            linkId: linkInfo.linkId,
            fromPortId: linkInfo.peerPortId,
            toPortId: ev.port.port_id,
            fromDeviceId: fromOwner.device.id,
            toDeviceId: toOwner.device.id,
            frameKind: ev.frame.kind,
            frameId: ev.frame.id,
            atNs: ev.at_ns,
          });
          traversedLinkIds.add(linkInfo.linkId);
        }
      }
    }
  } else if (hops.length === 0 && evidence.length > 0 && diagnosis.severity === "fault") {
    const probeEvs = evidence.filter((e) => matchesProbe(e.probe_id));
    for (const ev of probeEvs) {
      if (ev.port && isIngressArrivalFact(ev.fact)) {
        const linkInfo = linkByPort.get(ev.port.port_id);
        if (linkInfo) traversedLinkIds.add(linkInfo.linkId);
      }
    }
  }

  // Keep the LAST 18 hops when truncating long flood traces so causal fault / reply hops are preserved!
  if (hops.length > 18) {
    hops = hops.slice(-18);
  }

  const outcomeKind: PacketPlaybackPlan["outcomeKind"] =
    diagnosis.severity === "ok"
      ? "reachable"
      : diagnosis.severity === "fault"
        ? "fault"
        : diagnosis.severity === "undetermined"
          ? "undetermined"
          : diagnosis.severity === "incomplete"
            ? "incomplete"
            : "unlocated";

  // Locate exact hop corresponding to causeEvidence so the landed packet never parks on an unrelated link
  let faultHopIndex: number | null = null;
  if (outcomeKind === "fault" && hops.length > 0) {
    const causeEv = diagnosis.causeEvidence;
    if (causeEv) {
      // 1. Exact frame id + port match
      let idx = hops.findIndex(
        (h) => h.frameId === causeEv.frame.id && h.toPortId === causeEv.port.port_id
      );
      // 2. Exact fault port match (latest matching arrival)
      if (idx < 0 && diagnosis.target?.portId) {
        for (let i = hops.length - 1; i >= 0; i--) {
          if (hops[i].toPortId === diagnosis.target.portId) {
            idx = i;
            break;
          }
        }
      }
      // 3. Matching fault device arrival (latest)
      if (idx < 0 && diagnosis.target?.deviceId) {
        for (let i = hops.length - 1; i >= 0; i--) {
          if (hops[i].toDeviceId === diagnosis.target.deviceId) {
            idx = i;
            break;
          }
        }
      }
      faultHopIndex = idx >= 0 ? idx : null;
    }
  }

  return {
    probeId: probe.id,
    hops,
    faultHopIndex,
    traversedLinkIds,
    outcomeKind,
    faultCallout: shortFaultCallout(diagnosis),
    faultPortId: diagnosis.target?.portId,
    faultDeviceId: diagnosis.target?.deviceId,
    faultLinkId: diagnosis.target?.linkId,
    endDeviceId:
      outcomeKind === "reachable"
        ? probe.source.device_id
        : diagnosis.target?.deviceId ?? probe.destination.device_id,
    reachableNs:
      probe.outcome.status === "reachable" ? probe.outcome.at_ns : undefined,
  };
}
