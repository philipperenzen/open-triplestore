#!/usr/bin/env python3
"""Like-for-like HTTP benchmark of a SPARQL server on the OTL-shaped dataset.

Runs the same phases against any SPARQL 1.1 Protocol + Graph Store endpoint:

  1. load    — PUT the Turtle file into the named graph (Graph Store Protocol)
  2. queries — six shapes, median of 5 timed runs after a warm-up
  3. mixed   — W writer threads POSTing INSERT DATA batches of 500 quads next
               to R reader threads doing single-asset lookups, for S seconds

Usage:
  scale_compare_http.py --name fuseki --sparql http://127.0.0.1:3031/otl/sparql \
      --update http://127.0.0.1:3031/otl/update --gsp http://127.0.0.1:3031/otl/data \
      --file otl-100k.ttl [--token …] [--writers 4 --readers 4 --seconds 20]

Prints one JSON document. Query results are counted, not parsed for
content; the result cache of the server under test must be off.
"""
import argparse, json, statistics, sys, threading, time, urllib.parse, urllib.request

EX = "https://example.org/otl/"
G = "https://example.org/otl/instances"


def req(url, data=None, headers=None, method=None, timeout=3600):
    h = dict(headers or {})
    r = urllib.request.Request(url, data=data, headers=h, method=method)
    with urllib.request.urlopen(r, timeout=timeout) as resp:
        return resp.status, resp.read()


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--name", required=True)
    ap.add_argument("--sparql", required=True)
    ap.add_argument("--update", required=True)
    ap.add_argument("--gsp", required=True)
    ap.add_argument("--file", required=True)
    ap.add_argument("--token")
    ap.add_argument("--writers", type=int, default=4)
    ap.add_argument("--readers", type=int, default=4)
    ap.add_argument("--seconds", type=int, default=20)
    ap.add_argument("--skip-load", action="store_true")
    a = ap.parse_args()
    auth = {"Authorization": f"Bearer {a.token}"} if a.token else {}

    def query(q, accept="application/sparql-results+json"):
        body = urllib.parse.urlencode({"query": q}).encode()
        st, out = req(a.sparql, body, {**auth, "Content-Type": "application/x-www-form-urlencoded", "Accept": accept}, "POST")
        return out

    def rows(out):
        try:
            return len(json.loads(out)["results"]["bindings"])
        except Exception:
            return -1

    result = {"store": a.name, "file": a.file}

    # 1. load
    if not a.skip_load:
        # curl streams the file and reads the reply even when the server
        # answers before the upload ends (urllib reports a broken pipe then).
        import subprocess
        cmd = ["curl", "-s", "-o", "/dev/null", "-w", "%{http_code}", "-X", "PUT", "-H", "Content-Type: text/turtle",
               "--data-binary", f"@{a.file}", f"{a.gsp}?graph={urllib.parse.quote(G, safe='')}"]
        if a.token:
            cmd[1:1] = ["-H", f"Authorization: Bearer {a.token}"]
        t0 = time.perf_counter()
        st = subprocess.run(cmd, capture_output=True, text=True, check=False).stdout.strip()
        load_s = time.perf_counter() - t0
        result["load_status"] = st
        if st not in ("200", "201", "204"):
            print(f"load failed: HTTP {st}", file=sys.stderr)
            sys.exit(2)
    else:
        load_s = None
    n = int(json.loads(query(f"SELECT (COUNT(*) AS ?c) WHERE {{ GRAPH <{G}> {{ ?s ?p ?o }} }}"))["results"]["bindings"][0]["c"]["value"])
    result["quads"] = n
    if load_s is not None:
        result["load"] = {"seconds": round(load_s, 2), "quads_per_s": round(n / load_s)}
    assets = n // 9
    probe = assets // 2

    # 2. queries
    qs = [
        ("lookup", f"SELECT ?p ?o WHERE {{ GRAPH <{G}> {{ <{EX}asset{probe}> ?p ?o }} }}"),
        ("join_2way", f"SELECT ?a ?pn WHERE {{ GRAPH <{G}> {{ ?a <{EX}partOf> ?p . ?p <{EX}name> ?pn }} }} LIMIT 10000"),
        ("filter", f"SELECT (COUNT(*) AS ?c) WHERE {{ GRAPH <{G}> {{ ?a <{EX}length> ?l FILTER(?l > 40.0 && ?l < 42.0) }} }}"),
        ("group_by", f"SELECT ?t (COUNT(?a) AS ?c) (AVG(?l) AS ?avg) WHERE {{ GRAPH <{G}> {{ ?a a ?t ; <{EX}length> ?l }} }} GROUP BY ?t"),
        ("path", f"SELECT (COUNT(?anc) AS ?c) WHERE {{ GRAPH <{G}> {{ <{EX}asset{probe}> <{EX}partOf>+ ?anc }} }}"),
        ("count_all", f"SELECT (COUNT(*) AS ?c) WHERE {{ GRAPH <{G}> {{ ?s ?p ?o }} }}"),
    ]
    result["queries"] = {}
    for name, q in qs:
        out = query(q)  # warm-up
        ms = []
        for _ in range(5):
            t0 = time.perf_counter()
            out = query(q)
            ms.append((time.perf_counter() - t0) * 1000)
        result["queries"][name] = {"median_ms": round(statistics.median(ms), 2), "rows": rows(out)}
        print(f"  {name}: {result['queries'][name]}", file=sys.stderr)

    # 3. mixed
    stop = threading.Event()
    wl, rl, werr, rerr = [], [], [0], [0]
    lock = threading.Lock()

    def batch(from_id, count):
        lines = []
        for i in range(from_id, from_id + count):
            t = i % 40
            lines.append(
                f'<{EX}asset{i}> a <{EX}Type{t}>, <{EX}Asset> ; <{EX}name> "Asset {i}" ; <{EX}code> "AB-{i}" ; '
                f'<{EX}length> "{(i % 500) / 10 + 1}"^^<http://www.w3.org/2001/XMLSchema#decimal> ; '
                f'<{EX}installed> "{1950 + i % 76}"^^<http://www.w3.org/2001/XMLSchema#gYear> ; '
                f'<{EX}status> "planned" ; <{EX}location> "POINT(4.5 51.5)" ; <{EX}partOf> <{EX}asset{i // 10}> .'
            )
        return f"INSERT DATA {{ GRAPH <{G}> {{ {' '.join(lines)} }} }}"

    def writer(w):
        i = 0
        while not stop.is_set():
            upd = batch(10_000_000 + w * 1_000_000 + i * 50, 50)
            t0 = time.perf_counter()
            try:
                req(a.update, upd.encode(), {**auth, "Content-Type": "application/sparql-update"}, "POST")
                with lock:
                    wl.append((time.perf_counter() - t0) * 1000)
            except Exception:
                with lock:
                    werr[0] += 1
            i += 1

    def reader(r):
        i = r
        while not stop.is_set():
            q = f"SELECT ?p ?o WHERE {{ GRAPH <{G}> {{ <{EX}asset{(i * 7919) % max(assets, 1)}> ?p ?o }} }}"
            t0 = time.perf_counter()
            try:
                query(q)
                with lock:
                    rl.append((time.perf_counter() - t0) * 1000)
            except Exception:
                with lock:
                    rerr[0] += 1
            i += 1

    threads = [threading.Thread(target=writer, args=(w,)) for w in range(a.writers)] + [threading.Thread(target=reader, args=(r,)) for r in range(a.readers)]
    for t in threads:
        t.start()
    time.sleep(a.seconds)
    stop.set()
    for t in threads:
        t.join()

    def p95(v):
        return round(sorted(v)[int(len(v) * 0.95) % len(v)], 2) if v else None

    result["mixed"] = {
        "seconds": a.seconds, "writers": a.writers, "readers": a.readers,
        "batches_written": len(wl), "quads_written": len(wl) * 500, "quads_written_per_s": round(len(wl) * 500 / a.seconds),
        "write_p95_ms": p95(wl), "write_errors": werr[0],
        "reads": len(rl), "reads_per_s": round(len(rl) / a.seconds), "read_p95_ms": p95(rl), "read_errors": rerr[0],
    }
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
