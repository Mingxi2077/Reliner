"""Windows local comparison. Run with Python 3.11+ from the repository."""
import argparse
import http.client
import json
import os
from pathlib import Path
import socket
import subprocess
import tempfile
from time import perf_counter, sleep


def timed(fn, *args):
    start = perf_counter()
    result = fn(*args)
    return result, (perf_counter() - start) * 1000


def summary(samples):
    samples = sorted(samples)
    return [round(samples[(len(samples) * p + 99) // 100 - 1], 3)
            for p in (50, 95, 99, 100)]


def emit(**data):
    print(json.dumps(data), flush=True)


def fixture(work, index):
    name = f"bench-{index}"
    folder = work / name
    folder.mkdir()
    ids = [f"f0000000-0000-4000-8000-{index * 32 + i:012x}" for i in range(32)]
    layout = {node: {"x": (i // 2 % 8) * 95 - 330,
                     "y": (2 * (i // 16) + i % 2) * 100 - 150}
              for i, node in enumerate(ids)}
    nodes = [dict(node_id=node, name=f"PC-{i + 1}", node_type="vpcs", compute_id="local",
                  properties={}, z=1, **layout[node]) for i, node in enumerate(ids)]
    links = [dict(link_id=f"e0000000-0000-4000-8000-{index * 32 + i:012x}",
                  nodes=[dict(node_id=node, adapter_number=0, port_number=0)
                         for node in ids[i:i + 2]]) for i in range(0, 32, 2)]
    gid = f"d0000000-0000-4000-8000-{index:012x}"
    g = dict(type="topology", name=name, project_id=gid, revision=9, version="2.2.61",
             auto_close=False, auto_start=False, auto_open=False,
             topology=dict(nodes=nodes, links=links, computes=[], drawings=[]))
    r = dict(format_version=2, id=name, revision="1", presentation=layout,
             devices=[dict(id=node, name=f"PC-{i + 1}", kind="host",
                           ipv4=f"192.0.{i // 2 + 10}.{i % 2 + 1}", prefix_length=24,
                           mac=f"02:00:00:{index // 256:02x}:{index % 256:02x}:{i:02x}",
                           port=dict(id=f"{node}:a0p0", name="p0"))
                      for i, node in enumerate(ids)],
             links=[dict(id=link["link_id"], delay_ns="100",
                         ports=[f'{end["node_id"]}:a0p0' for end in link["nodes"]])
                    for link in links],
             scenario=dict(end_ns="5000", arp_lifetime_ns="1000000", mac_lifetime_ns="1000000",
                           probes=[dict(id="echo", source=ids[0], destination=ids[1],
                                        at_ns="0", deadline_ns="4000")]))
    gp, rp = folder / f"{name}.gns3", folder / f"{name}.json"
    gp.write_text(json.dumps(g), encoding="utf-8")
    rp.write_text(json.dumps(r), encoding="utf-8")
    return gp, rp, gid, ids[0], layout


def benchmark(work, server, exe, conn, count, samples, reopen):
    def g(method, path, data=None):
        conn.request(method, "/v2" + path, None if data is None else json.dumps(data),
                     {"Content-Type": "application/json"})
        response = conn.getresponse()
        body = response.read()
        if response.status not in (200, 201, 204):
            raise RuntimeError(f"GNS3 {method} {path}: {response.status}: {body!r}")
        return json.loads(body) if body else None

    deadline = perf_counter() + 15
    while True:
        try:
            version = g("GET", "/version")["version"]
            break
        except ConnectionRefusedError:
            if server.poll() is not None or perf_counter() >= deadline:
                raise
            sleep(.05)
    assert version == "2.2.61"
    with subprocess.Popen([exe, "workspace", "--jsonl"], stdin=subprocess.PIPE,
                          stdout=subprocess.PIPE, encoding="utf-8",
                          creationflags=subprocess.CREATE_NO_WINDOW) as reliner:
        req, sid = 0, None

        def r(kind, **cmd):
            nonlocal req
            req += 1
            packet = dict(protocol_version=1, request_id=str(req), command=dict(type=kind, **cmd))
            if sid is not None:
                packet["session_id"] = sid
            reliner.stdin.write(json.dumps(packet) + "\n")
            reliner.stdin.flush()
            reply = json.loads(reliner.stdout.readline())
            assert reply["request_id"] == str(req) and reply["status"] == "ok", reply
            return reply["data"]

        try:
            hello = r("hello")
            sid = hello["session_id"]
            emit(gns3=version, reliner=hello["crate_version"], nodes=32, links=16)
            fixtures = []
            for n in count:
                while len(fixtures) < n:
                    f = fixture(work, len(fixtures))
                    gp, rp, gid, _, _ = f
                    assert g("POST", "/projects/load", {"path": str(gp)})["project_id"] == gid
                    fixtures.append([f, r("document.open", path=str(rp))])
                assert sum(p["status"] == "opened" for p in g("GET", "/projects")) == n
                metrics = {}
                edited_slots = set()

                def measure(key, fn, *args, **kwargs):
                    result, elapsed[key] = timed(lambda: fn(*args, **kwargs))
                    return result

                for i in range(20 + samples):
                    edited_slots.add(i % n)
                    slot = fixtures[i % n]
                    (gp, rp, gid, node, layout), doc = slot
                    base = f"/projects/{gid}"
                    x = layout[node]["x"] = -329 if layout[node]["x"] == -330 else -330
                    rid = doc["id"]
                    elapsed = {}

                    nodes = measure("G_nodes", g, "GET", base + "/nodes")
                    links = measure("G_links", g, "GET", base + "/links")
                    elapsed["G_read_pair"] = elapsed["G_nodes"] + elapsed["G_links"]
                    view = measure("R_read", lambda: json.loads(
                        r("document.get", document_id=rid)["experiment"]))
                    assert (len(nodes), len(links)) == (32, 16)
                    assert all(device["status"] == "stopped" for device in nodes)
                    assert (len(view["devices"]), len(view["links"])) == (32, 16)
                    moved = measure("G_edit_save", g, "PUT", f"{base}/nodes/{node}", {"x": x})
                    edited = measure("R_edit", r, "document.replace_presentation", document_id=rid,
                                     expected_revision=doc["revision"], presentation=json.dumps(layout))
                    saved = measure("R_save", r, "document.save", document_id=rid,
                                    expected_revision=edited["revision"], path=str(rp))
                    elapsed["R_edit_save"] = elapsed["R_edit"] + elapsed["R_save"]
                    assert saved["saved_revision"] == edited["revision"] and moved["x"] == x
                    slot[1] = edited
                    if i >= 20:
                        for key, value in elapsed.items():
                            metrics.setdefault(key, []).append(value)
                for index in sorted(edited_slots):
                    (gp, rp, _, node, layout), _ = fixtures[index]
                    persisted_g = json.loads(gp.read_text(encoding="utf-8"))["topology"]["nodes"]
                    persisted_r = json.loads(rp.read_text(encoding="utf-8"))["presentation"]
                    x = layout[node]["x"]
                    assert next(item["x"] for item in persisted_g if item["node_id"] == node) == x
                    assert persisted_r[node]["x"] == x
                emit(documents=n, samples=samples, files_checked_per_product=len(edited_slots),
                     columns=["p50", "p95", "p99", "max"],
                     ms={key: summary(values) for key, values in metrics.items()})
                if not reopen:
                    continue
                metrics = {}
                for i in range(18):
                    slot = fixtures[i % n]
                    (gp, rp, gid, node, layout), doc = slot
                    base, rid = f"/projects/{gid}", doc["id"]
                    x = layout[node]["x"]
                    elapsed = {}
                    measure("G_close", g, "POST", base + "/close")
                    measure("R_close", r, "document.close", document_id=rid,
                            expected_revision=doc["revision"])
                    reopened = measure("G_open", g, "POST", "/projects/load", {"path": str(gp)})
                    assert reopened["project_id"] == gid
                    slot[1] = measure("R_open", r, "document.open", path=str(rp))
                    assert g("GET", f"{base}/nodes/{node}")["x"] == x
                    view = r("document.get", document_id=slot[1]["id"])
                    assert json.loads(view["presentation"])[node]["x"] == x
                    assert not slot[1]["dirty"]
                    if i >= 3:
                        for key, value in elapsed.items():
                            metrics.setdefault(key, []).append(value)
                emit(documents=n, samples=15, columns=["p50", "max"],
                     ms={key: [summary(values)[j] for j in (0, 3)]
                         for key, values in metrics.items()})
        finally:
            reliner.stdin.close()
            reliner.wait(timeout=10)
        assert reliner.returncode == 0


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--app", type=Path, default=Path(r"C:\Network\GNS3-2.2.61\app"))
    parser.add_argument("--reliner", type=Path,
                        default=Path("target/x86_64-pc-windows-msvc/release/reliner.exe"))
    parser.add_argument("--documents", type=int, nargs="+", default=[1, 16, 64])
    parser.add_argument("--samples", type=int, default=120)
    parser.add_argument("--reopen", action="store_true", help="also measure 15 closes and reopens per tier")
    args = parser.parse_args()
    app, exe = args.app.resolve(strict=True), args.reliner.resolve(strict=True)
    with tempfile.TemporaryDirectory(prefix="comparison-", dir=app.parent) as scratch:
        work = Path(scratch)
        config = work / "server.ini"
        config.write_text(f"[Server]\nprojects_path = {work}\nimages_path = {work / 'images'}\n"
                          f"ubridge_path = {app / 'ubridge/ubridge.exe'}\n"
                          f"[VPCS]\nvpcs_path = {app / 'vpcs/vpcs.exe'}\n", encoding="utf-8")
        with socket.socket() as listener:
            listener.bind(("127.0.0.1", 0))
            port = listener.getsockname()[1]
        env = os.environ | dict.fromkeys(("APPDATA", "LOCALAPPDATA", "COMMON_APPDATA", "USERPROFILE"),
                                         scratch)
        log = work / "server.log"
        conn = http.client.HTTPConnection("127.0.0.1", port, timeout=8)
        try:
            with log.open("w", encoding="utf-8") as output:
                server = subprocess.Popen([app / "gns3server.exe", "--host", "127.0.0.1",
                                           "--port", str(port), "--config", config, "--local"],
                                          cwd=work, env=env, stdout=output, stderr=subprocess.STDOUT,
                                          creationflags=subprocess.CREATE_NO_WINDOW)
                try:
                    benchmark(work, server, exe, conn, args.documents, args.samples, args.reopen)
                finally:
                    conn.close()
                    server.kill()
                    server.wait()
        except Exception:
            print("\n".join(log.read_text(encoding="utf-8", errors="replace").splitlines()[-18:]))
            raise


if __name__ == "__main__":
    main()
