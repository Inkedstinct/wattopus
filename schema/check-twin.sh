#!/usr/bin/env bash
# usage: schema/check-twin.sh [greycat-url]
set -euo pipefail

GC="${1:-http://localhost:8080}"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

echo "[contract] ingest fixture -> $GC/twin::ingest"
printf '[%s]' "$(cat "$HERE/ingest.sample.json")" \
  | curl -fsS -X POST -H 'content-type: application/json' -H 'accept: application/json' \
    -d @- "$GC/twin::ingest" >/dev/null
echo "[contract] ingest: OK"

echo "[contract] simulate_scale on the fixture's deployment"
SIM="$(curl -fsS -X POST -H 'content-type: application/json' -H 'accept: application/json' \
  -d '["wattopus","app-store",1]' "$GC/twin::simulate_scale")"
echo "[contract]   $SIM"
echo "$SIM" | grep -q '"cpu_per_pod_predicted"' \
  || { echo "[contract] FAIL: response lacks cpu_per_pod_predicted (shape drifted?)"; exit 1; }

curl -fsS -X POST -H 'content-type: application/json' \
  -d '["wattopus","app-store"]' "$GC/twin::rollback_scale" >/dev/null
echo "[contract] simulate_scale + rollback: OK"

echo "[contract] predictor coupling: deployments / latest / ingest_prediction"
DEPS="$(curl -fsS -X POST -H 'content-type: application/json' -H 'accept: application/json' \
  -d '[]' "$GC/twin::deployments")"
echo "$DEPS" | grep -q '"app-store"' \
  || { echo "[contract] FAIL: twin::deployments does not list the fixture deployment"; exit 1; }

LATEST="$(curl -fsS -X POST -H 'content-type: application/json' -H 'accept: application/json' \
  -d '["wattopus","app-store"]' "$GC/twin::latest")"
echo "[contract]   $LATEST"
echo "$LATEST" | grep -q '"cpu_usage"' \
  || { echo "[contract] FAIL: twin::latest lacks cpu_usage (shape drifted?)"; exit 1; }

curl -fsS -X POST -H 'content-type: application/json' \
  -d '[{"namespace":"wattopus","name":"app-store","timestamp":1730800030,"cpu_predicted":0.2,"joules_predicted":28.0,"quiescent":true}]' \
  "$GC/twin::ingest_prediction" >/dev/null

PRED="$(curl -fsS -X POST -H 'content-type: application/json' -H 'accept: application/json' \
  -d '["wattopus","app-store"]' "$GC/twin::last_prediction")"
echo "[contract]   $PRED"
echo "$PRED" | grep -q '"cpu_predicted"' \
  || { echo "[contract] FAIL: prediction did not round-trip through the twin"; exit 1; }
echo "[contract] predictor coupling: OK"

echo "[contract] graph export for the grafana node-graph panel"
GNODES="$(curl -fsS -X POST -H 'content-type: application/json' -H 'accept: application/json' \
  -d '[]' "$GC/twin::graph_nodes")"
echo "$GNODES" | grep -q '"mainStat"' \
  || { echo "[contract] FAIL: graph_nodes lacks mainStat (node-graph contract drifted?)"; exit 1; }
GEDGES="$(curl -fsS -X POST -H 'content-type: application/json' -H 'accept: application/json' \
  -d '[]' "$GC/twin::graph_edges")"
echo "$GEDGES" | grep -q '"source"' \
  || { echo "[contract] FAIL: graph_edges lacks source"; exit 1; }
echo "[contract] graph export: OK"

echo "[contract] PASS: fixture, ingest, simulate_scale, prediction and graph shapes agree"
