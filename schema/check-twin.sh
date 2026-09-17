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

echo "[contract] route coupling: ingest_routes -> route_history"
printf '[%s]' "$(cat "$HERE/routes.sample.json")" \
  | curl -fsS -X POST -H 'content-type: application/json' -H 'accept: application/json' \
    -d @- "$GC/twin::ingest_routes" >/dev/null

HIST="$(curl -fsS -X POST -H 'content-type: application/json' -H 'accept: application/json' \
  -d '["wattopus",1730800000,1730800060]' "$GC/twin::route_history")"
echo "$HIST" | grep -q '"service_watts"' \
  || { echo "[contract] FAIL: route_history lacks service_watts (shape drifted?)"; exit 1; }
echo "$HIST" | grep -q '/checkout' \
  || { echo "[contract] FAIL: route_history did not return the fixture's route"; exit 1; }
echo "[contract] route coupling: OK"

echo "[contract] model coupling: ingest_route_model -> route_models -> simulate_load"
curl -fsS -X POST -H 'content-type: application/json' \
  -d '[{"namespace":"wattopus","service":"app-compute","fitted_at":1730800030,"intercept":0.4,"coefs":[{"route":"/checkout","watts_per_rps":0.1}],"r2":0.9,"samples":120}]' \
  "$GC/twin::ingest_route_model" >/dev/null

MODELS="$(curl -fsS -X POST -H 'content-type: application/json' -H 'accept: application/json' \
  -d '["wattopus"]' "$GC/twin::route_models")"
echo "$MODELS" | grep -q '"watts_per_rps"' \
  || { echo "[contract] FAIL: route_models lacks watts_per_rps (shape drifted?)"; exit 1; }

LOAD="$(curl -fsS -X POST -H 'content-type: application/json' -H 'accept: application/json' \
  -d '["wattopus","/checkout",10.0]' "$GC/twin::simulate_load")"
echo "[contract]   $LOAD"
echo "$LOAD" | grep -q '"watts_predicted"' \
  || { echo "[contract] FAIL: simulate_load lacks watts_predicted"; exit 1; }

CSV="$(curl -fsS -X POST -H 'content-type: application/json' -H 'accept: application/json' \
  -d '["wattopus",1730800000,1730800060]' "$GC/twin::export_history" | tr -d '"')"
echo "[contract]   exported $CSV"
echo "$CSV" | grep -q 'route_history_' \
  || { echo "[contract] FAIL: export_history did not return a path"; exit 1; }
echo "[contract] model coupling: OK"

echo "[contract] PASS: fixture, ingest, simulate_scale, prediction and graph shapes agree"
