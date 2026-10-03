#!/usr/bin/env python3
"""Cross-store SPARQL benchmark driver (standard library only).

Subcommands
-----------
  params    Draw the BSBM explore-mix query instances from a generated
            N-Triples file (seeded, so every store gets the same queries).
  client    The load generator. Runs inside a container on the store's Docker
            network (see campaign.py); never run two of these at once.
              --workload explore     single client: warm-up mixes, then measured mixes
              --workload features    SPARQL 1.1 feature queries, warm-up + fixed runs
              --workload concurrent  N client processes running explore mixes for S seconds
              --workload shacl       one store-specific SHACL validation call, repeated
              --workload load-http   POST an N-Triples file in line-aligned chunks
  report    Merge the per-store result files into CSV/JSON summaries and the
            Markdown tables used by docs/performance-comparison.md, with the
            result-count cross-check.

Every query record carries its latency, HTTP status, result count and a hash
of the query text, so the report can compare stores query by query.
"""
import argparse
import csv
import hashlib
import http.client
import json
import multiprocessing
import os
import random
import re
import statistics
import sys
import time
import urllib.parse
from collections import Counter, defaultdict

BSBM_INST = "http://www4.wiwiss.fu-berlin.de/bizer/bsbm/v01/instances/"
BSBM_VOC = "http://www4.wiwiss.fu-berlin.de/bizer/bsbm/v01/vocabulary/"
RDF_TYPE = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type"
SUBCLASS = "http://www.w3.org/2000/01/rdf-schema#subClassOf"
REV_REVIEW = "http://purl.org/stuff/rev#Review"
# BSBM's generator dates offers around this day; the explore mix filters
# offers still valid after it (the BSBM test driver uses the same anchor).
CURRENT_DATE = '"2008-06-20T00:00:00"^^<http://www.w3.org/2001/XMLSchema#dateTime>'
# The BSBM V3 explore query mix (queries/explore/querymix.txt): 25 queries.
EXPLORE_MIX = [1, 2, 2, 3, 2, 2, 4, 2, 2, 5, 7, 7, 5, 7, 7, 8, 9, 9, 8, 9, 9, 10, 10, 11, 12]

DISPLAY = {"ots": "Open Triplestore", "oxigraph": "Oxigraph", "fuseki": "Fuseki", "qlever": "QLever",
           "virtuoso": "Virtuoso", "rdf4j": "RDF4J"}

NT_LINE = re.compile(r'^<([^>]*)> <([^>]*)> (.*) \.\s*$')


def qhash(text):
    return hashlib.sha256(text.encode()).hexdigest()[:16]


# ---------------------------------------------------------------- params ---

def scan_dataset(path):
    """One pass over the N-Triples file; collect what the templates need."""
    product_types = defaultdict(set)  # product -> types (minus bsbm:Product)
    product_features = defaultdict(list)
    subclass = {}
    offers, reviews = [], []
    products = []
    with open(path, encoding="utf-8") as f:
        for line in f:
            m = NT_LINE.match(line)
            if not m:
                continue
            s, p, o = m.groups()
            if p == RDF_TYPE:
                if o == f"<{BSBM_VOC}Product>":
                    products.append(s)
                elif o == f"<{BSBM_VOC}Offer>":
                    offers.append(s)
                elif o == f"<{REV_REVIEW}>":
                    reviews.append(s)
                elif o.startswith(f"<{BSBM_INST}ProductType") and "/Product" in s:
                    product_types[s].add(o[1:-1])
            elif p == f"{BSBM_VOC}productFeature":
                product_features[s].append(o[1:-1])
            elif p == SUBCLASS:
                subclass[s] = o[1:-1]
    return products, product_types, product_features, subclass, offers, reviews


def load_templates(bsbm_dir):
    qdir = os.path.join(bsbm_dir, "queries", "explore")
    templates = {}
    for qid in sorted(set(EXPLORE_MIX)):
        with open(os.path.join(qdir, f"query{qid}.txt"), encoding="utf-8") as f:
            text = f.read()
        with open(os.path.join(qdir, f"query{qid}desc.txt"), encoding="utf-8") as f:
            desc = f.read()
        kind = "select"
        if "QueryType=Describe" in desc:
            kind = "describe"
        elif "QueryType=Construct" in desc:
            kind = "construct"
        templates[qid] = (text, kind)
    return templates


def cmd_params(a):
    rnd = random.Random(a.seed)
    products, ptypes, pfeat, subclass, offers, reviews = scan_dataset(a.data)
    products.sort()
    offers.sort()
    reviews.sort()
    all_features = sorted({f for fs in pfeat.values() for f in fs})
    templates = load_templates(a.bsbm)
    root = f"{BSBM_INST}ProductType1"

    def iri(x):
        return f"<{x}>"

    def pick_type_and_features(n):
        p = rnd.choice(products)
        types = sorted(t for t in ptypes.get(p, ()) if t != root) or [root]
        t = rnd.choice(types)
        feats = sorted(set(pfeat.get(p, [])))
        rnd.shuffle(feats)
        while len(feats) < n:
            feats.append(rnd.choice(all_features))
        return t, feats[:n]

    def instance(qid):
        text, kind = templates[qid]
        sub = {"%currentDate%": CURRENT_DATE}
        if qid in (1, 3, 4):
            t, feats = pick_type_and_features(3)
            sub["%ProductType%"] = iri(t)
            sub["%ProductFeature1%"] = iri(feats[0])
            sub["%ProductFeature2%"] = iri(feats[1])
            sub["%ProductFeature3%"] = iri(feats[2])
            sub["%x%"] = str(rnd.randint(1, 500))
            sub["%y%"] = str(rnd.randint(1, 500))
        if qid in (2, 5, 7, 8, 10):
            sub["%ProductXYZ%"] = iri(rnd.choice(products))
        if qid == 9:
            sub["%ReviewXYZ%"] = iri(rnd.choice(reviews))
        if qid in (11, 12):
            sub["%OfferXYZ%"] = iri(rnd.choice(offers))
        for k, v in sub.items():
            text = text.replace(k, v)
        if "%" in re.sub(r"%\d", "", text.split("WHERE")[-1]):
            leftover = re.findall(r"%[A-Za-z0-9]+%", text)
            if leftover:
                raise SystemExit(f"query {qid}: unfilled parameters {leftover}")
        return {"qid": f"Q{qid}", "kind": kind, "text": text, "hash": qhash(text)}

    def mixes(n):
        return [[instance(q) for q in EXPLORE_MIX] for _ in range(n)]

    out = {
        "dataset": os.path.basename(a.data),
        "seed": a.seed,
        "products": len(products),
        "offers": len(offers),
        "reviews": len(reviews),
        "warmup": mixes(a.warmup),
        "measured": mixes(a.mixes),
        "concurrent": mixes(a.concurrent_mixes),
    }
    with open(a.out, "w", encoding="utf-8") as f:
        json.dump(out, f)
    print(f"{a.out}: {len(products)} products, {a.warmup}+{a.mixes}+{a.concurrent_mixes} mixes", file=sys.stderr)


# ---------------------------------------------------------------- client ---

class Endpoint:
    """A persistent HTTP/1.1 connection to one SPARQL endpoint."""

    def __init__(self, url, headers=None, timeout=300):
        u = urllib.parse.urlsplit(url)
        self.host, self.port = u.hostname, u.port or 80
        self.path = u.path or "/"
        self.query_string = u.query
        self.headers = dict(headers or {})
        self.timeout = timeout
        self.conn = None

    def _connect(self):
        self.conn = http.client.HTTPConnection(self.host, self.port, timeout=self.timeout)

    def request(self, method, body, headers, path=None):
        """Return (status, content_type, body_bytes, seconds). Status -1 = transport error."""
        path = path or (self.path + (("?" + self.query_string) if self.query_string else ""))
        h = dict(self.headers)
        h.update(headers)
        for attempt in (0, 1):
            if self.conn is None:
                self._connect()
            t0 = time.perf_counter()
            try:
                self.conn.request(method, path, body=body, headers=h)
                r = self.conn.getresponse()
                data = r.read()
                dt = time.perf_counter() - t0
                if r.getheader("Connection", "").lower() == "close":
                    self.conn.close()
                    self.conn = None
                return r.status, r.getheader("Content-Type", ""), data, dt
            except (http.client.RemoteDisconnected, ConnectionResetError, BrokenPipeError) as e:
                self.conn.close()
                self.conn = None
                if attempt == 1:
                    return -1, "", str(e).encode(), time.perf_counter() - t0
            except Exception as e:  # timeouts and anything else
                dt = time.perf_counter() - t0
                try:
                    self.conn.close()
                finally:
                    self.conn = None
                return -1, "", f"{type(e).__name__}: {e}".encode(), dt
        return -1, "", b"unreachable", 0.0

    def sparql(self, text, kind):
        accept = ("application/sparql-results+json" if kind == "select"
                  else "application/n-triples, text/plain;q=0.9")
        body = urllib.parse.urlencode({"query": text}).encode()
        st, ctype, data, dt = self.request(
            "POST", body, {"Content-Type": "application/x-www-form-urlencoded", "Accept": accept})
        count, err = None, None
        if st == 200:
            count, err = count_results(data, kind, ctype)
        else:
            err = data[:300].decode("utf-8", "replace")
        return {"status": st, "seconds": dt, "count": count, "error": err, "bytes": len(data)}


def count_results(data, kind, ctype):
    """Count solutions (SELECT) or triples (CONSTRUCT/DESCRIBE); strict parsing.

    A body that does not parse is an error, never a count: QLever, for one,
    can append a plain-text error to a 200 response when export fails midway.
    """
    if kind == "select":
        try:
            doc = json.loads(data)
            return len(doc["results"]["bindings"]), None
        except Exception as e:
            return None, f"unparseable SELECT result ({type(e).__name__}): {data[-200:]!r}"
    text = data.decode("utf-8", "replace")
    ct = ctype.lower()
    if "n-triples" in ct or "ntriples" in ct or "text/plain" in ct or "n-quads" in ct:
        n = 0
        for line in text.splitlines():
            s = line.strip()
            if not s or s.startswith("#"):
                continue
            if not s.endswith("."):
                return None, f"malformed N-Triples line: {s[:120]!r}"
            n += 1
        return n, None
    return None, f"unexpected content type for {kind}: {ctype!r}"


def run_mix(ep, mix, phase, mix_index, client=0):
    recs = []
    for q in mix:
        r = ep.sparql(q["text"], q["kind"])
        r.update({"qid": q["qid"], "hash": q["hash"], "phase": phase,
                  "mix": mix_index, "client": client, "kind": q["kind"]})
        recs.append(r)
    return recs


def workload_explore(a, ep, params):
    recs = []
    t0 = time.perf_counter()
    for i, mix in enumerate(params["warmup"]):
        recs += run_mix(ep, mix, "warmup", i)
    warm_s = time.perf_counter() - t0
    mix_times = []
    for i, mix in enumerate(params["measured"]):
        m0 = time.perf_counter()
        recs += run_mix(ep, mix, "measured", i)
        mix_times.append(time.perf_counter() - m0)
    total = sum(mix_times)
    return {"records": recs, "warmup_seconds": warm_s, "mix_seconds": mix_times,
            "qmph": 3600.0 * len(mix_times) / total if total else None}


def _concurrent_worker(args):
    url, headers, timeout, mixes, client, seconds, start_at, out_path = args
    ep = Endpoint(url, headers, timeout)
    while time.time() < start_at:
        time.sleep(0.001)
    deadline = start_at + seconds
    recs, done, i = [], 0, 0
    while time.time() < deadline:
        mix = mixes[(client * 7 + i) % len(mixes)]
        mix_recs = run_mix(ep, mix, "concurrent", i, client)
        if time.time() <= deadline:  # count only mixes completed inside the window
            done += 1
            recs += mix_recs
        i += 1
    with open(out_path, "w", encoding="utf-8") as f:
        json.dump({"client": client, "mixes": done, "records": recs}, f)


def workload_concurrent(a, ep, params):
    levels = [int(x) for x in a.clients.split(",")]
    result = {"levels": []}
    for n in levels:
        start_at = time.time() + 2.0
        paths = [f"/tmp/conc-{n}-{c}.json" for c in range(n)]
        jobs = [(a.endpoint, ep.headers, a.timeout, params["concurrent"], c, a.seconds, start_at, paths[c])
                for c in range(n)]
        procs = [multiprocessing.Process(target=_concurrent_worker, args=(j,)) for j in jobs]
        for p in procs:
            p.start()
        for p in procs:
            p.join()
        recs, mixes = [], 0
        for pth in paths:
            with open(pth, encoding="utf-8") as f:
                d = json.load(f)
            mixes += d["mixes"]
            recs += d["records"]
            os.remove(pth)
        result["levels"].append({"clients": n, "seconds": a.seconds, "mixes": mixes,
                                 "qmph": 3600.0 * mixes / a.seconds, "records": recs})
        print(f"  {n} clients: {mixes} mixes in {a.seconds}s", file=sys.stderr)
        time.sleep(5)
    return result


def load_feature_queries(qdir):
    out = []
    for name in sorted(os.listdir(qdir)):
        if not name.endswith(".rq"):
            continue
        with open(os.path.join(qdir, name), encoding="utf-8") as f:
            text = f.read()
        kind = "construct" if re.search(r"^\s*CONSTRUCT", text, re.M | re.I) else "select"
        out.append({"qid": name[:-3], "kind": kind, "text": text, "hash": qhash(text)})
    return out


def workload_features(a, ep):
    """Warm-up runs, then a fixed number of measured runs per query.

    A query whose warm-up run fails or takes longer than --slow-seconds gets no
    further warm-up and a single measured run (a failure: none), so one
    pathological query cannot hold the machine for an hour; the report shows
    the run count next to every median.
    """
    recs = []
    for q in load_feature_queries(a.features):
        runs, failed = a.feature_runs, False
        for w in range(a.feature_warmup):
            r = ep.sparql(q["text"], q["kind"])
            r.update({"qid": q["qid"], "hash": q["hash"], "phase": "warmup", "kind": q["kind"]})
            recs.append(r)
            if r["status"] != 200:
                failed = True
                break
            if r["seconds"] > a.slow_seconds:
                runs = 1
                break
        for i in range(0 if failed else runs):
            r = ep.sparql(q["text"], q["kind"])
            r.update({"qid": q["qid"], "hash": q["hash"], "phase": "measured", "run": i, "kind": q["kind"]})
            recs.append(r)
        if failed:  # keep the failure visible in the measured set
            r = dict(recs[-1])
            r["phase"] = "measured"
            recs.append(r)
        print(f"  {q['qid']}: {0 if failed else runs} runs, last {recs[-1]['seconds']*1000:.1f} ms "
              f"status {recs[-1]['status']} count {recs[-1]['count']}", file=sys.stderr)
    return {"records": recs}


def workload_shacl(a, ep):
    with open(a.shapes, "rb") as f:
        shapes = f.read()
    recs = []
    for i in range(a.shacl_warmup + a.shacl_runs):
        phase = "warmup" if i < a.shacl_warmup else "measured"
        if a.shacl_kind == "fuseki":
            st, ctype, data, dt = ep.request(
                "POST", shapes, {"Content-Type": "text/turtle", "Accept": "application/n-triples"})
            count = None
            if st == 200:
                count = sum(1 for l in data.decode("utf-8", "replace").splitlines()
                            if "<http://www.w3.org/ns/shacl#result>" in l)
            extra = {}
        elif a.shacl_kind == "ots":
            body = json.dumps({"shapes_graph": a.shapes_graph} if a.shapes_graph else {}).encode()
            st, ctype, data, dt = ep.request("POST", body, {"Content-Type": "application/json"})
            count, extra = None, {}
            if st == 200:
                doc = json.loads(data)
                rep = doc.get("report") or {}
                count = rep.get("results_count")
                extra = {"metrics": rep.get("metrics"), "conforms": rep.get("conforms"),
                         "test_run": doc.get("test")}
        else:
            raise SystemExit(f"unknown --shacl-kind {a.shacl_kind}")
        rec = {"qid": "shacl", "phase": phase, "status": st, "seconds": dt, "count": count,
               "error": None if st == 200 else data[:300].decode("utf-8", "replace")}
        rec.update(extra)
        recs.append(rec)
        print(f"  shacl {phase}: {dt:.2f}s status {st} results {count}", file=sys.stderr)
    return {"records": recs}


def workload_load_http(a, ep):
    """POST the file in line-aligned chunks of at most --chunk-mb (sequential)."""
    limit = a.chunk_mb * 1024 * 1024
    chunks, t_all = [], time.perf_counter()
    with open(a.file, "rb") as f:
        buf, size = [], 0
        while True:
            line = f.readline()
            if line:
                buf.append(line)
                size += len(line)
            if (size >= limit or not line) and buf:
                body = b"".join(buf)
                st, _, data, dt = ep.request("POST", body, {"Content-Type": a.content_type})
                chunks.append({"bytes": len(body), "status": st, "seconds": dt,
                               "error": None if 200 <= st < 300 else data[:300].decode("utf-8", "replace")})
                print(f"  chunk {len(chunks)}: {len(body)/1e6:.0f} MB in {dt:.1f}s HTTP {st}", file=sys.stderr)
                if not 200 <= st < 300:
                    break
                buf, size = [], 0
            if not line:
                break
    return {"chunks": chunks, "seconds": time.perf_counter() - t_all,
            "ok": all(200 <= c["status"] < 300 for c in chunks)}


def cmd_client(a):
    headers = {}
    if a.token:
        headers["Authorization"] = f"Bearer {a.token}"
    for h in a.header or []:
        k, v = h.split(":", 1)
        headers[k.strip()] = v.strip()
    ep = Endpoint(a.endpoint, headers, a.timeout)
    params = None
    if a.workload in ("explore", "concurrent"):
        with open(a.params, encoding="utf-8") as f:
            params = json.load(f)
    t0 = time.time()
    if a.workload == "explore":
        res = workload_explore(a, ep, params)
    elif a.workload == "concurrent":
        res = workload_concurrent(a, ep, params)
    elif a.workload == "features":
        res = workload_features(a, ep)
    elif a.workload == "shacl":
        res = workload_shacl(a, ep)
    elif a.workload == "load-http":
        res = workload_load_http(a, ep)
    else:
        raise SystemExit(f"unknown workload {a.workload}")
    res.update({"workload": a.workload, "store": a.store, "started": t0, "finished": time.time()})
    with open(a.out, "w", encoding="utf-8") as f:
        json.dump(res, f)


# ---------------------------------------------------------------- report ---

def pct(values, p):
    if not values:
        return None
    v = sorted(values)
    k = (len(v) - 1) * p / 100.0
    lo, hi = int(k), min(int(k) + 1, len(v) - 1)
    return v[lo] + (v[hi] - v[lo]) * (k - lo)


def fmt_ms(s):
    if s is None:
        return "—"
    ms = s * 1000
    if ms >= 10000:
        return f"{ms/1000:.1f} s"
    if ms >= 100:
        return f"{ms:.0f}"
    if ms >= 10:
        return f"{ms:.1f}"
    return f"{ms:.2f}"


def cmd_report(a):
    """Merge <results>/<store>/<scale>/*.json into summaries and Markdown."""
    root = a.results
    runs = {}  # (scale, store) -> {workload: data, "meta": ...}
    for store in sorted(os.listdir(root)):
        sdir = os.path.join(root, store)
        if not os.path.isdir(sdir):
            continue
        for scale in sorted(os.listdir(sdir)):
            d = os.path.join(sdir, scale)
            if not os.path.isdir(d):
                continue
            entry = {}
            for name in os.listdir(d):
                if name.endswith(".json"):
                    with open(os.path.join(d, name), encoding="utf-8") as f:
                        entry[name[:-5]] = json.load(f)
            runs[(scale, store)] = entry
    scales = sorted({s for s, _ in runs}, key=lambda s: (len(s), s))
    stores_order = a.stores.split(",") if a.stores else sorted({st for _, st in runs})

    # The cross-check, per query instance (hash of the query text):
    #  * a store that gave two different counts for the same instance in
    #    different runs is flagged "inconsistent" (a partial or unstable answer);
    #  * otherwise each store's count is compared with the majority: the count
    #    a strict majority of the stores that answered gave, when at least three
    #    answered. A store that differs from it is flagged "differs".
    #  * DESCRIBE is implementation-defined (SPARQL 1.1 §16.4), so its
    #    differences are listed apart and are not treated as wrong answers.
    def records_of(entry):
        for wl in ("explore", "features", "concurrent"):
            data = entry.get(wl)
            if not data:
                continue
            if wl == "concurrent":
                for lvl in data["levels"]:
                    yield from ((f"concurrent-{lvl['clients']}", r) for r in lvl["records"])
            else:
                yield from ((wl, r) for r in data["records"])

    summary = {"scales": {}, "flags": []}
    csv_rows = []
    for scale in scales:
        counts = defaultdict(lambda: defaultdict(Counter))  # hash -> store -> Counter(count)
        where = defaultdict(lambda: defaultdict(set))  # hash -> store -> {workloads}
        qid_of, kind_of = {}, {}
        for (sc, st), entry in runs.items():
            if sc != scale:
                continue
            for wl, r in records_of(entry):
                if "hash" not in r:
                    continue
                qid_of[r["hash"]] = r["qid"]
                kind_of[r["hash"]] = r.get("kind", "select")
                if r.get("status") == 200 and r.get("count") is not None:
                    counts[r["hash"]][st][r["count"]] += 1
                    where[r["hash"]][st].add(wl)
        agg = defaultdict(lambda: {"instances": 0, "examples": []})
        for h, per_store in counts.items():
            describe = kind_of.get(h) == "describe"
            modal = {}
            for st, cs in per_store.items():
                if len(cs) > 1:
                    key = (st, qid_of[h], "inconsistent")
                    agg[key]["instances"] += 1
                    if len(agg[key]["examples"]) < 3:
                        agg[key]["examples"].append({"hash": h, "counts": dict(cs),
                                                     "workloads": sorted(where[h][st])})
                modal[st] = cs.most_common(1)[0][0]
            if len(modal) < 3:
                continue
            votes = Counter(modal.values())
            ref, n = votes.most_common(1)[0]
            if n * 2 <= len(modal):
                continue  # no strict majority
            for st, c in modal.items():
                if c != ref:
                    key = (st, qid_of[h], "describe-differs" if describe else "differs")
                    agg[key]["instances"] += 1
                    if len(agg[key]["examples"]) < 3:
                        agg[key]["examples"].append({"hash": h, "count": c, "majority": ref})
        for (st, qid, kind), v in sorted(agg.items()):
            summary["flags"].append({"scale": scale, "store": st, "query": qid, "kind": kind,
                                     "instances": v["instances"], "examples": v["examples"]})

        sc_sum = {}
        for st in stores_order:
            entry = runs.get((scale, st))
            if not entry:
                continue
            s = {"meta": entry.get("meta", {})}
            ex = entry.get("explore")
            if ex:
                per_q = defaultdict(list)
                errors = Counter()
                for r in ex["records"]:
                    if r["phase"] != "measured":
                        continue
                    if r["status"] == 200 and r["count"] is not None:
                        per_q[r["qid"]].append(r["seconds"])
                    else:
                        errors[r["qid"]] += 1
                s["explore"] = {
                    "qmph": ex.get("qmph"),
                    "mixes": len(ex.get("mix_seconds", [])),
                    "per_query": {q: {"median": statistics.median(v), "p95": pct(v, 95), "n": len(v)}
                                  for q, v in per_q.items()},
                    "errors": dict(errors),
                }
                for q, v in per_q.items():
                    csv_rows.append([scale, st, "explore", q, len(v), statistics.median(v), pct(v, 95), errors.get(q, 0)])
            ft = entry.get("features")
            if ft:
                per_q = defaultdict(list)
                errors = {}
                for r in ft["records"]:
                    if r["phase"] != "measured":
                        continue
                    if r["status"] == 200 and r["count"] is not None:
                        per_q[r["qid"]].append(r["seconds"])
                    else:
                        errors[r["qid"]] = (r.get("error") or "")[:160] or f"HTTP {r['status']}"
                s["features"] = {"per_query": {q: {"median": statistics.median(v), "p95": pct(v, 95), "n": len(v)}
                                               for q, v in per_q.items()},
                                 "errors": errors}
                for q, v in per_q.items():
                    csv_rows.append([scale, st, "features", q, len(v), statistics.median(v), pct(v, 95), 0])
                for q in errors:
                    csv_rows.append([scale, st, "features", q, 0, None, None, 1])
            cc = entry.get("concurrent")
            if cc:
                s["concurrent"] = []
                for lvl in cc["levels"]:
                    lat = [r["seconds"] for r in lvl["records"] if r["status"] == 200]
                    err = sum(1 for r in lvl["records"] if r["status"] != 200)
                    s["concurrent"].append({"clients": lvl["clients"], "qmph": lvl["qmph"], "mixes": lvl["mixes"],
                                            "median": statistics.median(lat) if lat else None,
                                            "p95": pct(lat, 95), "errors": err})
                    csv_rows.append([scale, st, f"concurrent-{lvl['clients']}", "mix", len(lat),
                                     statistics.median(lat) if lat else None, pct(lat, 95), err])
            sh = entry.get("shacl")
            if sh:
                t = [r["seconds"] for r in sh["records"] if r["phase"] == "measured" and r["status"] == 200]
                cnt = {r["count"] for r in sh["records"] if r["status"] == 200}
                s["shacl"] = {"median": statistics.median(t) if t else None, "p95": pct(t, 95), "n": len(t),
                              "results": sorted(c for c in cnt if c is not None)}
                csv_rows.append([scale, st, "shacl", "validate", len(t), s["shacl"]["median"], s["shacl"]["p95"], 0])
            sc_sum[st] = s
        summary["scales"][scale] = sc_sum

    os.makedirs(a.out, exist_ok=True)
    with open(os.path.join(a.out, "summary.json"), "w", encoding="utf-8") as f:
        json.dump(summary, f, indent=1, sort_keys=True)
    with open(os.path.join(a.out, "latencies.csv"), "w", newline="", encoding="utf-8") as f:
        w = csv.writer(f)
        w.writerow(["scale", "store", "workload", "query", "runs", "median_s", "p95_s", "errors"])
        w.writerows(csv_rows)
    with open(os.path.join(a.out, "crosscheck.csv"), "w", newline="", encoding="utf-8") as f:
        w = csv.writer(f)
        w.writerow(["scale", "store", "query", "kind", "instances"])
        for fl in summary["flags"]:
            w.writerow([fl["scale"], fl["store"], fl["query"], fl["kind"], fl["instances"]])
    with open(os.path.join(a.out, "tables.md"), "w", encoding="utf-8") as f:
        f.write(markdown_tables(summary, scales, stores_order, a.triples))
    for sc in scales:
        vals = [(st, summary["scales"][sc][st].get("explore", {}).get("qmph"))
                for st in stores_order if st in summary["scales"][sc]]
        with open(os.path.join(a.out, f"explore-qmph-{sc}.svg"), "w", encoding="utf-8") as f:
            f.write(bar_chart_svg(f"BSBM explore, one client, {sc} products: query mixes per hour",
                                  [(DISPLAY.get(st, st), v) for st, v in vals if v]))
    print(f"wrote {a.out}/summary.json, latencies.csv, crosscheck.csv, tables.md", file=sys.stderr)


def bar_chart_svg(title, values, width=640):
    """A horizontal bar chart (one series) as standalone SVG, light and dark."""
    from html import escape
    bar_h, gap, left, right, top = 20, 12, 128, 80, 44
    height = top + len(values) * (bar_h + gap) + 8
    vmax = max(v for _, v in values)
    plot_w = width - left - right
    out = [f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {width} {height}" width="{width}" '
           f'height="{height}" role="img" aria-label="{escape(title)}">',
           "<style>.bg{fill:#fcfcfb}.bar{fill:#2a78d6}.t1{fill:#0b0b0b}.t2{fill:#52514e}"
           "text{font:13px -apple-system,BlinkMacSystemFont,'Segoe UI',Helvetica,Arial,sans-serif}"
           ".ttl{font-weight:600}.axis{stroke:#d9d8d4}"
           "@media (prefers-color-scheme: dark){.bg{fill:#1a1a19}.bar{fill:#3987e5}.t1{fill:#fff}"
           ".t2{fill:#c3c2b7}.axis{stroke:#3a3a37}}</style>",
           f'<rect class="bg" width="{width}" height="{height}" rx="6"/>',
           f'<text class="t1 ttl" x="16" y="26">{escape(title)}</text>',
           f'<line class="axis" x1="{left}" x2="{left}" y1="{top - 6}" y2="{height - 8}"/>']
    for i, (name, v) in enumerate(values):
        y = top + i * (bar_h + gap)
        w = max(2.0, plot_w * v / vmax)
        r = min(4, w / 2)
        path = (f"M{left},{y} H{left + w - r} Q{left + w},{y} {left + w},{y + r} V{y + bar_h - r} "
                f"Q{left + w},{y + bar_h} {left + w - r},{y + bar_h} H{left} Z")
        out.append(f'<g><title>{escape(name)}: {v:,.0f} QMpH</title><path class="bar" d="{path}"/>'
                   f'<text class="t2" x="{left - 8}" y="{y + 15}" text-anchor="end">{escape(name)}</text>'
                   f'<text class="t1" x="{left + w + 6}" y="{y + 15}">{v:,.0f}</text></g>')
    out.append("</svg>")
    return "\n".join(out) + "\n"


def markdown_tables(summary, scales, stores, triples_arg):
    """The Markdown tables of docs/performance-comparison.md, from the summary."""
    triples = dict(t.split("=") for t in triples_arg.split(",")) if triples_arg else {}
    out = []

    def row(cells):
        out.append("| " + " | ".join(cells) + " |")

    def head(cols, align=None):
        row(cols)
        row(align or ["---"] + ["--:"] * (len(cols) - 1))

    def mib(b):
        return "—" if not b else f"{b / 2**20:,.0f}"

    present = lambda sc: [st for st in stores if st in summary["scales"].get(sc, {})]
    name = lambda st: DISPLAY.get(st, st)

    out.append("### Load\n")
    head(["Store", "Scale", "Load path", "Load time (s)", "Triples/s", "Peak memory (MiB)", "Store size (MiB)",
          "HTTP POST load (s)"], ["---", "---", "---", "--:", "--:", "--:", "--:", "--:"])
    for sc in scales:
        for st in present(sc):
            m = summary["scales"][sc][st]["meta"]
            ph = m.get("phases", {})
            ld = ph.get("load", {})
            secs = ld.get("seconds")
            n = int(triples.get(sc, 0))
            rate = f"{n / secs:,.0f}" if secs and n else "—"
            http_s = ph.get("load_http", {}).get("seconds")
            row([name(st), sc, m.get("load_method", ""), f"{secs:.1f}" if secs else "—", rate, mib(ld.get("peak_mem_bytes")),
                 mib(m.get("db_bytes_after_load")), f"{http_s:.1f}" if http_s else "—"])
    out.append("")

    for sc in scales:
        sts = present(sc)
        out.append(f"### BSBM explore, single client — {sc}\n")
        head(["Query"] + [name(st) for st in sts])
        qids = sorted({q for st in sts for q in summary["scales"][sc][st].get("explore", {}).get("per_query", {})},
                      key=lambda q: int(q[1:]))
        for q in qids:
            cells = [q]
            for st in sts:
                pq = summary["scales"][sc][st].get("explore", {}).get("per_query", {}).get(q)
                err = summary["scales"][sc][st].get("explore", {}).get("errors", {}).get(q)
                if pq:
                    c = f"{fmt_ms(pq['median'])} / {fmt_ms(pq['p95'])}"
                    if err:
                        c += f" ({err} failed)"
                else:
                    c = f"failed ×{err}" if err else "—"
                cells.append(c)
            row(cells)
        cells = ["**QMpH**"]
        for st in sts:
            q = summary["scales"][sc][st].get("explore", {}).get("qmph")
            cells.append(f"**{q:,.0f}**" if q else "—")
        row(cells)
        out.append("")
        out.append("Cells: median / p95 latency in ms (100 measured mixes). QMpH: explore query mixes per hour.\n")

    for sc in scales:
        sts = present(sc)
        out.append(f"### SPARQL 1.1 feature mix — {sc}\n")
        head(["Query"] + [name(st) for st in sts])
        qids = sorted({q for st in sts for q in list(summary["scales"][sc][st].get("features", {}).get("per_query", {}))
                       + list(summary["scales"][sc][st].get("features", {}).get("errors", {}))})
        for q in qids:
            cells = [q]
            for st in sts:
                ft = summary["scales"][sc][st].get("features", {})
                pq = ft.get("per_query", {}).get(q)
                if pq:
                    c = fmt_ms(pq["median"])
                    if pq["n"] < 10:
                        c += f" (n={pq['n']})"
                elif q in ft.get("errors", {}):
                    e = ft["errors"][q].lower()
                    c = "timeout" if "timeout" in e or "timed out" in e else "error"
                else:
                    c = "—"
                cells.append(c)
            row(cells)
        out.append("")
        out.append("Cells: median latency in ms of 10 runs after 2 warm-up runs (n = fewer runs, see method).\n")

    for sc in scales:
        sts = present(sc)
        out.append(f"### Concurrent clients — {sc}\n")
        head(["Store", "QMpH, 1 client", "QMpH, 4 clients", "QMpH, 8 clients", "p95 ms, 1", "p95 ms, 4", "p95 ms, 8",
              "Failed queries"])
        for st in sts:
            lv = {l["clients"]: l for l in summary["scales"][sc][st].get("concurrent", [])}
            if not lv:
                continue
            row([name(st)] + [f"{lv[c]['qmph']:,.0f}" if c in lv else "—" for c in (1, 4, 8)]
                + [fmt_ms(lv[c]["p95"]) if c in lv else "—" for c in (1, 4, 8)]
                + [str(sum(l["errors"] for l in lv.values()))])
        out.append("")

    out.append("### SHACL validation\n")
    head(["Store", "Scale", "Median (s)", "p95 (s)", "Runs", "Results reported"])
    for sc in scales:
        for st in present(sc):
            shc = summary["scales"][sc][st].get("shacl")
            if shc and shc["median"] is not None:
                row([name(st), sc, f"{shc['median']:.2f}", f"{shc['p95']:.2f}", str(shc["n"]),
                     ", ".join(f"{c:,}" for c in shc["results"])])
    out.append("")

    out.append("### Peak container memory per phase (MiB)\n")
    phases = ["load", "explore", "features", "concurrent", "shacl"]
    head(["Store", "Scale"] + phases)
    for sc in scales:
        for st in present(sc):
            ph = summary["scales"][sc][st]["meta"].get("phases", {})
            row([name(st), sc] + [mib(ph.get(p, {}).get("peak_mem_bytes")) for p in phases])
    out.append("")

    out.append("### Result-count cross-check\n")
    if summary["flags"]:
        head(["Scale", "Store", "Query", "Finding", "Query instances"], ["---", "---", "---", "---", "--:"])
        for fl in summary["flags"]:
            row([fl["scale"], name(fl["store"]), fl["query"], fl["kind"], str(fl["instances"])])
    else:
        out.append("No store returned a different result count for any query instance.")
    out.append("")
    return "\n".join(out) + "\n"


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)

    p = sub.add_parser("params")
    p.add_argument("--data", required=True)
    p.add_argument("--bsbm", required=True, help="unpacked bsbmtools directory (query templates)")
    p.add_argument("--out", required=True)
    p.add_argument("--seed", type=int, default=20261003)
    p.add_argument("--warmup", type=int, default=20)
    p.add_argument("--mixes", type=int, default=100)
    p.add_argument("--concurrent-mixes", type=int, default=200)

    c = sub.add_parser("client")
    c.add_argument("--store", required=True)
    c.add_argument("--endpoint", required=True)
    c.add_argument("--workload", required=True)
    c.add_argument("--out", required=True)
    c.add_argument("--params")
    c.add_argument("--token")
    c.add_argument("--header", action="append")
    c.add_argument("--timeout", type=float, default=300)
    c.add_argument("--clients", default="1,4,8")
    c.add_argument("--seconds", type=int, default=60)
    c.add_argument("--features")
    c.add_argument("--feature-warmup", type=int, default=2)
    c.add_argument("--feature-runs", type=int, default=10)
    c.add_argument("--slow-seconds", type=float, default=30.0)
    c.add_argument("--shapes")
    c.add_argument("--shacl-kind")
    c.add_argument("--shapes-graph", help="OTS: the shapes graph IRI to validate against")
    c.add_argument("--shacl-warmup", type=int, default=1)
    c.add_argument("--shacl-runs", type=int, default=5)
    c.add_argument("--file")
    c.add_argument("--chunk-mb", type=int, default=100)
    c.add_argument("--content-type", default="application/n-triples")

    r = sub.add_parser("report")
    r.add_argument("--results", required=True)
    r.add_argument("--out", required=True)
    r.add_argument("--stores", default="")
    r.add_argument("--triples", default="", help="scale=triples pairs, e.g. 1k=374911,10k=3564773")

    a = ap.parse_args()
    {"params": cmd_params, "client": cmd_client, "report": cmd_report}[a.cmd](a)


if __name__ == "__main__":
    main()
