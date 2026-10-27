#!/bin/bash
# Paired before/after measurements of the delta-versions fixes (2026-10-23),
# on the host and harness of ../2026-10-15-x86-delta/ (see its series.sh).
# "base" is develop at e6f55c62, "after" this branch; both are release
# builds of examples/scale_delta.rs and examples/scale_otl.rs made in the
# builder image, run in it with a named volume at /data and this worktree at
# /w/dp, under the machine lock and an 8 GiB / 8-CPU container limit (the
# machine was shared with other work, so no 9M-quad run was made):
#
#   docker run --rm -m 8g --cpus 8 -v <data volume>:/data -v "$PWD:/w/dp" -w /w/dp <builder image> \
#     bash docs/perf-data/2026-10-23-x86-delta-fixes/series.sh <series>
#
# Series: load, cut (item 5's checkpoint cut, delta cuts and builds at 0.9M),
# bulk (items 2 and 9: bulk_insert_quads at 1M quads), mixed (item 3), crash
# (item 7, after only). The two sides alternate run by run, each on a fresh
# copy of a base store loaded with capture off.
set -u
BIN=/w/dp/target/dp-bin
R=/w/dp/docs/perf-data/2026-10-23-x86-delta-fixes/raw
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

fresh() {
    rm -rf /data/run
    cp -a "/data/$1" /data/run
}

case "${1:-}" in
load)
    export OTS_CHANGE_CAPTURE_ENABLED=off
    rm -rf /data/base09
    run load-base09 $BIN/base/scale_delta load /data/base09 data 0 100000 50000 --with-model
    du -sh /data/base09 >&2
    ;;
cut)
    export OTS_CHANGE_CAPTURE_ENABLED=on
    for i in 1 2 3; do
        for side in base after; do
            fresh base09
            run cut09-$side-$i $BIN/$side/scale_delta restore /data/run data 0 100000 1000 1 --skip-tier2
        done
    done
    ;;
bulk)
    export OTS_CHANGE_CAPTURE_ENABLED=on
    for side in base after; do
        fresh base09
        run probe-1m-$side $BIN/$side/scale_delta probe /data/run 1000000 5
    done
    for i in 1 2 3; do
        for side in base after; do
            for cap in off on; do
                fresh base09
                run bulk1m-$side-$cap-$i $BIN/$side/scale_delta bulk /data/run 111112 $cap
            done
        done
    done
    ;;
mixed)
    for i in 1 2 3; do
        for side in base after; do
            for cap in off on; do
                fresh base09
                OTS_CHANGE_CAPTURE_ENABLED=$cap run mixed-$side-$cap-$i $BIN/$side/scale_otl 100000 /data/run 4 4 20 --skip-load
            done
        done
    done
    ;;
crash)
    export OTS_CHANGE_CAPTURE_ENABLED=on
    rm -rf /data/crash /data/crash-logs
    run crash-after $BIN/after/scale_delta crash /data/crash 60 10
    rm -rf /data/crash /data/crash-logs
    ;;
cutscale)
    # Whether the copy is linear: a checkpoint cut at a third of the size.
    export OTS_CHANGE_CAPTURE_ENABLED=off
    rm -rf /data/base03
    run load-base03 $BIN/base/scale_delta load /data/base03 data 0 33334 50000 --with-model
    export OTS_CHANGE_CAPTURE_ENABLED=on
    for i in 1 2 3; do
        for side in base after; do
            fresh base03
            run cut03-$side-$i $BIN/$side/scale_delta restore /data/run data 0 33334 1000 1 --skip-tier2 --skip-builds
        done
    done
    ;;
*)
    echo "usage: series.sh load|cut|cutscale|bulk|mixed|crash" >&2
    exit 2
    ;;
esac
