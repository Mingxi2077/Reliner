import { describe, expect, it } from "vitest";
import { diagnoseProbe } from "../protocol/diagnosis";
import type { EvidenceItem, ProbeRow, RoutingEvent } from "../protocol/types";

const baseProbe: Omit<ProbeRow, "outcome" | "request_evidence_id"> = {
  id: "p1",
  source: { device_id: "pc-a", name: "PC-A", ipv4: "192.0.2.10" },
  destination: { device_id: "pc-b", name: "PC-B", ipv4: "203.0.113.20" },
  at_ns: "0",
  deadline_ns: "2000",
  ttl: 64,
};

describe("diagnoseProbe", () => {
  it("explains VLAN trunk drop and points to offending port config", () => {
    const ev: EvidenceItem = {
      probe_id: "p1",
      id: "1",
      at_ns: "200",
      port: {
        device_id: "sw2",
        device_name: "SW2",
        port_id: "sw2-uplink",
        port_name: "G0/1",
      },
      frame: { id: "2", parent_id: "1", kind: "arp_request" },
      fact: {
        kind: "discarded",
        stage: "ingress",
        reason: {
          rule: "trunk_vlan_not_allowed",
          vlan: 10,
          allowed_vlans: [20],
        },
      },
    };
    const probe: ProbeRow = {
      ...baseProbe,
      request_evidence_id: null,
      outcome: {
        status: "timeout",
        at_ns: "2000",
        waiting_for: "arp_reply",
        cause: "1",
      },
    };
    const diag = diagnoseProbe(probe, new Map([["1", ev]]), []);
    expect(diag.severity).toBe("fault");
    expect(diag.explanation).toContain("SW2/G0/1");
    expect(diag.explanation).toContain("VLAN 10");
    expect(diag.target?.portId).toBe("sw2-uplink");
    expect(diag.target?.direction).toBe("ingress");
  });

  it("explains return-path ACL deny alongside request delivery evidence", () => {
    const reqEv: EvidenceItem = {
      probe_id: "p1",
      id: "1",
      at_ns: "600",
      port: {
        device_id: "pc-b",
        device_name: "PC-B",
        port_id: "b-eth0",
        port_name: "eth0",
      },
      frame: { id: "3", parent_id: "2", kind: "echo_request" },
      fact: { kind: "echo_request_received" },
    };
    const aclEv: EvidenceItem = {
      probe_id: "p1",
      id: "2",
      at_ns: "700",
      port: {
        device_id: "r1",
        device_name: "R1",
        port_id: "r1-a",
        port_name: "LAN-A",
      },
      frame: { id: "4", parent_id: null, kind: "echo_reply" },
      fact: {
        kind: "ipv4_acl_decision",
        event_id: "12",
        direction: "egress",
        source: "203.0.113.20",
        destination: "192.0.2.10",
        action: "deny",
        matched_rule: {
          rule_number: 1,
          network: "203.0.113.0",
          prefix_length: 24,
        },
      },
    };
    const probe: ProbeRow = {
      ...baseProbe,
      request_evidence_id: "1",
      outcome: {
        status: "timeout",
        at_ns: "1000",
        waiting_for: "echo_reply",
        cause: "2",
      },
    };
    const diag = diagnoseProbe(
      probe,
      new Map([
        ["1", reqEv],
        ["2", aclEv],
      ]),
      []
    );
    expect(diag.severity).toBe("fault");
    expect(diag.requestNote).toContain("600 ns");
    expect(diag.explanation).toContain("第 1 条 deny 规则");
    expect(diag.target?.portId).toBe("r1-a");
  });

  it("attaches dynamic routing convergence basis when routing_event_id is present", () => {
    const ev: EvidenceItem = {
      probe_id: "p1",
      id: "5",
      at_ns: "10400",
      port: {
        device_id: "r1",
        device_name: "R1",
        port_id: "r1-r3",
        port_name: "Direct",
      },
      frame: { id: "9", parent_id: "8", kind: "echo_request" },
      fact: { kind: "link_down", link_id: "r1-r3", down_event_id: "33" },
      routing_event_id: "7",
    };
    const routes: RoutingEvent[] = [
      {
        event_id: "7",
        at_ns: "2500",
        router_id: "r1",
        triggered_by_link_event_id: null,
        installed_routes: 1,
      },
    ];
    const probe: ProbeRow = {
      ...baseProbe,
      request_evidence_id: null,
      outcome: {
        status: "timeout",
        at_ns: "14500",
        waiting_for: "echo_reply",
        cause: "5",
      },
    };
    const diag = diagnoseProbe(probe, new Map([["5", ev]]), routes);
    expect(diag.routingBasis).toContain("2500 ns");
    expect(diag.target?.linkId).toBe("r1-r3");
  });

  it("never turns undetermined outcome with modeled reachable into ok", () => {
    const probe: ProbeRow = {
      ...baseProbe,
      request_evidence_id: null,
      outcome: {
        status: "undetermined",
        modeled: { status: "reachable", at_ns: "500", evidence_id: "3" },
        coverage: { source_component: "1", destination_component: "1" },
      },
    };
    const diag = diagnoseProbe(probe, new Map(), []);
    expect(diag.severity).toBe("undetermined");
    expect(diag.badgeText).toContain("未判定");
    expect(diag.modeledNote).toContain("500 ns");
  });

  it("reports unlocated when timeout has no cause evidence and never claims return-path fault", () => {
    const reqEv: EvidenceItem = {
      probe_id: "p1",
      id: "1",
      at_ns: "50",
      port: {
        device_id: "pc-b",
        device_name: "PC-B",
        port_id: "b-eth0",
        port_name: "eth0",
      },
      frame: { id: "3", parent_id: null, kind: "echo_request" },
      fact: { kind: "echo_request_received" },
    };
    const probe: ProbeRow = {
      ...baseProbe,
      request_evidence_id: "1",
      outcome: {
        status: "timeout",
        at_ns: "100",
        waiting_for: "echo_reply",
        cause: null,
      },
    };
    const diag = diagnoseProbe(probe, new Map([["1", reqEv]]), []);
    expect(diag.severity).toBe("unlocated");
    expect(diag.badgeText).toContain("未定位");
    expect(diag.requestNote).not.toContain("故障发生在回程");
  });

  it("does not misclassify located cause > 128 as unlocated while cause evidence page is unloaded", () => {
    const probe: ProbeRow = {
      ...baseProbe,
      request_evidence_id: null,
      outcome: {
        status: "timeout",
        at_ns: "5000",
        waiting_for: "echo_reply",
        cause: "205",
      },
    };
    const diag = diagnoseProbe(probe, new Map(), []);
    expect(diag.severity).toBe("fault");
    expect(diag.badgeText).toContain("已定位");
    expect(diag.explanation).toContain("#205");
  });
});
