# Power meters

The meter is a module behind one interface: **per-pod watts in Prometheus**.
`POWER_QUERY` on the attributor and the feeder must return
`(namespace, pod) -> watts` — the Rust side reads exactly the labels
`namespace` and `pod` from the query result. Anything exporting per-pod
watts fits.

## mockpower — default (local/kind)

Deterministic linear model (`cpu share x WATTS_PER_CORE`), part of the base
deploy. Stays forever: it is the reproducible meter for reviewers without
cluster hardware (issue #13).

## Kepler — the real meter (Grid'5000 path, implemented)

[Kepler](https://github.com/sustainable-computing-io/kepler) reads RAPL on
each node (privileged DaemonSet, bare metal — no BMC involved) and splits
power per pod. Installed by reference from the upstream OCI chart, pinned in
the Makefile (`KEPLER_CHART_VERSION`); our only config is
`deploy/kepler-values.yaml` (scrape annotations + log level).

    make deploy-storage            # only on clusters w/o a default StorageClass
    kubectl apply -k deploy/g5k    # base + Kepler POWER_QUERY, mockpower dropped
    make deploy-kepler             # the meter itself, ns kepler, port 28282

The g5k overlay swaps `POWER_QUERY` to `kepler_pod_cpu_watts` re-labelled via
`label_replace` (Kepler exports `pod_namespace`/`pod_name`, and the pods
scrape job attaches its own `namespace`/`pod` which must be overwritten —
the exporting Kepler pod is kept as `meter_pod` so the meter's identity is
never lost). Verified live on kube5k against Kepler v0.11.4.

Caveat: RAPL covers CPU/DRAM package power, not the full node (fans, PSU
losses) — fine for attribution, wrong for a datacenter bill.

## KubeWatt — documented alternative (needs BMC credentials)

[KubeWatt](https://github.com/bjornpijnacker/kubewatt) reads real node power
over Redfish (BMC/iDRAC) and splits it across containers by CPU share.
**Redfish is its only power source, and Grid'5000 does not hand out iDRAC
credentials** (probed 2026-07-22: `/redfish/v1/` answers 200 from pods, the
Power endpoint 401s) — so it cannot run on G5K. On infra where you own the
BMCs:

1. Check Redfish access from the cluster to each node's BMC.
2. Run the KubeWatt init job once (`INIT_BASE` on an empty cluster or
   `INIT_BOOTSTRAP` on a busy one) to get per-node static power values.
3. `helm install kubewatt ./chart` from the KubeWatt repo with `values.yaml`
   carrying the Redfish endpoints, credentials, node list, and the static
   power values from step 2.
4. Annotate its pods with `prometheus.io/scrape: "true"` +
   `prometheus.io/port` so the pods job picks them up.
5. Check the exported metric names (`curl <kubewatt>:port/metrics`), then set
   `POWER_QUERY` to a query returning `(namespace, pod) -> watts`.

## Kwollect — ground truth, not wired

G5K's own metering API is open from inside the cluster
(`https://api.grid5000.fr/stable/sites/<site>/metrics`, no auth):
`bmc_node_power_watt` at 5 s per node (no `wattmetre` on dahu). Node-level
only — kept as a future cross-check of `sum(pod watts)` vs measured node
power (roadmap).
