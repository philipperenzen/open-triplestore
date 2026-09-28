#!/usr/bin/env bash
# Prepare an Open Triplestore instance for scripts/scale_compare_http.py: a
# user, a public dataset "otl" owning the benchmark graph, and a bearer token
# (printed on stdout). Usage: scale_setup_ots.sh http://127.0.0.1:7983
set -euo pipefail
B="${1:?usage: scale_setup_ots.sh <base-url>}"
j() { python3 -c 'import sys,json; d=json.load(sys.stdin); print(d'"$1"')'; }
curl -s -X POST "$B/api/auth/register" -H 'Content-Type: application/json' \
  -d '{"username":"bench","email":"bench@example.org","password":"Bench-Pass-2026!"}' >/dev/null || true
TOKEN=$(curl -s -X POST "$B/api/auth/login" -H 'Content-Type: application/json' \
  -d '{"username":"bench","password":"Bench-Pass-2026!"}' | j '["access_token"]')
UID_=$(curl -s "$B/api/auth/me" -H "Authorization: Bearer $TOKEN" | j '["id"]')
curl -s -o /dev/null -X POST "$B/api/datasets" -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
  -d '{"name":"otl","id":"otl","description":"OTL-scale benchmark data","owner_type":"user","owner_id":"'"$UID_"'","visibility":"public"}'
DS=$(curl -s "$B/api/datasets" -H "Authorization: Bearer $TOKEN" | python3 -c 'import sys,json; d=json.load(sys.stdin); items=d if isinstance(d,list) else d.get("datasets", d.get("items", [])); print(next(x["id"] for x in items if x.get("name")=="otl"))')
curl -s -o /dev/null -X POST "$B/api/datasets/$DS/graphs" -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
  -d '{"graph_iri":"https://example.org/otl/instances"}'
echo "$TOKEN"
