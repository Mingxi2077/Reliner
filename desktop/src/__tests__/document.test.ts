import { describe, expect, it } from "vitest";
import {
  addPortToDevice,
  applyQuickProbe,
  bumpNativeRevision,
  createDevice,
  createStarterLab,
  deleteDeviceFromDoc,
  deleteLinkFromDoc,
  parseExperimentJson,
  serializeExperimentJson,
  type NativeLabDocument,
} from "../model/document";
import { ensureNodePositions } from "../canvas/layout";

describe("document model helpers", () => {
  it("preserves format_version 1 semantics and omits presentation and unset optional fields", () => {
    const raw = JSON.stringify({
      format_version: 1,
      id: "legacy-lab",
      revision: "1",
      devices: [
        {
          kind: "host",
          id: "pc-a",
          name: "PC-A",
          ipv4: "192.0.2.10",
          prefix_length: 24,
          mac: "02:00:00:00:00:0a",
          port: { id: "a0", name: "eth0" },
        },
        {
          kind: "switch",
          id: "sw1",
          name: "SW1",
          ports: [
            { id: "s1", name: "G0/1", mode: { kind: "access", vlan: 10 } },
            { id: "s2", name: "G0/2", mode: { kind: "trunk", allowed_vlans: [10] } },
          ],
        },
      ],
      links: [{ id: "l1", ports: ["a0", "s1"], delay_ns: "100" }],
      probe: { source: "pc-a", destination: "pc-a", deadline_ns: "2000" },
      presentation: { nodes: { "pc-a": { x: 10, y: 20 } } },
    });

    const parsed = parseExperimentJson(raw);
    const out = JSON.parse(serializeExperimentJson(parsed)) as Record<string, unknown>;
    expect(out.format_version).toBe(1);
    expect(out.presentation).toBeUndefined();
    expect(out.scenario).toBeUndefined();
    const sw = (out.devices as Array<Record<string, unknown>>)[1];
    expect("stp_priority" in sw).toBe(false);
    const trunk = (sw.ports as Array<Record<string, unknown>>)[1];
    expect("stp_cost" in trunk).toBe(false);
    expect("native_vlan" in (trunk.mode as Record<string, unknown>)).toBe(false);
  });

  it("generates unique devices, bumps revisions cleanly, and auto-layouts nodes", () => {
    const lab = createStarterLab(1);
    const newHost = createDevice(lab, "host");
    const newRouter = createDevice({ ...lab, devices: [...lab.devices, newHost] }, "router");
    expect(newHost.id).not.toBe("pc-a");
    expect(newRouter.kind).toBe("router");
    expect(bumpNativeRevision("1")).toBe("2");
    expect(bumpNativeRevision("rev-3")).toBe("rev-4");

    const updated = applyQuickProbe(lab, "pc-b", "pc-a");
    expect(updated.scenario?.probes.length).toBe(2);

    const positions = ensureNodePositions(lab, {});
    expect(Object.keys(positions)).toEqual(["pc-a", "sw1", "pc-b"]);
    expect(positions["pc-a"].x).toBeLessThan(positions["sw1"].x);
    expect(positions["sw1"].x).toBeLessThan(positions["pc-b"].x);
  });

  it("rejects malformed experiment JSON, strips NaN/empty optional fields, and avoids duplicate names/router prefixes", () => {
    expect(() => parseExperimentJson("{}")).toThrow();
    expect(() =>
      parseExperimentJson(JSON.stringify({ format_version: 9, id: "x", revision: "1", devices: [], links: [] }))
    ).toThrow();

    const lab = createStarterLab(1);
    // Starter lab already has SW1; creating a new switch must produce a distinct name
    const sw2 = createDevice(lab, "switch");
    expect(sw2.name).not.toBe("SW1");

    // Adding ports to a router never duplicates /24 third octets
    const router = createDevice(lab, "router");
    const withRouter: NativeLabDocument = {
      ...lab,
      devices: [...lab.devices, router],
    };
    const added = addPortToDevice(withRouter, router.id);
    expect(added).not.toBeNull();
    const updatedRouter = added!.doc.devices.find((d) => d.id === router.id);
    expect(updatedRouter?.kind).toBe("router");
    if (updatedRouter?.kind === "router") {
      const thirds = updatedRouter.ports.map((p) => p.ipv4.split(".")[2]);
      expect(new Set(thirds).size).toBe(thirds.length);
    }

    // NaN / empty optional scenario fields are omitted on serialization
    const dirtyLab: NativeLabDocument = {
      ...lab,
      scenario: {
        ...lab.scenario!,
        trace_limit: Number.NaN,
        routing_convergence_ns: "   ",
        probes: [
          {
            ...lab.scenario!.probes[0],
            ttl: Number.NaN,
            payload_bytes: Number.POSITIVE_INFINITY,
          },
        ],
      },
    };
    const serialized = JSON.parse(serializeExperimentJson(dirtyLab)) as {
      scenario: Record<string, unknown> & { probes: Array<Record<string, unknown>> };
    };
    expect("trace_limit" in serialized.scenario).toBe(false);
    expect("routing_convergence_ns" in serialized.scenario).toBe(false);
    expect("ttl" in serialized.scenario.probes[0]).toBe(false);
    expect("payload_bytes" in serialized.scenario.probes[0]).toBe(false);

    // Non-numeric end_ns in applyQuickProbe never throws BigInt SyntaxError
    const safeProbe = applyQuickProbe(
      { ...lab, scenario: { ...lab.scenario!, end_ns: "invalid-ns" } },
      "pc-b",
      "pc-a"
    );
    expect(safeProbe.scenario?.end_ns).toBe("5000");
  });

  it("cascades delete cleanly across link_state, stp_cost, convergence timers, v1 probe, and v3 import gaps", () => {
    const doc: NativeLabDocument = {
      format_version: 3,
      id: "cascade-lab",
      revision: "1",
      devices: [
        {
          kind: "router",
          id: "r1",
          name: "R1",
          ports: [
            { id: "r1-p1", name: "G0/0", ipv4: "198.51.100.1", prefix_length: 30, mac: "02:00:00:00:01:01" },
          ],
          routes: [],
          link_state: { peers: [{ port: "r1-p1", cost: 10 }], advertise: ["r1-p1"] },
        },
        {
          kind: "router",
          id: "r2",
          name: "R2",
          ports: [
            { id: "r2-p1", name: "G0/0", ipv4: "198.51.100.2", prefix_length: 30, mac: "02:00:00:00:02:01" },
          ],
          routes: [],
          link_state: { peers: [{ port: "r2-p1", cost: 10 }], advertise: ["r2-p1"] },
        },
        {
          kind: "switch",
          id: "sw1",
          name: "SW1",
          ports: [{ id: "s1-p1", name: "G0/1", stp_cost: 4, mode: { kind: "trunk", allowed_vlans: [10] } }],
        },
        {
          kind: "switch",
          id: "sw2",
          name: "SW2",
          ports: [{ id: "s2-p1", name: "G0/1", stp_cost: 4, mode: { kind: "trunk", allowed_vlans: [10] } }],
        },
      ],
      links: [
        { id: "r1-r2", ports: ["r1-p1", "r2-p1"], delay_ns: "100" },
        { id: "s1-s2", ports: ["s1-p1", "s2-p1"], delay_ns: "100" },
      ],
      scenario: {
        end_ns: "5000",
        arp_lifetime_ns: "1000000000",
        mac_lifetime_ns: "1000000000",
        routing_convergence_ns: "500",
        stp_convergence_ns: "500",
        warmup: [{ port: "r1-p1", neighbor: "198.51.100.2", at_ns: "0" }],
        probes: [],
      },
      import_source: {
        gaps: [
          { id: "g1", device_id: "r2", message: "r2 gap" },
          { id: "g2", device_id: "sw1", message: "keep me" },
        ],
      },
    };

    const afterLinkDel = deleteLinkFromDoc(doc, "s1-s2");
    const sw1 = afterLinkDel.devices.find((d) => d.id === "sw1");
    expect(sw1?.kind === "switch" && sw1.ports[0].stp_cost).toBeUndefined();
    expect(afterLinkDel.scenario?.stp_convergence_ns).toBeUndefined();

    const afterRouterDel = deleteDeviceFromDoc(afterLinkDel, "r2");
    const r1 = afterRouterDel.devices.find((d) => d.id === "r1");
    expect(r1?.kind === "router" && r1.link_state).toBeUndefined();
    expect(afterRouterDel.scenario?.routing_convergence_ns).toBeUndefined();
    expect(afterRouterDel.scenario?.warmup).toEqual([]);
    expect((afterRouterDel.import_source?.gaps as Array<{ id: string }>).map((g) => g.id)).toEqual(["g2"]);

    // Also verify format_version 1 probe cleanup & ghost position filtering in ensureNodePositions
    const v1Doc: NativeLabDocument = {
      format_version: 1,
      id: "v1-lab",
      revision: "1",
      devices: [
        { kind: "host", id: "h1", name: "H1", ipv4: "192.0.2.1", prefix_length: 24, mac: "02:00:00:00:00:01", port: { id: "h1-p", name: "eth0" } },
        { kind: "host", id: "h2", name: "H2", ipv4: "192.0.2.2", prefix_length: 24, mac: "02:00:00:00:00:02", port: { id: "h2-p", name: "eth0" } },
        { kind: "host", id: "h3", name: "H3", ipv4: "192.0.2.3", prefix_length: 24, mac: "02:00:00:00:00:03", port: { id: "h3-p", name: "eth0" } },
      ],
      links: [],
      probe: { source: "h1", destination: "h2", deadline_ns: "2000" },
    };
    const v1After = deleteDeviceFromDoc(v1Doc, "h2");
    expect(v1After.probe?.destination).toBe("h3");

    const pos = ensureNodePositions(v1After, { h1: { x: 1, y: 2 }, h2: { x: 99, y: 99 } });
    expect("h2" in pos).toBe(false);
  });
});
