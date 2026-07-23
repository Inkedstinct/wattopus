REGISTRY ?= inkedstinct
TAG ?= 0.2.0
LOCAL_PATH_VERSION ?= v0.0.31
KEPLER_CHART ?= oci://quay.io/sustainable_computing_io/charts/kepler
KEPLER_CHART_VERSION ?= 0.11.4
BINS = app attributor feeder operator mockpower predictor

build:
	for b in $(BINS); do \
	  docker build --build-arg BIN=$$b -t $(REGISTRY)/$$b:$(TAG) . || exit 1; \
	done
	docker build -t $(REGISTRY)/greycat-twin:$(TAG) greycat
	docker build -t $(REGISTRY)/kubediagram:$(TAG) deploy/kubediagram

set-version:
	sed -i 's/^TAG ?= .*/TAG ?= $(VERSION)/' Makefile
	grep -rl --include='*.yaml' 'inkedstinct/.*:' deploy/ | xargs sed -i -E 's#(inkedstinct/[a-z-]+):[0-9][0-9A-Za-z.-]*#\1:$(VERSION)#g'


deploy-base:
	kubectl apply -k deploy/

undeploy-base:
	kubectl delete -k deploy/ --ignore-not-found

deploy-storage:
	kubectl apply -f https://raw.githubusercontent.com/rancher/local-path-provisioner/$(LOCAL_PATH_VERSION)/deploy/local-path-storage.yaml
	kubectl -n local-path-storage rollout status deploy/local-path-provisioner
	kubectl patch storageclass local-path -p '{"metadata":{"annotations":{"storageclass.kubernetes.io/is-default-class":"true"}}}'


deploy-kepler:
	helm upgrade --install kepler $(KEPLER_CHART) --version $(KEPLER_CHART_VERSION) \
	  -n kepler --create-namespace -f deploy/kepler-values.yaml

undeploy-kepler:
	helm uninstall kepler -n kepler

deploy-rust-demo:
	kubectl apply -f deploy/apps/rust-demo.yaml -f deploy/apps/rust-demo-load.yaml

undeploy-rust-demo:
	kubectl delete -f deploy/apps/rust-demo.yaml -f deploy/apps/rust-demo-load.yaml --ignore-not-found

deploy-otel-demo:
	helm upgrade --install otel-demo open-telemetry/opentelemetry-demo \
	  -n otel-demo --create-namespace -f deploy/apps/otel-demo/values.yaml

undeploy-otel-demo:
	helm uninstall otel-demo -n otel-demo

test:
	cargo test --workspace

contract:
	schema/check-twin.sh $${GREYCAT_URL:-http://localhost:8080}

backup:
	curl -s -X POST -d '[]' $${GREYCAT_URL:-http://localhost:8080}/runtime::Runtime::backup_full

demo:
	kubectl -n wattopus run curl-demo --rm -i --restart=Never --image=curlimages/curl -- \
	sh -c 'curl -s app-gateway:8000/checkout; echo; curl -s app-gateway:8000/catalog; echo; curl -s app-gateway:8000/report; echo'

simulate-1:
	kubectl -n wattopus run curl-sim --rm -i --restart=Never --image=curlimages/curl -- \
	sh -c 'curl -s -X POST operator:8080/simulate -H "content-type: application/json" \
	-d "{\"namespace\":\"wattopus\",\"deployment\":\"app-store\",\"replicas\":1}"; echo'

simulate-2:
	kubectl -n wattopus run curl-sim --rm -i --restart=Never --image=curlimages/curl -- \
	sh -c 'curl -s -X POST operator:8080/simulate -H "content-type: application/json" \
	-d "{\"namespace\":\"wattopus\",\"deployment\":\"app-store\",\"replicas\":2}"; echo'

simulate-3:
	kubectl -n wattopus run curl-sim --rm -i --restart=Never --image=curlimages/curl -- \
	sh -c 'curl -s -X POST operator:8080/simulate -H "content-type: application/json" \
	-d "{\"namespace\":\"wattopus\",\"deployment\":\"app-store\",\"replicas\":3}"; echo'

build-kind:
	kind create cluster --config deploy/local/kind.yaml

delete-kind:
	kind delete cluster -n wattopus