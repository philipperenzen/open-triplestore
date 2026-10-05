#!/usr/bin/env python3
"""Run the comparison: one store at a time, each alone on the machine.

For every (store, scale) pair this script

  1. waits for a quiet window (no rustc/cargo process, 1-minute load average
     under --max-load), polling every --poll seconds, and records the load;
  2. starts the store from its pinned compose file under the shared memory and
     CPU limits, loads the BSBM N-Triples with the store's documented bulk path
     and times it;
  3. waits the same settle time for every store, then runs the workloads from a
     load-generator container on the store's Docker network (bench.py client);
  4. samples the store container's memory (docker stats) throughout and keeps
     the peak per phase;
  5. removes the containers and the volume.

Usage (from the repository root):

  python3 scripts/bench-compare/campaign.py --data-root <dir with bsbm-<scale>/> \\
      --cache <dir with the Fuseki jar> --out <raw results dir> \\
      --stores ots,oxigraph,fuseki,qlever,virtuoso,rdf4j --scales 1k,10k \\
      --ots-image ots-bench:<commit>

Requires Docker with the compose plugin. Prints progress to stderr.
"""
import argparse
import json
import os
import platform
import re
import secrets
import subprocess
import sys
import threading
import time
import urllib.error
import urllib.parse
import urllib.request

HERE = os.path.dirname(os.path.abspath(__file__))
COMPOSE = os.path.join(HERE, "compose")
PROJECT = "benchcmp"
NETWORK = f"{PROJECT}_default"
CLIENT_IMAGE = "python:3.12-slim@sha256:78387bc3881b8273120a12ebe6c1ab22b018ccc2c9adf565ae1ac9b536e184ea"
BUSYBOX = "busybox:1.37"
OTS_GRAPH = "http://example.org/bench/bsbm"
OTS_SHAPES_GRAPH = "http://example.org/bench/shapes"
VIRTUOSO_GRAPH = "http://example.org/bench/bsbm"

# In-network endpoints (the client container resolves the compose service name).
ENDPOINTS = {
    "ots": "http://ots:7878/api/datasets/bench/services/sparql/sparql",
    "oxigraph": "http://oxigraph:7878/query",
    "fuseki": "http://fuseki:3030/ds/sparql",
    "qlever": "http://qlever:7001/",
    "virtuoso": "http://virtuoso:8890/sparql?default-graph-uri=" + urllib.parse.quote(VIRTUOSO_GRAPH, safe=""),
    "rdf4j": "http://rdf4j:8080/rdf4j-server/repositories/bench",
}
SHACL = {
    "fuseki": ("fuseki", "http://fuseki:3030/ds/shacl?graph=default"),
    # ?test=true: validate and answer without recording the run or writing the
    # report graph, like Fuseki's /shacl service.
    "ots": ("ots", "http://ots:7878/api/datasets/bench/validate?test=true"),
}
LOAD_METHOD = {
    "ots": "HTTP Graph Store POST into a dataset graph, 100 MB N-Triples chunks",
    "oxigraph": "`oxigraph load` (offline bulk loader) + `oxigraph optimize`",
    "fuseki": "`tdb2.tdbloader` (offline, default loader)",
    "qlever": "`qlever-index` (offline index build)",
    "virtuoso": "`ld_dir` + `rdf_loader_run()` + `checkpoint` (bulk loader)",
    "rdf4j": "HTTP `POST /statements`, 100 MB N-Triples chunks",
}
# Second load measurement over HTTP for stores whose primary path is offline.
HTTP_LOAD = {
    "oxigraph": "http://oxigraph:7878/store?default",
    "fuseki": "http://fuseki:3030/ds/data?default",
}


def log(msg):
    print(f"[{time.strftime('%H:%M:%S')}] {msg}", file=sys.stderr, flush=True)


def sh(cmd, env=None, check=True, capture=True, timeout=None):
    r = subprocess.run(cmd, env=env, capture_output=capture, text=True, timeout=timeout)
    if check and r.returncode != 0:
        raise RuntimeError(f"{' '.join(cmd)} failed ({r.returncode}): {r.stderr[-2000:] if capture else ''}")
    return r


def loadavg():
    return list(os.getloadavg())


def busy_builds():
    names = []
    for proc in ("rustc", "cargo"):
        r = subprocess.run(["pgrep", "-x", proc], capture_output=True, text=True)
        if r.stdout.strip():
            names.append(f"{proc}×{len(r.stdout.split())}")
    return names


class NotQuiet(Exception):
    pass


def swap_used():
    r = subprocess.run(["sysctl", "-n", "vm.swapusage"], capture_output=True, text=True)
    return r.stdout.strip()


def free_gib(path="/System/Volumes/Data" if os.path.isdir("/System/Volumes/Data") else "/"):
    st = os.statvfs(path)
    return st.f_bavail * st.f_frsize / 2**30


MIN_FREE_GIB = 15.0


def wait_quiet(max_load, poll, max_wait, require=False):
    """Wait for load < max_load, no rustc/cargo, and at least MIN_FREE_GIB free disk."""
    t0 = time.time()
    if max_wait <= 0:  # smoke tests: record the load, do not wait for quiet
        return {"load_before": loadavg(), "waited_s": 0, "not_quiet": True, "builds": busy_builds()}
    while True:
        la = loadavg()
        builds = busy_builds()
        disk = free_gib()
        if disk < MIN_FREE_GIB:
            log(f"only {disk:.1f} GiB free disk (< {MIN_FREE_GIB}); waiting")
            time.sleep(poll)
            continue
        if la[0] < max_load and not builds:
            return {"load_before": la, "waited_s": round(time.time() - t0), "free_gib": round(disk, 1)}
        if time.time() - t0 > max_wait:
            if require:
                raise NotQuiet(f"no quiet window within {max_wait}s (load {la[0]:.1f}, builds {builds or 'none'})")
            log(f"no quiet window after {max_wait}s (load {la[0]:.1f}, {builds}); running anyway, flagged")
            return {"load_before": la, "waited_s": round(time.time() - t0), "not_quiet": True, "builds": builds}
        log(f"waiting for a quiet window: load {la[0]:.1f}/{la[1]:.1f}/{la[2]:.1f}, builds {builds or 'none'}")
        time.sleep(poll)


class MemSampler:
    """Streams `docker stats` for one container; keeps (time, bytes) samples."""

    UNITS = {"B": 1, "KiB": 1024, "MiB": 1024 ** 2, "GiB": 1024 ** 3, "kB": 1000, "MB": 1000 ** 2, "GB": 1000 ** 3}

    def __init__(self, container):
        self.container = container
        self.samples = []
        self.proc = subprocess.Popen(["docker", "stats", "--format", "{{.MemUsage}}", container],
                                     stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True)
        self.thread = threading.Thread(target=self._read, daemon=True)
        self.thread.start()

    def _read(self):
        for line in self.proc.stdout:
            m = re.search(r"([\d.]+)\s*([KMG]i?B|B|kB)\s*/", line.replace("\x1b[2J\x1b[H", ""))
            if m:
                self.samples.append((time.time(), float(m.group(1)) * self.UNITS.get(m.group(2), 1)))

    def peak(self, t0=None, t1=None):
        vals = [b for t, b in self.samples if (t0 is None or t >= t0) and (t1 is None or t <= t1)]
        return max(vals) if vals else None

    def stop(self):
        self.proc.terminate()


def http(url, data=None, headers=None, method=None, timeout=60):
    req = urllib.request.Request(url, data=data, headers=headers or {}, method=method)
    with urllib.request.urlopen(req, timeout=timeout) as r:
        return r.status, r.read()


def wait_http(url, timeout=600, ok=(200,), data=None, headers=None):
    t0 = time.time()
    last = None
    while time.time() - t0 < timeout:
        try:
            st, _ = http(url, data=data, headers=headers, timeout=10)
            if st in ok:
                return time.time() - t0
        except urllib.error.HTTPError as e:
            last = e.code
            if e.code in ok:
                return time.time() - t0
        except Exception as e:  # connection refused while starting
            last = type(e).__name__
        time.sleep(1)
    raise RuntimeError(f"{url} not ready after {timeout}s (last: {last})")


class Run:
    def __init__(self, a, store, scale):
        self.a, self.store, self.scale = a, store, scale
        self.data_dir = os.path.join(a.data_root, f"bsbm-{scale}")
        self.out = os.path.join(a.out, store, scale)
        os.makedirs(self.out, exist_ok=True)
        self.port = a.port
        self.base = f"http://127.0.0.1:{self.port}"
        self.compose = os.path.join(COMPOSE, f"{store}.yml")
        self.container = f"{PROJECT}-{store}-1"
        self.env = dict(os.environ)
        self.env.update({
            "BENCH_DATA": self.data_dir, "BENCH_CACHE": a.cache, "BENCH_PORT": str(self.port),
            "BENCH_MEM": a.mem, "BENCH_CPUS": str(a.cpus),
            "OTS_IMAGE": a.ots_image or "unset", "OTS_JWT_SECRET": secrets.token_hex(32),
            "VIRTUOSO_DBA_PASSWORD": secrets.token_hex(12),
        })
        self.meta = {"store": store, "scale": scale, "limits": {"memory": a.mem, "cpus": a.cpus},
                     "load_method": LOAD_METHOD[store], "phases": {}, "commands": []}
        self.token = None

    # -- docker helpers --------------------------------------------------
    def compose_cmd(self, *args):
        cmd = ["docker", "compose", "-f", self.compose, "-p", PROJECT, *args]
        self.meta["commands"].append(" ".join(cmd))
        return cmd

    def down(self):
        sh(self.compose_cmd("--profile", "load", "down", "-v", "--remove-orphans"), env=self.env, check=False)

    def up(self):
        sh(self.compose_cmd("up", "-d", self.store), env=self.env)

    def client(self, workload, out_name, *extra, timeout_s=None):
        cmd = ["docker", "run", "--rm", "--network", NETWORK, "--cpus", "2", "-m", "2g",
               "-v", f"{HERE}:/h:ro", "-v", f"{self.data_dir}:/bench:ro", "-v", f"{self.out}:/out",
               CLIENT_IMAGE, "python", "/h/bench.py", "client", "--store", self.store,
               "--workload", workload, "--out", f"/out/{out_name}.json", *extra]
        self.meta["commands"].append(" ".join(c if "Bearer" not in c else "Authorization: Bearer <token>" for c in cmd)
                                     .replace(self.token or "\0", "<token>"))
        t0 = time.time()
        r = subprocess.run(cmd, capture_output=True, text=True, timeout=timeout_s)
        sys.stderr.write(r.stderr[-4000:])
        if r.returncode != 0:
            raise RuntimeError(f"client {workload} failed: {r.stderr[-1500:]}")
        return t0, time.time()

    def phase(self, name, t0, t1, sampler, **extra):
        p = {"seconds": round(t1 - t0, 3), "peak_mem_bytes": sampler.peak(t0, t1) if sampler else None,
             "load_after": loadavg()}
        p.update(extra)
        self.meta["phases"][name] = p
        log(f"{self.store}/{self.scale} {name}: {t1 - t0:.1f}s, peak mem "
            f"{(p['peak_mem_bytes'] or 0) / 2**20:.0f} MiB")

    def db_size(self):
        r = sh(["docker", "run", "--rm", "-v", f"{PROJECT}_db:/v:ro", BUSYBOX, "du", "-sk", "/v"], check=False)
        try:
            return int(r.stdout.split()[0]) * 1024
        except Exception:
            return None

    # -- store-specific load ----------------------------------------------
    def load(self):
        st = self.store
        if st in ("oxigraph", "fuseki", "qlever"):
            name = f"{PROJECT}-loader"
            cmd = self.compose_cmd("--profile", "load", "run", "--rm", "--name", name, "load")
            p = subprocess.Popen(cmd, env=self.env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
            t0 = time.time()
            time.sleep(1.0)
            sampler = MemSampler(name)
            out, _ = p.communicate()
            t1 = time.time()
            sampler.stop()
            if p.returncode != 0:
                raise RuntimeError(f"{st} load failed: {out[-3000:]}")
            if st == "oxigraph":
                t_opt = time.time()
                r = sh(self.compose_cmd("--profile", "load", "run", "--rm", "optimize"), env=self.env, check=False)
                out += r.stdout + r.stderr
                if r.returncode != 0:
                    raise RuntimeError(f"oxigraph optimize failed: {r.stderr[-2000:]}")
                self.meta["optimize_seconds"] = round(time.time() - t_opt, 3)
                t1 = time.time()
            with open(os.path.join(self.out, "load.log"), "w") as f:
                f.write(out[-20000:])
            self.phase("load", t0, t1, sampler, method=LOAD_METHOD[st])
            self.up()
            return
        self.up()
        sampler = MemSampler(self.container)
        try:
            if st == "virtuoso":
                self._wait_virtuoso()
                pw = self.env["VIRTUOSO_DBA_PASSWORD"]
                sql = (f"ld_dir('/bench', 'dataset.nt', '{VIRTUOSO_GRAPH}'); rdf_loader_run(); checkpoint; "
                       "select count(*) from DB.DBA.load_list where ll_error is not null;")
                t0 = time.time()
                r = sh(["docker", "exec", self.container, "isql", "1111", "dba", pw, f"exec={sql}"], check=False)
                t1 = time.time()
                self.meta["commands"].append(f"docker exec {self.container} isql 1111 dba <password> exec=\"{sql}\"")
                with open(os.path.join(self.out, "load.log"), "w") as f:
                    f.write(r.stdout[-20000:] + r.stderr[-5000:])
                if r.returncode != 0:
                    raise RuntimeError(f"virtuoso load failed: {r.stdout[-2000:]} {r.stderr[-2000:]}")
                self.phase("load", t0, t1, sampler, method=LOAD_METHOD[st])
            elif st == "ots":
                wait_http(self.base + "/livez", 600)
                self._ots_setup()
                t0, t1 = self.client("load-http", "load", "--endpoint",
                                     f"http://ots:7878/store?graph={urllib.parse.quote(OTS_GRAPH, safe='')}",
                                     "--token", self.token, "--file", "/bench/dataset.nt", "--chunk-mb", "100")
                self.phase("load", t0, t1, sampler, method=LOAD_METHOD[st])
            elif st == "rdf4j":
                wait_http(self.base + "/rdf4j-server/protocol", 600)
                with open(os.path.join(HERE, "config", "rdf4j-repo.ttl"), "rb") as f:
                    cfg = f.read()
                http(self.base + "/rdf4j-server/repositories/bench", data=cfg,
                     headers={"Content-Type": "text/turtle"}, method="PUT")
                t0, t1 = self.client("load-http", "load", "--endpoint",
                                     "http://rdf4j:8080/rdf4j-server/repositories/bench/statements",
                                     "--file", "/bench/dataset.nt", "--chunk-mb", "100")
                self.phase("load", t0, t1, sampler, method=LOAD_METHOD[st])
            if st in ("ots", "rdf4j"):
                with open(os.path.join(self.out, "load.json")) as f:
                    if not json.load(f).get("ok"):
                        raise RuntimeError(f"{st} HTTP load reported a failed chunk")
        finally:
            sampler.stop()

    def _wait_virtuoso(self):
        pw = self.env["VIRTUOSO_DBA_PASSWORD"]
        for _ in range(300):
            r = sh(["docker", "exec", self.container, "isql", "1111", "dba", pw, "exec=select 1;"], check=False)
            if r.returncode == 0:
                return
            time.sleep(2)
        raise RuntimeError("virtuoso did not come up")

    def _ots_setup(self):
        pw = secrets.token_hex(12)
        body = json.dumps({"username": "bench", "email": "bench@example.org", "password": pw}).encode()
        _, out = http(self.base + "/api/auth/register", data=body, headers={"Content-Type": "application/json"})
        self.token = json.loads(out)["access_token"]
        auth = {"Authorization": f"Bearer {self.token}", "Content-Type": "application/json"}
        _, me = http(self.base + "/api/auth/me", headers=auth)
        me = json.loads(me)
        uid = me.get("id") or me.get("user_id") or (me.get("user") or {}).get("id")
        body = json.dumps({"name": "Bench", "slug": "bench", "owner_type": "user", "owner_id": uid,
                           "visibility": "public"}).encode()
        _, ds = http(self.base + "/api/datasets", data=body, headers=auth)
        ds = json.loads(ds)
        ds_id = ds.get("id") or ds.get("slug")
        if ds_id != "bench" and ds.get("slug") != "bench":
            raise RuntimeError(f"OTS dataset not addressable as 'bench': {ds}")
        for iri, role in ((OTS_GRAPH, "instances"), (OTS_SHAPES_GRAPH, "shapes")):
            http(self.base + "/api/datasets/bench/graphs", data=json.dumps({"graph_iri": iri, "role": role}).encode(),
                 headers=auth)
        with open(os.path.join(HERE, "shapes", "bsbm-shapes.ttl"), "rb") as f:
            shapes = f.read()
        http(self.base + f"/store?graph={urllib.parse.quote(OTS_SHAPES_GRAPH, safe='')}", data=shapes,
             headers={"Authorization": f"Bearer {self.token}", "Content-Type": "text/turtle"}, method="PUT")

    def http_load_only(self):
        """Second load measurement: HTTP POST into an empty store."""
        url = HTTP_LOAD[self.store]
        self.down()
        self.up()
        sampler = MemSampler(self.container)
        try:
            ready = {"oxigraph": "/query?query=ASK%7B%7D", "fuseki": "/$/ping"}[self.store]
            wait_http(self.base + ready, 600)
            t0, t1 = self.client("load-http", "load_http", "--endpoint", url,
                                 "--file", "/bench/dataset.nt", "--chunk-mb", "100")
            with open(os.path.join(self.out, "load_http.json")) as f:
                if not json.load(f).get("ok"):
                    raise RuntimeError(f"{self.store} HTTP load reported a failed chunk")
            self.phase("load_http", t0, t1, sampler, method="HTTP Graph Store POST, 100 MB N-Triples chunks")
        finally:
            sampler.stop()

    # -- main sequence -----------------------------------------------------
    def execute(self):
        a = self.a
        require = self.scale in a.require_quiet.split(",")
        self.meta["quiet"] = wait_quiet(a.max_load, a.poll, a.max_wait, require)
        self.meta["quiet"]["swap"] = swap_used()
        self.meta["started"] = time.strftime("%Y-%m-%dT%H:%M:%S%z")
        self.down()
        try:
            self.load()
            self.meta["db_bytes_after_load"] = self.db_size()
            # Equal settle for every store (lets background index/accelerator builds finish).
            log(f"{self.store}/{self.scale}: settling {a.settle}s")
            time.sleep(a.settle)
            sampler = MemSampler(self.container)
            try:
                ep = ENDPOINTS[self.store]
                tok = ["--token", self.token] if self.token else []
                params = f"/bench/params.json"
                wl = set(a.workloads.split(","))
                steps = [
                    ("explore", ["--endpoint", ep, "--params", params, *tok]),
                    ("features", ["--endpoint", ep, "--features", "/h/queries/features",
                                  "--feature-runs", str(a.feature_runs), *tok]),
                    ("concurrent", ["--endpoint", ep, "--params", params, "--clients", a.clients,
                                    "--seconds", str(a.seconds), *tok]),
                ]
                if self.store in SHACL:
                    kind, url = SHACL[self.store]
                    steps.append(("shacl", ["--endpoint", url, "--shacl-kind", kind,
                                            "--shapes", "/h/shapes/bsbm-shapes.ttl",
                                            "--shacl-runs", str(a.shacl_runs),
                                            *(["--shapes-graph", OTS_SHAPES_GRAPH] if self.store == "ots" else []),
                                            *tok]))
                for name, args in steps:
                    if name in wl:
                        t0, t1 = self.client(name, name, *args)
                        self.phase(name, t0, t1, sampler)
            finally:
                sampler.stop()
            self.meta["image"] = sh(["docker", "inspect", "--format", "{{.Image}} {{.Config.Image}}", self.container],
                                    check=False).stdout.strip()
            if self.store in HTTP_LOAD and not a.skip_http_load:
                self.http_load_only()
        finally:
            self.meta["finished"] = time.strftime("%Y-%m-%dT%H:%M:%S%z")
            self.meta["load_after"] = loadavg()
            logs = sh(["docker", "logs", "--tail", "200", self.container], check=False)
            with open(os.path.join(self.out, "server.log"), "w") as f:
                f.write((logs.stdout + logs.stderr)[-40000:])
            self.down()
            with open(os.path.join(self.out, "meta.json"), "w") as f:
                json.dump(self.meta, f, indent=1)


def host_info():
    info = {"platform": platform.platform(), "python": platform.python_version()}
    for k, cmd in {"cpu": ["sysctl", "-n", "machdep.cpu.brand_string"], "mem_bytes": ["sysctl", "-n", "hw.memsize"],
                   "ncpu": ["sysctl", "-n", "hw.ncpu"], "macos": ["sw_vers", "-productVersion"],
                   "docker": ["docker", "version", "--format", "{{.Server.Version}}"],
                   "docker_vm": ["docker", "info", "--format", "{{.NCPU}} CPUs, {{.MemTotal}} bytes, kernel {{.KernelVersion}}"]}.items():
        r = subprocess.run(cmd, capture_output=True, text=True)
        info[k] = r.stdout.strip()
    return info


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--data-root", required=True)
    ap.add_argument("--cache", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--stores", default="ots,oxigraph,fuseki,qlever,virtuoso,rdf4j")
    ap.add_argument("--scales", default="1k,10k")
    ap.add_argument("--ots-image")
    ap.add_argument("--mem", default="4g")
    ap.add_argument("--cpus", type=int, default=4)
    ap.add_argument("--port", type=int, default=17001)
    ap.add_argument("--settle", type=int, default=60)
    ap.add_argument("--clients", default="1,4,8")
    ap.add_argument("--seconds", type=int, default=60)
    ap.add_argument("--feature-runs", type=int, default=10)
    ap.add_argument("--shacl-runs", type=int, default=5)
    ap.add_argument("--max-load", type=float, default=6.0)
    ap.add_argument("--poll", type=int, default=180)
    ap.add_argument("--max-wait", type=int, default=5400)
    ap.add_argument("--skip-http-load", action="store_true")
    ap.add_argument("--workloads", default="explore,features,concurrent,shacl")
    ap.add_argument("--require-quiet", default="",
                    help="comma-separated scales that run only in a quiet window (skipped otherwise)")
    a = ap.parse_args()
    os.makedirs(a.out, exist_ok=True)
    with open(os.path.join(a.out, "host.json"), "w") as f:
        json.dump(host_info(), f, indent=1)
    failures = []
    for scale in a.scales.split(","):
        for store in a.stores.split(","):
            log(f"=== {store} @ {scale}")
            try:
                Run(a, store, scale).execute()
            except NotQuiet as e:
                log(f"--- {store}/{scale} skipped: {e}")
                failures.append({"store": store, "scale": scale, "skipped": str(e)})
                with open(os.path.join(a.out, "failures.json"), "w") as f:
                    json.dump(failures, f, indent=1)
            except Exception as e:
                log(f"!!! {store}/{scale} failed: {e}")
                failures.append({"store": store, "scale": scale, "error": str(e)[:2000]})
                with open(os.path.join(a.out, "failures.json"), "w") as f:
                    json.dump(failures, f, indent=1)
    log(f"done; {len(failures)} failure(s)")


if __name__ == "__main__":
    main()
