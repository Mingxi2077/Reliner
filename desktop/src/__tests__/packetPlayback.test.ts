import { describe, expect, it } from "vitest";
import { buildPacketPlaybackPlan } from "../canvas/packetPlayback";
import { diagnoseProbe } from "../protocol/diagnosis";
import type { NativeLabDocument } from "../model/document";
import type { EvidenceItem, ProbeRow, TraceEntry } from "../protocol/types";

const twoSwitchDoc: NativeLabDocument = {
  format_version: 2,
  id: "playback-test",
  revision: "1",
  devices: [
    {
      kind: "host",
      id: "pc-a",
      name: "PC-A",
      ipv4: "192.0.2.10",
      prefix_length: 24,
      mac: "02:00:00:00:00:0a",
      port: { id: "a-eth0", name: "eth0" },
    },
    {
      kind: "switch",
      id: "sw1",
      name: "SW1",
      ports: [
        { id: "s1-a", name: "G0/1", mode: { kind: "access", vlan: 10 } },
        { id: "s1-s2", name: "G0/2", mode: { kind: "trunk", allowed_vlans: [10] } },
      ],
    },
    {
      kind: "switch",
      id: "sw2",
      name: "SW2",
      ports: [
        { id: "s2-s1", name: "G0/1", mode: { kind: "trunk", allowed_vlans: [20] } },
        { id: "s2-b", name: "G0/2", mode: { kind: "access", vlan: 10 } },
      ],
    },
    {
      kind: "host",
      id: "pc-b",
      name: "PC-B",
      ipv4: "192.0.2.20",
      prefix_length: 24,
      mac: "02:00:00:00:00:0b",
      port: { id: "b-eth0", name: "eth0" },
    },
  ],
  links: [
    { id: "l-a", ports: ["a-eth0", "s1-a"], delay_ns: "100" },
    { id: "l-mid", ports: ["s1-s2", "s2-s1"], delay_ns: "100" },
    { id: "l-b", ports: ["s2-b", "b-eth0"], delay_ns: "100" },
  ],
  scenario: {
    end_ns: "5000",
    arp_lifetime_ns: "1000000000",
    mac_lifetime_ns: "1000000000",
    probes: [],
  },
};

const baseProbe: Omit<ProbeRow, "outcome" | "request_evidence_id"> = {
  id: "p1",
  source: { device_id: "pc-a", name: "PC-A", ipv4: "192.0.2.10" },
  destination: { device_id: "pc-b", name: "PC-B", ipv4: "192.0.2.20" },
  at_ns: "0",
  deadline_ns: "2000",
  ttl: 64,
};

describe("buildPacketPlaybackPlan", () => {
  it("never fabricates a backwards hop or traversed link on egress drops when trace is empty", () => {
    const egressDropEv: EvidenceItem = {
      probe_id: "p1",
      id: "1",
      at_ns: "200",
      port: {
        device_id: "sw1",
        device_name: "SW1",
        port_id: "s1-s2",
        port_name: "G0/2",
      },
      frame: { id: "2", parent_id: "1", kind: "arp_request" },
      fact: {
        kind: "discarded",
        stage: "egress",
        reason: { rule: "trunk_vlan_not_allowed", vlan: 10, allowed_vlans: [20] },
      },
    };
    const probe: ProbeRow = {
      ...baseProbe,
      request_evidence_id: null,
      outcome: { status: "timeout", at_ns: "2000", waiting_for: "arp_reply", cause: "1" },
    };
    const diag = diagnoseProbe(probe, new Map([["1", egressDropEv]]), []);
    const plan = buildPacketPlaybackPlan(twoSwitchDoc, probe, diag, [], [egressDropEv]);
    expect(plan).not.toBeNull();
    expect(plan!.hops).toHaveLength(0);
    expect(plan!.traversedLinkIds.has("l-mid")).toBe(false);
    expect(plan!.faultHopIndex).toBeNull();
  });

  it("never paints partial-stub traversed links on reachable runs when trace is disabled", () => {
    const replyEv: EvidenceItem = {
      probe_id: "p1",
      id: "2",
      at_ns: "800",
      port: {
        device_id: "pc-a",
        device_name: "PC-A",
        port_id: "a-eth0",
        port_name: "eth0",
      },
      frame: { id: "6", parent_id: "5", kind: "echo_reply" },
      fact: { kind: "echo_reply_received" },
    };
    const probe: ProbeRow = {
      ...baseProbe,
      request_evidence_id: null,
      outcome: { status: "reachable", at_ns: "800", evidence_id: "2" },
    };
    const diag = diagnoseProbe(probe, new Map([["2", replyEv]]), []);
    const plan = buildPacketPlaybackPlan(twoSwitchDoc, probe, diag, [], [replyEv]);
    expect(plan!.outcomeKind).toBe("reachable");
    expect(plan!.traversedLinkIds.size).toBe(0);
    expect(plan!.hops).toHaveLength(0);
  });

  it("matches faultHopIndex to causeEvidence instead of parking on an unrelated later hop", () => {
    const dropEv: EvidenceItem = {
      probe_id: "p1",
      id: "1",
      at_ns: "200",
      port: {
        device_id: "sw2",
        device_name: "SW2",
        port_id: "s2-s1",
        port_name: "G0/1",
      },
      frame: { id: "10", parent_id: null, kind: "arp_request" },
      fact: {
        kind: "discarded",
        stage: "ingress",
        reason: { rule: "trunk_vlan_not_allowed", vlan: 10, allowed_vlans: [20] },
      },
    };
    const probe: ProbeRow = {
      ...baseProbe,
      request_evidence_id: null,
      outcome: { status: "timeout", at_ns: "2000", waiting_for: "arp_reply", cause: "1" },
    };
    const trace: TraceEntry[] = [
      {
        event_id: "1",
        at_ns: "100",
        event: "frame_arrival",
        probe_id: "p1",
        port: { device_id: "sw1", device_name: "SW1", port_id: "s1-a", port_name: "G0/1" },
        frame: { id: "9", parent_id: null, kind: "arp_request" },
        learning: null,
        route: null,
      },
      {
        event_id: "2",
        at_ns: "200",
        event: "frame_arrival",
        probe_id: "p1",
        port: { device_id: "sw2", device_name: "SW2", port_id: "s2-s1", port_name: "G0/1" },
        frame: { id: "10", parent_id: "9", kind: "arp_request" },
        learning: null,
        route: null,
      },
      {
        event_id: "3",
        at_ns: "250",
        event: "frame_arrival",
        probe_id: "p1",
        port: { device_id: "pc-b", device_name: "PC-B", port_id: "b-eth0", port_name: "eth0" },
        frame: { id: "11", parent_id: null, kind: "arp_request" },
        learning: null,
        route: null,
      },
    ];
    const diag = diagnoseProbe(probe, new Map([["1", dropEv]]), []);
    const plan = buildPacketPlaybackPlan(twoSwitchDoc, probe, diag, trace, [dropEv]);
    expect(plan!.hops).toHaveLength(3);
    // Exact fault hop is index 1 (frame 10 arriving at s2-s1), NOT index 2!
    expect(plan!.faultHopIndex).toBe(1);
  });
});
