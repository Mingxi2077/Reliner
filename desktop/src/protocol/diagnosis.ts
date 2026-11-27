// Faithful Chinese diagnosis formatter derived strictly from backend evidence facts
// (mirrors src/main.rs semantics without inventing or guessing unverified fault causes).

import type {
  DropRule,
  DynamicRouteMissReason,
  EvidenceFact,
  EvidenceItem,
  FrameKind,
  ModeledOutcome,
  Outcome,
  PortStage,
  ProbeRow,
  ResourceLimit,
  RoutingEvent,
  StopReason,
  TraceEvent,
  WaitingFor,
} from "./types";

export interface FaultActionTarget {
  kind: "port" | "device" | "link";
  deviceId?: string;
  portId?: string;
  linkId?: string;
  label: string;
  direction?: PortStage;
}

export interface ProbeDiagnosis {
  severity: "ok" | "fault" | "undetermined" | "incomplete" | "unlocated";
  badgeText: string;
  headline: string;
  requestNote?: string;
  explanation: string;
  nextStep?: string;
  routingBasis?: string;
  protectedLinkNote?: string;
  stpEventId?: string;
  routingEventId?: string;
  target?: FaultActionTarget;
  modeledNote?: string;
  causeEvidence?: EvidenceItem;
}

export function frameKindLabel(kind: FrameKind): string {
  switch (kind) {
    case "arp_request":
      return "ARP 请求";
    case "arp_reply":
      return "ARP 应答";
    case "echo_request":
      return "ICMP Echo Request";
    case "echo_reply":
      return "ICMP Echo Reply";
  }
}

export function stageLabel(stage: PortStage): string {
  return stage === "ingress" ? "入口" : "出口";
}

export function resourceLimitLabel(limit: ResourceLimit): string {
  switch (limit) {
    case "event_work":
      return "单事件工作量";
    case "total_work":
      return "累计工作量";
    case "pending_events":
      return "待处理事件";
    case "state_entries":
      return "动态表项";
    case "essential_facts":
      return "必要证据";
  }
}

export function stopReasonText(reason: StopReason): string {
  if (reason === "time_overflow") return "派生事件的时刻超出范围";
  if (reason === "cancelled") return "运行已取消";
  return `达到${resourceLimitLabel(reason.resource_limit.limit)}上限`;
}

export function traceEventLabel(event: TraceEvent): string {
  switch (event) {
    case "link_change":
      return "链路状态变更";
    case "routing_convergence":
      return "动态路由收敛";
    case "stp_convergence":
      return "STP 收敛";
    case "table_expiry":
      return "表项老化";
    case "probe_deadline":
      return "探测截止";
    case "transmit_start":
      return "开始发送帧";
    case "transmit_cancelled":
      return "取消在途/队列帧";
    case "frame_arrival":
      return "帧到达端口";
    case "probe_start":
      return "发起探测";
    case "neighbor_warmup":
      return "邻居预热";
  }
}

export function dynamicMissReasonText(reason: DynamicRouteMissReason): string {
  switch (reason) {
    case "awaiting_initial_convergence":
      return "动态路由还在进行初次收敛";
    case "no_advertised_prefix":
      return "动态路由没有发布覆盖这个地址的网段";
    case "no_path_to_advertisement":
      return "动态路由表里没有前往该网段发布端的路径";
  }
}

export function dropRuleText(reason: DropRule): string {
  switch (reason.rule) {
    case "trunk_vlan_not_allowed":
      return `VLAN ${reason.vlan} 不在 Trunk 允许列表 [${reason.allowed_vlans.join(", ")}] 中`;
    case "trunk_requires_tag":
      return "Trunk 只接收带 VLAN 标签的帧";
    case "untagged_port_rejects_tag":
      return `端口只接收无标签帧，收到 VLAN ${reason.vlan} 标签`;
    case "access_vlan_mismatch":
      return `帧属于 VLAN ${reason.vlan}，Access 端口属于 VLAN ${reason.access_vlan}`;
  }
}

export function waitingForSummary(
  waitingFor: WaitingFor,
  atNs: string
): string {
  switch (waitingFor) {
    case "arp_reply":
      return `超时，在 ${atNs} ns 截止前未收到 ARP 应答，ICMP 请求尚未发出。`;
    case "echo_reply":
      return `超时，在 ${atNs} ns 截止前未收到 ICMP Echo Reply。`;
    case "probe_deadline":
      return `源端在发包前已拒绝该请求，探测于 ${atNs} ns 结案。`;
  }
}

export function formatEvidenceSummary(evidence: EvidenceItem): {
  text: string;
  nextStep?: string;
  target?: FaultActionTarget;
} {
  const fact: EvidenceFact = evidence.fact;
  const kind = fact.kind === "ipv4_mtu_exceeded" ? fact.packet_kind : evidence.frame.kind;
  const frame = frameKindLabel(kind);
  const loc = `${evidence.port.device_name}/${evidence.port.port_name}`;
  const at = evidence.at_ns;

  switch (fact.kind) {
    case "ipv4_acl_decision": {
      const stage = stageLabel(fact.direction);
      const verb = fact.action === "permit" ? "放行" : "拦截";
      const rule = fact.matched_rule
        ? `命中第 ${fact.matched_rule.rule_number} 条 ${fact.action} 规则，源前缀 ${fact.matched_rule.network}/${fact.matched_rule.prefix_length}`
        : "未匹配规则，执行末尾隐式拒绝";
      return {
        text: `${frame}（${fact.source} → ${fact.destination}）在 ${loc} 的${stage}被 IPv4 ACL ${verb}（${at} ns）。${rule}。`,
        nextStep:
          fact.action === "deny"
            ? `打开 ${loc} 的端口配置，检查 IPv4 ACL ${stage}规则顺序后保存并重跑。`
            : undefined,
        target: {
          kind: "port",
          deviceId: evidence.port.device_id,
          portId: evidence.port.port_id,
          label: `打开 ${loc} 端口配置`,
          direction: fact.direction,
        },
      };
    }
    case "discarded": {
      const stage = stageLabel(fact.stage);
      const reasonText = dropRuleText(fact.reason);
      let nextStep = `打开 ${loc} 的端口配置核对 VLAN 设置，修改后保存并重跑。`;
      if (fact.reason.rule === "trunk_vlan_not_allowed") {
        nextStep = `若此链路应承载 VLAN ${fact.reason.vlan}，请在 ${loc} 的 Trunk 允许列表中加入 VLAN ${fact.reason.vlan} 后保存并重跑。`;
      } else if (fact.reason.rule === "access_vlan_mismatch") {
        nextStep = `核对 ${loc} 的 Access VLAN（当前 ${fact.reason.access_vlan}）或对端 Trunk 的 Native VLAN 配置后保存并重跑。`;
      }
      return {
        text: `${frame} 在 ${loc} 的${stage}被拦截（${at} ns）。${reasonText}。`,
        nextStep,
        target: {
          kind: "port",
          deviceId: evidence.port.device_id,
          portId: evidence.port.port_id,
          label: `打开 ${loc} 端口配置`,
          direction: fact.stage,
        },
      };
    }
    case "ipv4_route_miss":
      return {
        text: `${frame} 在 ${loc} 停下（${at} ns）。IPv4 查表未找到去往 ${fact.destination} 的路由。`,
        nextStep: `打开 ${evidence.port.device_name} 配置，核对静态路由或默认网关后保存并重跑。`,
        target: {
          kind: "device",
          deviceId: evidence.port.device_id,
          portId: evidence.port.port_id,
          label: `打开 ${evidence.port.device_name} 路由配置`,
        },
      };
    case "dynamic_route_miss":
      return {
        text: `${frame} 在 ${loc} 停下（${at} ns）。IPv4 查表未找到去往 ${fact.destination} 的路由。${dynamicMissReasonText(fact.reason)}。`,
        nextStep: `打开 ${evidence.port.device_name} 或相关路由器的动态路由（link_state）与收敛时长配置，修改后保存并重跑。`,
        target: {
          kind: "device",
          deviceId: evidence.port.device_id,
          portId: evidence.port.port_id,
          label: `打开 ${evidence.port.device_name} 配置`,
        },
      };
    case "ipv4_ttl_expired":
      return {
        text: `${frame} 在 ${loc} 停下（${at} ns）。收到的 TTL 为 ${fact.received_ttl}，无法继续转发。`,
        nextStep: "核对探测的初始 TTL 设置或是否存在转发环路。",
        target: {
          kind: "device",
          deviceId: evidence.port.device_id,
          portId: evidence.port.port_id,
          label: `查看 ${evidence.port.device_name}`,
        },
      };
    case "ipv4_mtu_exceeded":
      return {
        text: `${frame} 在 ${loc} 出口丢弃（${at} ns）。IPv4 总长 ${fact.ipv4_total_bytes} 字节，超过出口 MTU ${fact.mtu_bytes} 字节。`,
        nextStep: `打开 ${loc} 的端口配置核对 IPv4 MTU（当前 ${fact.mtu_bytes} 字节），修改后用同一探测保存并重跑。`,
        target: {
          kind: "port",
          deviceId: evidence.port.device_id,
          portId: evidence.port.port_id,
          label: `打开 ${loc} MTU 配置`,
          direction: "egress",
        },
      };
    case "ipv4_neighbor_not_unicast":
      return {
        text: `${frame} 在 ${loc} 停下（${at} ns）。下一跳 ${fact.next_hop} 无法在选定接口上作单播解析。`,
        nextStep: `打开 ${evidence.port.device_name} 核对路由下一跳与接口网段配置。`,
        target: {
          kind: "device",
          deviceId: evidence.port.device_id,
          portId: evidence.port.port_id,
          label: `打开 ${evidence.port.device_name} 配置`,
        },
      };
    case "in_flight_link_loss":
      return {
        text: `${frame} 在链路 ${fact.link_id} 传输时遇到断链，无法按原计划于 ${fact.planned_arrival_ns} ns 到达 ${loc}。`,
        nextStep: `查看链路 ${fact.link_id} 与场景中的断链动作（事件 ${fact.invalidated_by}）。`,
        target: {
          kind: "link",
          linkId: fact.link_id,
          deviceId: evidence.port.device_id,
          portId: evidence.port.port_id,
          label: `查看链路 ${fact.link_id}`,
        },
      };
    case "in_flight_protection_loss":
      return {
        text: `${frame} 在传输中遇到主备切换。链路 ${fact.link_id} 于 ${fact.invalidated_at_ns} ns 退出转发，帧未按原计划于 ${fact.planned_arrival_ns} ns 到达 ${loc}。`,
        nextStep: `查看主备切换链路事件（事件 ${fact.invalidated_by}）。`,
        target: {
          kind: "link",
          linkId: fact.link_id,
          deviceId: evidence.port.device_id,
          portId: evidence.port.port_id,
          label: `查看链路 ${fact.link_id}`,
        },
      };
    case "stp_awaiting_convergence":
      return {
        text: `${frame} 在 ${loc} 未转发（${at} ns）。链路 ${fact.link_id} 物理已恢复，STP 等待本轮收敛后重新选路。`,
        nextStep: `查看 STP 收敛事件与链路 ${fact.link_id} 状态。`,
        target: {
          kind: "link",
          linkId: fact.link_id,
          deviceId: evidence.port.device_id,
          portId: evidence.port.port_id,
          label: `查看链路 ${fact.link_id}`,
        },
      };
    case "in_flight_stp_loss":
      return {
        text: `${frame} 未按计划在 ${fact.planned_arrival_ns} ns 到达 ${loc}。STP 于 ${fact.invalidated_at_ns} ns 将链路 ${fact.link_id} 移出转发树。`,
        nextStep: `查看 STP 收敛事件（事件 ${fact.invalidated_by}）及各端口角色。`,
        target: {
          kind: "link",
          linkId: fact.link_id,
          deviceId: evidence.port.device_id,
          portId: evidence.port.port_id,
          label: `查看链路 ${fact.link_id}`,
        },
      };
    case "protection_unavailable":
      return {
        text: `${frame} 在 ${evidence.port.device_name} 未转发（${at} ns）。主链路 ${fact.primary_link_id} 和备用链路 ${fact.standby_link_id} 均已断开。`,
        nextStep: `检查主链路 ${fact.primary_link_id} 和备用链路 ${fact.standby_link_id} 的物理动作时序。`,
        target: {
          kind: "link",
          linkId: fact.primary_link_id,
          deviceId: evidence.port.device_id,
          portId: evidence.port.port_id,
          label: `查看主链路 ${fact.primary_link_id}`,
        },
      };
    case "transmit_queue_full": {
      const queueDesc =
        fact.queue_frames === 0
          ? "没有等待位置"
          : `${fact.queue_frames} 个等待位置已用完`;
      return {
        text: `${frame} 在 ${loc} 出口丢弃（${at} ns）。链路 ${fact.link_id} 正在发送，${queueDesc}。本帧 ${fact.frame_bytes} 字节，链路速率 ${fact.rate_bps} bit/s。`,
        nextStep: `打开链路 ${fact.link_id} 配置，核对发送速率或等待帧数（queue_frames）后保存并重跑。`,
        target: {
          kind: "link",
          linkId: fact.link_id,
          deviceId: evidence.port.device_id,
          portId: evidence.port.port_id,
          label: `打开链路 ${fact.link_id} 队列配置`,
          direction: "egress",
        },
      };
    }
    case "link_down":
      return {
        text: `${frame} 在 ${loc} 未发出（${at} ns）。链路 ${fact.link_id} 已断开。`,
        nextStep: `查看链路 ${fact.link_id} 的断链动作（事件 ${fact.down_event_id}）或收敛时长配置。`,
        target: {
          kind: "link",
          linkId: fact.link_id,
          deviceId: evidence.port.device_id,
          portId: evidence.port.port_id,
          label: `查看链路 ${fact.link_id}`,
        },
      };
    case "arp_resolved":
      return { text: `${loc} 在 ${at} ns 完成 ARP 解析。` };
    case "echo_request_received":
      return { text: `${evidence.port.device_name} 在 ${at} ns 收到 ICMP Echo Request。` };
    case "echo_reply_received":
      return { text: `${evidence.port.device_name} 在 ${at} ns 收到 ICMP Echo Reply。` };
  }
}

function summarizeModeled(modeled: ModeledOutcome): string {
  switch (modeled.status) {
    case "reachable":
      return `已映射模型在 ${modeled.at_ns} ns 收到回包；因存在未映射配置，这不构成完整配置下的连通结论。`;
    case "timeout":
      return `已映射模型在 ${modeled.at_ns} ns 处理截止，阻断原因尚不能确认。`;
    case "incomplete":
      return `已映射模型执行未完成（${stopReasonText(modeled.reason)}），最后完整事件在 ${modeled.at_ns} ns。`;
  }
}

export function diagnoseProbe(
  probe: ProbeRow,
  evidenceById: Map<string, EvidenceItem>,
  routingEvents: RoutingEvent[]
): ProbeDiagnosis {
  const outcome: Outcome = probe.outcome;

  if (outcome.status === "undetermined") {
    return {
      severity: "undetermined",
      badgeText: "未判定：配置覆盖不足",
      headline: "通断未判定，可能受尚未映射的配置影响。",
      explanation: "存在影响当前连通组件的未映射导入缺项，前端不将模型结果判定为已确认通断。",
      modeledNote: summarizeModeled(outcome.modeled),
    };
  }

  if (outcome.status === "incomplete") {
    return {
      severity: "incomplete",
      badgeText: "未完成",
      headline: `探测未完成：${stopReasonText(outcome.reason)}。`,
      explanation: `运行在仿真时刻 ${outcome.at_ns} ns 停在完整事件边界，尚未完成该探测结案。`,
    };
  }

  if (outcome.status === "reachable") {
    const replyEv = evidenceById.get(outcome.evidence_id);
    return {
      severity: "ok",
      badgeText: "连通",
      headline: `在仿真时刻 ${outcome.at_ns} ns 收到 ICMP Echo Reply。`,
      explanation: `${probe.source.name} (${probe.source.ipv4}) → ${probe.destination.name} (${probe.destination.ipv4}) 往返成功。`,
      causeEvidence: replyEv,
    };
  }

  // Timeout / rejected before send
  const requestEv = probe.request_evidence_id
    ? evidenceById.get(probe.request_evidence_id)
    : undefined;
  const causeEv = outcome.cause ? evidenceById.get(outcome.cause) : undefined;
  const hasLocatedCause = Boolean(outcome.cause);
  const requestNote = requestEv
    ? hasLocatedCause
      ? `${probe.destination.name} 在 ${requestEv.at_ns} ns 已收到 ICMP Echo Request（故障发生在回程）。`
      : `${probe.destination.name} 在 ${requestEv.at_ns} ns 已收到 ICMP Echo Request，但在截止前未收到回包。`
    : undefined;

  if (outcome.cause && !causeEv) {
    return {
      severity: "fault",
      badgeText: outcome.waiting_for === "probe_deadline" ? "发包前拒绝 · 已定位" : "超时 · 已定位",
      headline: requestEv
        ? `超时，在 ${outcome.at_ns} ns 截止前未收到 ICMP Echo Reply。`
        : waitingForSummary(outcome.waiting_for, outcome.at_ns),
      requestNote,
      explanation: `后端已定位根因证据 #${outcome.cause}，正在按需拉取该条证据详情。`,
      nextStep: "若未自动刷新，可在下方“必要证据明细”中点击加载更多查看完整原因。",
    };
  }

  if (!causeEv) {
    return {
      severity: "unlocated",
      badgeText: outcome.waiting_for === "probe_deadline" ? "发包前拒绝 · 未定位" : "超时 · 未定位",
      headline: waitingForSummary(outcome.waiting_for, outcome.at_ns),
      requestNote,
      explanation: "未定位到有证据支持的阻断点（可能是探测截止早于往返时延或缺少可证明端口丢弃的证据）。",
      nextStep: "核对探测截止时刻（deadline_ns）与链路总延迟，或展开时间线观察已提交事件。",
    };
  }

  const formatted = formatEvidenceSummary(causeEv);
  let routingBasis: string | undefined;
  if (causeEv.routing_event_id) {
    const route = routingEvents.find((item) => item.event_id === causeEv.routing_event_id);
    if (route) {
      const routerName =
        route.router_id === causeEv.port.device_id
          ? causeEv.port.device_name
          : route.router_id;
      routingBasis = `查表依据：${routerName} 在 ${route.at_ns} ns 安装的动态路由表（事件 ${route.event_id}）。`;
    }
  }
  const protectedLinkNote = causeEv.protected_link
    ? `当时承担转发的主备链路为 ${causeEv.protected_link.link_id}${
        causeEv.protected_link.selected_by
          ? `（由切换事件 ${causeEv.protected_link.selected_by} 选中）`
          : "（初始主链路）"
      }。`
    : undefined;

  return {
    severity: "fault",
    badgeText: outcome.waiting_for === "probe_deadline" ? "发包前拒绝" : "超时 · 已定位",
    headline: requestEv
      ? `超时，在 ${outcome.at_ns} ns 截止前未收到 ICMP Echo Reply。`
      : waitingForSummary(outcome.waiting_for, outcome.at_ns),
    requestNote,
    explanation: formatted.text,
    nextStep: formatted.nextStep,
    routingBasis,
    protectedLinkNote,
    stpEventId: causeEv.stp_event_id,
    routingEventId: causeEv.routing_event_id,
    target: formatted.target,
    causeEvidence: causeEv,
  };
}
