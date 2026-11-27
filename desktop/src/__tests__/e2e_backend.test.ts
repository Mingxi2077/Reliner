import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import readline from "node:readline";
import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { BackendClient, type BridgeTransport } from "../protocol/client";
import type { BridgeResponse, HandshakeState } from "../protocol/types";
import { diagnoseProbe } from "../protocol/diagnosis";
import {
  bumpNativeRevision,
  parseExperimentJson,
  serializeExperimentJson,
} from "../model/document";

const repoRoot = path.resolve(__dirname, "..", "..", "..");
const relinerExe = [
  path.join(repoRoot, "target", "x86_64-pc-windows-msvc", "release", "reliner.exe"),
  path.join(repoRoot, "target", "release", "reliner.exe"),
].find((p) => fs.existsSync(p));

class NodeJsonlTransport implements BridgeTransport {
  private child: ChildProcessWithoutNullStreams | null = null;
  private rl: readline.Interface | null = null;
  private pending: Array<(line: string) => void> = [];
  private reqId = 0;
  private sessionId: string | null = null;
  private cachedHello: HandshakeState | null = null;

  constructor(private readonly exePath: string) {}

  private ensureSpawned() {
    if (this.child) return;
    const child = spawn(this.exePath, ["workspace", "--jsonl"], {
      stdio: ["pipe", "pipe", "pipe"],
    });
    const rl = readline.createInterface({ input: child.stdout });
    rl.on("line", (line) => {
      const next = this.pending.shift();
      if (next) next(line);
    });
    this.child = child;
    this.rl = rl;
  }

  private async send(command: Record<string, unknown>): Promise<BridgeResponse<unknown>> {
    this.ensureSpawned();
    this.reqId += 1;
    const id = String(this.reqId);
    const payload = JSON.stringify({
      protocol_version: 1,
      request_id: id,
      session_id: this.sessionId,
      command,
    });
    const linePromise = new Promise<string>((resolve) => {
      this.pending.push(resolve);
    });
    this.child!.stdin.write(`${payload}\n`);
    const line = await linePromise;
    return JSON.parse(line) as BridgeResponse<unknown>;
  }

  async handshake(): Promise<HandshakeState> {
    if (this.cachedHello) return this.cachedHello;
    const resp = await this.send({ type: "hello" });
    if (resp.status !== "ok") throw new Error(resp.error.message);
    this.sessionId = resp.session_id;
    this.cachedHello = {
      session_id: resp.session_id,
      backend_exe: this.exePath,
      hello: resp.data as HandshakeState["hello"],
    };
    return this.cachedHello;
  }

  async restart(): Promise<HandshakeState> {
    this.close();
    return this.handshake();
  }

  async call(command: Record<string, unknown>): Promise<BridgeResponse<unknown>> {
    await this.handshake();
    return this.send(command);
  }

  async listExamples() {
    return [];
  }

  async readExample(fileName: string) {
    return fs.readFileSync(path.join(repoRoot, "examples", fileName), "utf8");
  }

  close() {
    this.rl?.close();
    this.child?.stdin.end();
    this.child?.kill();
    this.rl = null;
    this.child = null;
    this.sessionId = null;
    this.cachedHello = null;
  }
}

async function waitTerminal(client: BackendClient, runId: string) {
  const start = Date.now();
  while (Date.now() - start < 10_000) {
    const res = await client.runSummary(runId);
    if (
      res.summary.status === "finished" ||
      res.summary.status === "incomplete" ||
      res.summary.status === "cancelled" ||
      res.summary.status === "failed"
    ) {
      return res.summary;
    }
    await new Promise((r) => setTimeout(r, 10));
  }
  throw new Error(`Run ${runId} did not settle`);
}

describe.skipIf(!relinerExe)("Reliner Phase-1 troubleshooting loop against real reliner.exe", () => {
  let transport: NodeJsonlTransport;
  let client: BackendClient;
  const tempFile = path.join(
    os.tmpdir(),
    `reliner-desktop-e2e-${process.pid}-${Date.now()}.json`
  );

  beforeAll(async () => {
    transport = new NodeJsonlTransport(relinerExe!);
    client = new BackendClient(transport);
    const hello = await client.handshake();
    expect(hello.hello.protocol_version).toBe(1);
    expect(hello.hello.backend_api_version).toBe(2);
  });

  afterAll(() => {
    transport.close();
    if (fs.existsSync(tempFile)) {
      fs.unlinkSync(tempFile);
    }
  });

  it("completes VLAN fault discovery -> layout move -> config fix -> save_and_rerun -> run.compare", async () => {
    const raw = await transport.readExample("vlan-lab.json");
    const docInfo1 = await client.openDocumentInput(raw);
    expect(docInfo1.format_version).toBe(1);

    // 1. Canvas layout edit must NOT change effective_revision
    const layoutInfo = await client.replacePresentation(
      docInfo1.id,
      docInfo1.revision,
      JSON.stringify({ version: 1, nodes: { "pc-a": { x: 100, y: 200 } } })
    );
    expect(layoutInfo.revision).not.toBe(docInfo1.revision);
    expect(layoutInfo.effective_revision).toBe(docInfo1.effective_revision);

    // 2. Validate before run
    const validation = await client.validateDocument(docInfo1.id, layoutInfo.revision);
    expect(validation.status).toBe("valid");

    // 3. Submit baseline run
    const run1 = await client.submitRun(docInfo1.id, layoutInfo.revision, "automatic");
    const summary1 = await waitTerminal(client, run1.run.id);
    expect(summary1.status).toBe("finished");
    expect(summary1.probe_counts?.timed_out).toBe(1);

    const probes1 = await client.runProbes(run1.run.id, 0, 10);
    const evidence1 = await client.runEvidence(run1.run.id, 0, 50);
    const diag1 = diagnoseProbe(
      probes1.entries[0],
      new Map(evidence1.entries.map((e) => [e.id, e])),
      []
    );
    expect(diag1.severity).toBe("fault");
    expect(diag1.target?.portId).toBe("sw2-uplink");

    // 4. Fix SW2/G0/1 allowed_vlans -> [10, 20], bump revision -> "2"
    const docObj = parseExperimentJson(raw);
    const sw2 = docObj.devices.find((d) => d.id === "sw2");
    if (!sw2 || sw2.kind !== "switch") throw new Error("sw2 missing");
    const uplink = sw2.ports.find((p) => p.id === "sw2-uplink");
    if (!uplink || uplink.mode.kind !== "trunk") throw new Error("sw2-uplink missing");
    uplink.mode.allowed_vlans = [10, 20];
    docObj.revision = bumpNativeRevision(docObj.revision);

    const editedInfo = await client.replaceExperiment(
      docInfo1.id,
      layoutInfo.revision,
      serializeExperimentJson(docObj)
    );
    expect(editedInfo.effective_revision).not.toBe(layoutInfo.effective_revision);
    expect(editedInfo.native_revision).toBe("2");

    // 5. Save and rerun
    const saved = await client.saveAndRerun(
      docInfo1.id,
      editedInfo.revision,
      tempFile,
      "automatic"
    );
    expect(saved.rerun.status).toBe("accepted");
    if (saved.rerun.status !== "accepted") throw new Error("rerun rejected");

    const run2Id = saved.rerun.run.id;
    const summary2 = await waitTerminal(client, run2Id);
    expect(summary2.status).toBe("finished");
    expect(summary2.probe_counts?.reachable).toBe(1);

    const probes2 = await client.runProbes(run2Id, 0, 10);
    const evidence2 = await client.runEvidence(run2Id, 0, 50);
    const diag2 = diagnoseProbe(
      probes2.entries[0],
      new Map(evidence2.entries.map((e) => [e.id, e])),
      []
    );
    expect(diag2.severity).toBe("ok");

    // 6. Compare baseline vs candidate runs
    const comparison = await client.compareRuns(run1.run.id, run2Id, 0, 10);
    expect(comparison.baseline.revision).toBe("1");
    expect(comparison.candidate.revision).toBe("2");
    expect(comparison.entries[0].same_probe_parameters).toBe(true);
    expect(comparison.entries[0].baseline?.outcome.status).toBe("timeout");
    expect(comparison.entries[0].candidate?.outcome.status).toBe("reachable");
  });

  it("supports paused start, step, and resume controls on ipv4-lab.json", async () => {
    const raw = await transport.readExample("ipv4-lab.json");
    const doc = await client.openDocumentInput(raw);
    const run = await client.submitRun(doc.id, doc.revision, "paused");

    // Wait until paused
    let state = (await client.runSummary(run.run.id)).summary;
    const deadline = Date.now() + 5000;
    while (state.status !== "paused" && Date.now() < deadline) {
      await new Promise((r) => setTimeout(r, 10));
      state = (await client.runSummary(run.run.id)).summary;
    }
    expect(state.status).toBe("paused");

    // Step one event
    await client.controlRun(run.run.id, "step");
    let stepped = (await client.runSummary(run.run.id)).summary;
    while (stepped.update === state.update && Date.now() < deadline) {
      await new Promise((r) => setTimeout(r, 10));
      stepped = (await client.runSummary(run.run.id)).summary;
    }
    expect(stepped.last_event_id).not.toBeNull();

    // Resume to completion
    await client.controlRun(run.run.id, "resume");
    const finalSummary = await waitTerminal(client, run.run.id);
    expect(finalSummary.status).toBe("finished");
    expect(finalSummary.probe_counts?.reachable).toBe(2);
  });

  it("diagnoses campus-lab (STP, VLAN, dynamic routing, queue congestion, unlocated timeout)", async () => {
    const raw = await transport.readExample("campus-lab.json");
    const doc = await client.openDocumentInput(raw);
    const run = await client.submitRun(doc.id, doc.revision, "automatic");
    const summary = await waitTerminal(client, run.run.id);
    expect(summary.status).toBe("finished");

    const [probes, evidence, routes, stp] = await Promise.all([
      client.runProbes(run.run.id, 0, 20),
      client.runEvidence(run.run.id, 0, 128),
      client.runRoutes(run.run.id, 0, 64),
      client.runStp(run.run.id, 0, 64),
    ]);
    expect(probes.entries.length).toBe(7);
    expect(stp.entries.length).toBeGreaterThan(0);

    const evMap = new Map(evidence.entries.map((e) => [e.id, e]));

    // 1. vlan probe -> SW3/Direct ingress VLAN 10 drop with stp_event_id
    const vlanProbe = probes.entries.find((p) => p.id === "vlan")!;
    const vlanDiag = diagnoseProbe(vlanProbe, evMap, routes.entries);
    expect(vlanDiag.severity).toBe("fault");
    expect(vlanDiag.target?.portId).toBe("s3-s1");
    expect(vlanDiag.stpEventId).toBeTruthy();

    // Verify STP drill-down API using the exact stpEventId
    const [bridges, stpLinks] = await Promise.all([
      client.runStpBridges(run.run.id, vlanDiag.stpEventId!, 0, 64),
      client.runStpLinks(run.run.id, vlanDiag.stpEventId!, 0, 64),
    ]);
    expect(bridges.entries.length).toBeGreaterThan(0);
    expect(stpLinks.entries.length).toBeGreaterThan(0);

    // 2. short-deadline -> unlocated timeout (no fabricated cause)
    const shortProbe = probes.entries.find((p) => p.id === "short-deadline")!;
    const shortDiag = diagnoseProbe(shortProbe, evMap, routes.entries);
    expect(shortDiag.severity).toBe("unlocated");

    // 3. old-route -> link down using previously installed dynamic routing table
    const oldRouteProbe = probes.entries.find((p) => p.id === "old-route")!;
    const oldRouteDiag = diagnoseProbe(oldRouteProbe, evMap, routes.entries);
    expect(oldRouteDiag.severity).toBe("fault");
    expect(oldRouteDiag.routingBasis).toContain("2500 ns");

    // 4. burst-reply -> return-path transmit_queue_full on r1-r2
    const burstProbe = probes.entries.find((p) => p.id === "burst-reply")!;
    const burstDiag = diagnoseProbe(burstProbe, evMap, routes.entries);
    expect(burstDiag.severity).toBe("fault");
    expect(burstDiag.requestNote).toContain("PC-B");
    expect(burstDiag.target?.linkId).toBe("r1-r2");
  });

  it("diagnoses ACL deny and returns effective_settings for native-vlan-lab", async () => {
    const aclRaw = await transport.readExample("acl-lab.json");
    const aclDoc = await client.openDocumentInput(aclRaw);
    const aclRun = await client.submitRun(aclDoc.id, aclDoc.revision, "automatic");
    await waitTerminal(client, aclRun.run.id);
    const [aclProbes, aclEv] = await Promise.all([
      client.runProbes(aclRun.run.id, 0, 10),
      client.runEvidence(aclRun.run.id, 0, 64),
    ]);
    const aclDiag = diagnoseProbe(
      aclProbes.entries[0],
      new Map(aclEv.entries.map((e) => [e.id, e])),
      []
    );
    expect(aclDiag.severity).toBe("fault");
    expect(aclDiag.target?.portId).toBe("r1-a");
    expect(aclDiag.requestNote).toContain("600 ns");

    const nativeRaw = await transport.readExample("native-vlan-lab.json");
    const nativeDoc = await client.openDocumentInput(nativeRaw);
    const eff = await client.effectiveSettings(nativeDoc.id, nativeDoc.revision, 0, 64);
    expect(eff.entries.some((s) => s.key === "native_vlan" && s.source === "explicit")).toBe(true);
  });

  it("handles 64 open documents sharing one workspace with background runs and layout updates", async () => {
    const raw = await transport.readExample("campus-lab.json");
    const baseObj = parseExperimentJson(raw);
    const opened: Array<{ id: string; revision: string; effective: string }> = [];

    for (let i = 0; i < 64; i++) {
      const clone = { ...baseObj, id: `tab-doc-${i}` };
      const info = await client.openDocumentInput(serializeExperimentJson(clone));
      opened.push({
        id: info.id,
        revision: info.revision,
        effective: info.effective_revision,
      });
    }

    // Launch background runs on 2 tabs while editing layout on other tabs
    const bgRun1 = await client.submitRun(opened[0].id, opened[0].revision, "automatic");
    const bgRun2 = await client.submitRun(opened[1].id, opened[1].revision, "automatic");

    for (let i = 2; i < 64; i += 8) {
      const updated = await client.replacePresentation(
        opened[i].id,
        opened[i].revision,
        JSON.stringify({ version: 1, nodes: { "pc-a": { x: 100 + i, y: 200 } } })
      );
      expect(updated.effective_revision).toBe(opened[i].effective);
      opened[i].revision = updated.revision;
    }

    const [s1, s2] = await Promise.all([
      waitTerminal(client, bgRun1.run.id),
      waitTerminal(client, bgRun2.run.id),
    ]);
    expect(s1.status).toBe("finished");
    expect(s2.status).toBe("finished");

    // Cleanly close all 64 temporary docs
    for (const item of opened) {
      await client.closeDocument(item.id, item.revision, true);
    }
  });

  it("round-trips and validates all 11 built-in native lab examples without loss", async () => {
    const exampleFiles = [
      "vlan-lab.json",
      "native-vlan-lab.json",
      "ipv4-lab.json",
      "failover-lab.json",
      "mtu-lab.json",
      "congestion-lab.json",
      "routing-lab.json",
      "stp-lab.json",
      "acl-lab.json",
      "campus-lab.json",
      "stack-static-lab.json",
    ];
    for (const file of exampleFiles) {
      const raw = await transport.readExample(file);
      const parsed = parseExperimentJson(raw);
      const reserialized = serializeExperimentJson(parsed);
      const info = await client.openDocumentInput(reserialized);
      const val = await client.validateDocument(info.id, info.revision);
      expect(val.status, `${file} failed validation`).toBe("valid");
      await client.closeDocument(info.id, info.revision, true);
    }
  });
});
