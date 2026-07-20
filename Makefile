REGISTRY ?= inkedstinct
TAG ?= 0.1
BINS = app attributor feeder operator mockpower predictor

build:
	for b in $(BINS); do \
	  docker build --build-arg BIN=$$b -t $(REGISTRY)/$$b:$(TAG) . || exit 1; \
	done
	docker build -t $(REGISTRY)/greycat-twin:$(TAG) greycat
	docker build -t $(REGISTRY)/kubediagram:$(TAG) deploy/kubediagram

deploy-base:
	kubectl apply -f deploy/

undeploy-base:
	kubectl delete -f deploy/ --ignore-not-found

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

