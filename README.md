# Wattopus POC (Rust)

Modular proof of concept: measure the energy of a Kubernetes application per
**traced request path**, mirror the cluster into a **GreyCat digital twin**,
and change the cluster **through the twin**: simulate first, apply only if
requirements hold.

Every component we own is Rust. GreyCat (the twin database), Prometheus,
Grafana, Tempo and the OpenTelemetry Collector stay as-is: they are storage
and transport, not our logic.


## What any workload must provide

The platform discovers everything else from the apiserver and cAdvisor. To
get per-route attribution, an app has to follow three common OpenTelemetry
conventions:

1. emit OTLP traces with `service.name` equal to its Deployment name
   (pods are matched by the `<service>-` name prefix);
2. carry the route on the root span, in `http.route` (configurable via
   `ROUTE_ATTR`) or as the span name;
3. propagate W3C `traceparent` between services so a request is one trace.

Apps that do none of this still show up in the twin, in the predictor and in
`_unattributed` power. They only miss the per-route split.


## Quickstart

Needs: a cluster (kubectl configured), docker, images loadable by the cluster
nodes (push to a registry and set `REGISTRY=`, or `kind load` `ctr images import` the local ones).

- the "Wattopus Twin" dashboard: the twin graph as a node-graph panel
  (joules per node, replicas, quiescence) + route power over time
- traces: Explore / Tempo, search service `app-gateway`
- energy per path: `rate(wattopus_route_energy_joules_total[5m])` by `route`
- raw power: `mockpower_pod_watts`

The twin itself is browsable at `greycat:8080/explorer/` (GreyCat Explorer),
and a live KubeDiagrams rendering of the namespace (re-rendered by the
`kubediagram` deployment on shape changes) shows up in the same dashboard
as a Business Media panel.

## The twin

The feeder posts, every 30 s, the full picture to GreyCat: nodes, namespaces,
deployments (with replicas), services, pods, containers, each with
cpu/ram/disk usage, availability, and joules as temporal series. The
predictor reads the freshest sample per deployment back from the twin, keeps
a sliding window, and writes a `Prediction` (forecast + quiescence verdict)
next to the metrics. Scale decisions read from and are staged in the same
graph.

## Simulate, then apply

The operator asks the twin to stage the change (`twin::simulate_scale`
mutates `staged_replicas` and predicts per-pod CPU and joules). If predicted
CPU per pod stays under `CPU_LIMIT_PER_POD` (0.8 <- Magic number !), the operator scales the
real deployment via the apiserver and commits the twin. Otherwise it answers
`applied: false` with the reason and rolls the twin back. The operator is
itself traced, so the decision shows up in Tempo.

# Test

## The twin contract

Everything that crosses into or out of GreyCat is pinned three ways:

- `crates/ingest` holds the typed Rust structs (`Snapshot` and `Prediction`
  in, `ScaleSimulation` and `LatestSample` out); feeder, operator and
  predictor can only speak those shapes, the compiler enforces the producer
  side.
- `schema/ingest.sample.json` is the golden "fixture": the cross-language
  contract as one file. A unit test is set to make it noisy on fails. 
- `schema/check-twin.sh` fires the same fixture at a live GreyCat and
  exercises simulation, prediction and the graph export

## Next

- mockpower is a linear CPU model, check KubeWatt
- availability metrics are computed at node level only 
- the what-if is linear: joules scale with replicas, CPU work is conserved, we'll use LSTM I guess
- the prediction is a mock: mean/sigma quiescence + persistence forecast
- service-to-pod resolution is a quite fragile
- store keeps items in memory per replica; scaling changes what `/catalog` sees. Check volumes mounting
- all storage is emptyDir: history dies with pods 
