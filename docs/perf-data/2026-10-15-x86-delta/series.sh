#!/bin/bash
# The delta-versions measurements of docs/notes/delta-versioning-design.md
# §8.2, as run for docs/performance.md ("Delta versions at scale",
# 2026-10-15). Runs inside the builder image with the release binaries of
# `examples/scale_delta.rs` and `examples/scale_otl.rs`, a named volume at
# /data for the stores, and this worktree at /w/536:
#
#   docker run --rm -v <data volume>:/data -v "$PWD:/w/536" -w /w/536 <builder image> \
#     bash docs/perf-data/2026-10-15-x86-delta/series.sh <series>
#
# Series: smoke, host, load, a (items 1, 2, 4, 9), b (items 1, 5, 9), c
# (items 2, 5, 6, 9, 10, 11), d (items 3, 6, 7: the rest of c). Each step writes raw/<step>.json (the
# harness's JSON) and raw/<step>.log (its progress) next to this script.
# Nothing else may run on the machine meanwhile.
#
# What ran differently from this file: in series b, the transaction-memory
# steps (txrss-*) were taken with a resident-set baseline that did not trim
# the heap first, so memory an earlier step had freed was reused unseen;
# they are kept in raw/superseded/ and series c re-takes them with the
# harness's trimmed baseline. restore09-k100000 of series b stopped at its
# second tier-1 run (raw/superseded/): the first tier-1 restore, 300 020
# quads, was over the payload cap, its row was `counts`, and the harness then
# required an intact chain; series c re-runs it with the harness fixed.
# restore9m-k10000 was stopped by hand during its tier-2 MOVE, when the
# container passed 36 GiB resident on a shared host (raw/restore9m-k10000.log
# has the cut and tier-1 figures; there is no JSON).
set -u
B=${B:-/w/536/target/ots536-bin/scale_delta}
O=${O:-/w/536/target/ots536-bin/scale_otl}
R=/w/536/docs/perf-data/2026-10-15-x86-delta/raw
mkdir -p "$R"
export OTS_QUERY_CACHE_ENABLED=false OTS_PARALLEL_QUERY_ENABLED=false

run() {
    local name=$1
    shift
    echo "== $(date -Is) $name: $*" >&2
    local t0=$SECONDS
    "$@" >"$R/$name.json" 2>"$R/$name.log"
    local rc=$?
    echo "   exit $rc after $((SECONDS - t0))s" >&2
    return $rc
}

# A private copy of a loaded base store, so no measurement sees another's writes.
fresh() {
    rm -rf /data/run
    cp -a "/data/$1" /data/run
}

empty() {
    rm -rf /data/run
    mkdir -p /data/run
}

case "${1:-}" in
smoke)
    # Every command once on a tiny store, so a long series does not fail late.
    R=/tmp/smoke
    mkdir -p $R
    S=/data/smoke
    rm -rf $S /data/smoke2 /data/smoke3 /data/smoke-crash /data/smoke-crash-logs
    export OTS_CHANGE_CAPTURE_ENABLED=on
    set -e
    run s-load $B load $S data 0 2000 1000 --with-model
    run s-load2 $B load $S other 20000000 2000
    run s-scan $B scan $S data 0 2000 1
    run s-probe $B probe $S 18000 1
    run s-put $B put $S 2000 1
    run s-payload $B payload $S 1 9000
    run s-bulk $B bulk $S 2000 on
    run s-txrss $B txrss $S 9000 atomic on
    run s-restore $B restore $S data 0 2000 100 1
    run s-cold $B coldread $S data 0 2000 other 20000000 2000 100 1
    run s-census $B census /w/536/examples/seed-bundles
    mkdir -p /data/smoke2 /data/smoke3
    run s-annot $B annotations /data/smoke2 2000 1000 minimal
    run s-ldes $B ldes /data/smoke3 500 3 10 1
    run s-crash $B crash /data/smoke-crash 2 10
    set +e
    for f in $R/*.json; do python3 -c "import json,sys; json.load(open(sys.argv[1]))" "$f" || echo "BAD JSON $f"; done
    grep -h -A3 '"violations"' $R/s-crash.json | head -8
    rm -rf $S /data/smoke2 /data/smoke3 /data/smoke-crash /data/smoke-crash-logs
    echo SMOKE_OK
    ;;
host)
    {
        echo "{"
        echo "  \"cpu\": \"$(grep -m1 'model name' /proc/cpuinfo | cut -d: -f2 | sed 's/^ //')\","
        echo "  \"nproc\": $(nproc),"
        echo "  \"mem_total_kib\": $(grep MemTotal /proc/meminfo | awk '{print $2}'),"
        echo "  \"kernel\": \"$(uname -r)\","
        echo "  \"os_in_container\": \"$(. /etc/os-release && echo "$PRETTY_NAME")\","
        echo "  \"rustc\": \"$(rustc --version 2>/dev/null)\","
        echo "  \"data_volume_fs\": \"$(df -T /data | tail -1 | awk '{print $2}')\""
        echo "}"
    } >"$R/host-container.json"
    cat "$R/host-container.json"
    ;;
load)
    # Bases are loaded with capture off: a measurement opens its copy with
    # capture on, which starts a fresh change log there.
    export OTS_CHANGE_CAPTURE_ENABLED=off
    rm -rf /data/base09 /data/base17 /data/base9m
    run load-base09-data $B load /data/base09 data 0 100000 50000 --with-model
    run load-base09-s50 $B load /data/base09 https://example.org/otl/s50 10000000 5556
    run load-base09-s250 $B load /data/base09 https://example.org/otl/s250 11000000 27778
    run load-base17-data $B load /data/base17 data 0 100000 50000 --with-model
    run load-base17-other $B load /data/base17 other 20000000 88889
    run load-base9m-data $B load /data/base9m data 0 1000000 50000 --with-model
    run load-base9m-v09 $B load /data/base9m https://example.org/otl/v09 20000000 100000
    du -sh /data/base09 /data/base17 /data/base9m >&2
    ;;
a)
    export OTS_CHANGE_CAPTURE_ENABLED=on
    # Item 1: the before-image of a WHERE update, at 50k / 250k / 900k quads,
    # under the default scan cap and with the cap lifted.
    for cap in 250000 20000000; do
        fresh base09
        export OTS_CHANGE_CAPTURE_MAX_SCAN=$cap
        run scan-s50-cap$cap $B scan /data/run https://example.org/otl/s50 10000000 5556 5
        run scan-s250-cap$cap $B scan /data/run https://example.org/otl/s250 11000000 27778 5
        run scan-data09-cap$cap $B scan /data/run data 0 100000 5
    done
    unset OTS_CHANGE_CAPTURE_MAX_SCAN
    # Item 2: probes, a 1M-quad payload.
    fresh base09
    run probe-1m $B probe /data/run 1000000 3
    # Item 4: the PUT-replace pre-image, 900k quads replaced by 900k (1 % changed).
    fresh base09
    run put09-capdefault $B put /data/run 100000 3
    fresh base09
    OTS_CHANGE_CAPTURE_MAX_SCAN=20000000 run put09-scanned $B put /data/run 100000 3
    # Item 9: payload serialisation, 50k / 250k / 900k quads per row.
    fresh base09
    OTS_CHANGE_CAPTURE_MAX_PAYLOAD=1000000 run payload09-cap1m $B payload /data/run 3 50000 250000 900000
    fresh base09
    run payload09-capdefault $B payload /data/run 3 900000
    ;;
b)
    export OTS_CHANGE_CAPTURE_ENABLED=on
    # Item 9: a 9M-quad import through bulk_insert_quads.
    for i in 1 2; do
        fresh base09
        run bulk9m-off-$i $B bulk /data/run 1000000 off
        fresh base09
        run bulk9m-on-$i $B bulk /data/run 1000000 on
        fresh base09
        OTS_CHANGE_CAPTURE_MAX_PAYLOAD=10000000 run bulk9m-on-payload-$i $B bulk /data/run 1000000 on
    done
    # Item 5: transaction memory, one process per size.
    fresh base09
    for n in 50000 200000 500000 1000000; do
        for r in 1 2 3; do
            run txrss-$n-atomic-on-$r $B txrss /data/run $n atomic on
        done
    done
    for r in 1 2; do
        run txrss-200000-atomic-off-$r $B txrss /data/run 200000 atomic off
        run txrss-1000000-chunked-on-$r $B txrss /data/run 1000000 chunked on
    done
    # Item 5: restore and materialisation at 0.9M, then restore at 9M.
    fresh base09
    for k in 1000 10000 100000; do
        run restore09-k$k $B restore /data/run data 0 100000 $k 3
    done
    # Item 1 at 9M: the before-image of a 9M-quad graph, under the default
    # cap (an unknown row) and with the cap lifted.
    for cap in 250000 20000000; do
        fresh base9m
        OTS_CHANGE_CAPTURE_MAX_SCAN=$cap run scan-data9m-cap$cap $B scan /data/run data 0 1000000 3
    done
    fresh base9m
    run restore9m-k10000 $B restore /data/run data 0 1000000 10000 1 --skip-builds
    ;;
c)
    export OTS_CHANGE_CAPTURE_ENABLED=on
    # Item 5 again: restore at k = 100 000 with the harness fixed, and
    # transaction memory with a trimmed resident-set baseline.
    fresh base09
    run restore09-k100000 $B restore /data/run data 0 100000 100000 3
    fresh base09
    for n in 50000 200000 500000 1000000; do
        for r in 1 2 3; do
            run txrss-$n-atomic-on-$r $B txrss /data/run $n atomic on
        done
    done
    for n in 50000 200000; do
        for r in 1 2; do
            run txrss-$n-atomic-off-$r $B txrss /data/run $n atomic off
        done
    done
    for r in 1 2; do
        run txrss-1000000-chunked-on-$r $B txrss /data/run 1000000 chunked on
    done
    # Items 2 and 9 again, with more repetitions: the first series spread wide.
    fresh base09
    run probe-1m-r5 $B probe /data/run 1000000 5
    fresh base09
    OTS_CHANGE_CAPTURE_MAX_PAYLOAD=1000000 run payload09-cap1m-r7 $B payload /data/run 7 250000 900000
    # Item 10: a cold read of a 0.9M-quad delta version, as other readers see it.
    fresh base17
    OTS_PARALLEL_QUERY_ENABLED=true run coldread17-mirror $B coldread /data/run data 0 100000 other 20000000 88889 10000 20
    fresh base9m
    OTS_PARALLEL_QUERY_ENABLED=true OTS_PARALLEL_QUERY_MAX_TRIPLES=12000000 \
        run coldread99-mirror $B coldread /data/run https://example.org/otl/v09 20000000 100000 data 0 1000000 10000 20
    fresh base9m
    run coldread99-nomirror $B coldread /data/run https://example.org/otl/v09 20000000 100000 data 0 1000000 10000 20
    # Item 11: LDES and blank-node owners.
    for d in 1 4 8 9; do
        empty
        run ldes-d$d-m100 $B ldes /data/run 20000 $d 100 3
    done
    empty
    run ldes-d4-m4000 $B ldes /data/run 20000 4 4000 2
    empty
    run ldes-d4-m6000 $B ldes /data/run 20000 4 6000 2
    # Item 6: statement-provenance storage, 0.9M quads in rows under the payload cap.
    for p in off minimal full; do
        empty
        run annotations-$p $B annotations /data/run 100000 20000 $p
    done
    run census-seed-bundles $B census /w/536/examples/seed-bundles
    # Item 7: kill -9 in the middle of writes.
    run crash $B crash /data/crash 60 10
    rm -rf /data/crash /data/crash-logs
    # Item 3: the mixed phase of scale_otl, capture off against on.
    for i in 1 2 3; do
        for cap in off on; do
            fresh base09
            OTS_CHANGE_CAPTURE_ENABLED=$cap run mixed-$cap-$i $O 100000 /data/run 4 4 20 --skip-load
            du -sb /data/run/changes 2>/dev/null >"$R/mixed-$cap-$i.changes-du" || true
        done
    done
    ;;
d)
    # The end of series c, which stopped when Docker Desktop did (during
    # annotations-full): re-run from there.
    export OTS_CHANGE_CAPTURE_ENABLED=on
    empty
    run annotations-full $B annotations /data/run 100000 20000 full
    run census-seed-bundles $B census /w/536/examples/seed-bundles
    run crash $B crash /data/crash 60 10
    rm -rf /data/crash /data/crash-logs
    for i in 1 2 3; do
        for cap in off on; do
            fresh base09
            OTS_CHANGE_CAPTURE_ENABLED=$cap run mixed-$cap-$i $O 100000 /data/run 4 4 20 --skip-load
            du -sb /data/run/changes 2>/dev/null >"$R/mixed-$cap-$i.changes-du" || true
        done
    done
    ;;
*)
    echo "usage: series.sh smoke|host|load|a|b|c|d" >&2
    exit 2
    ;;
esac
rm -rf /data/run
