# Local development on kind

Run the whole Wattopus stack on your laptop, with no Grid'5000 reservation and no image registry. Good for iterating on the Rust binaries and the twin.

The only thing that differs from the real cluster (kube5k) is the power source: locally, **mockpower**. It reads each pod's CPU from
Prometheus and turns cores into watts, so the rest of the pipeline behaves exactly as it does on Grid'5000.

kube5k is kubeadm vanilla Kubernetes; kind is the same bootstrap family (kubeadm + containerd), so every manifest in `deploy/` applies unchanged.

## Prerequisites

- Docker
- `kubectl`
- `kind`  install once:
  ```sh
  curl -fsSL -o kind https://kind.sigs.k8s.io/dl/v0.32.0/kind-linux-amd64
  chmod +x kind && mv kind ~/.local/bin/kind
  ```

## 1. Create the cluster

```sh
kind create cluster --config deploy/local/kind.yaml
```

Starts one Docker container that acts as a Kubernetes node, bootstrapped with kubeadm. The profile pins the node image (`v1.33.12`, tracking kube5k's 1.33 line) and maps three NodePorts to your machine, so once the services are up:

| Service    | URL                     |
|------------|-------------------------|
| Grafana    | http://localhost:30300  |
| Prometheus | http://localhost:30900  |
| GreyCat    | http://localhost:30808  |

Check it:

```sh
kubectl get nodes            # wattopus-control-plane   Ready
```

## 2. Build the images

The manifests use `inkedstinct/*` images that aren't on a public registry, so
build them locally first. One command builds all eight, at the tag the
manifests reference (`inkedstinct/*:0.1`):

```sh
docker login -u YOUR_LOGIN #From DockerHub and then paste your personnal token
REGISTRY=YOUR_REGISTRY make build #DockerHub registry 
```

`make build` runs one `docker build` per image  the six Rust binaries (each
via `--build-arg BIN=`), the GreyCat twin, and kubediagram. It reads two
knobs, already defaulted for local use:

| Knob       | Default      | Effect                    |
|------------|--------------|---------------------------|
| `REGISTRY` | `inkedstinct`| image namespace           |
| `TAG`      | `0.1`        | tag on every image        |

The first run compiles the whole Rust workspace and is slow; the Docker layer
cache makes later builds quick.

## 3. Load the images into the cluster

kind runs its own container image store, separate from your Docker daemon, so a freshly built image isn't visible to the cluster until you load it in. This is what replaces "push to a registry"  no push, no pull, no reservation.

```sh
kind load docker-image inkedstinct/attributor:0.1   --name wattopus
kind load docker-image inkedstinct/feeder:0.1       --name wattopus
kind load docker-image inkedstinct/operator:0.1     --name wattopus
kind load docker-image inkedstinct/predictor:0.1    --name wattopus
kind load docker-image inkedstinct/mockpower:0.1    --name wattopus
kind load docker-image inkedstinct/app:0.1          --name wattopus
kind load docker-image inkedstinct/greycat-twin:0.1 --name wattopus
kind load docker-image inkedstinct/kubediagram:0.1  --name wattopus
```

Because each image is now present in the node, `imagePullPolicy: IfNotPresent` (what every manifest uses) finds it locally and never contacts a registry.

## 4. Deploy

```sh
kubectl apply -f deploy/                                              # the stack + mockpower
kubectl apply -f deploy/apps/rust-demo.yaml -f deploy/apps/rust-demo-load.yaml   # demo app + load generator
```

The load generator drives traffic through the demo app continuously, which is what produces the traces the attributor needs without it every pod's power lands in `_unattributed`.

## 5. Verify

```sh
kubectl -n wattopus get pods         # all Running
```

Confirm power is flowing and being attributed:

```sh
# mockpower is producing watts (needs a minute for the CPU rate() to warm up)
kubectl -n wattopus port-forward deploy/mockpower 9105:9105 &
curl -s localhost:9105/metrics | grep mockpower_pod_watts

# the attributor is turning traces into route watts
kubectl -n wattopus port-forward deploy/attributor 9500:9500 &
curl -s localhost:9500/metrics | grep wattopus_route_power_watts
```

Then open Grafana at http://localhost:30300, the Wattopus Twin dashboard is provisioned automatically.

## The inner loop: change one binary

After editing a Rust crate, rebuild just that image, reload it, and restart its deployment. Three commands (attributor as the example):

```sh
docker build --build-arg BIN=attributor -t inkedstinct/attributor:0.1 .
kind load docker-image inkedstinct/attributor:0.1 --name wattopus
kubectl -n wattopus rollout restart deploy/attributor
```

The `rollout restart` is needed because the tag didn't change: reloading the image replaces the bits under `inkedstinct/attributor:0.1` in the node, and the restart creates a new pod that picks them up. (Loading alone doesn't touch the running pod.)

## Teardown

```sh
kind delete cluster --name wattopus
```

The cluster lives entirely inside Docker deleting it leaves your host
untouched, and step 1 recreates it in about 30 seconds (the node image is
cached after the first pull).

## Notes