# Default shell
set shell := ["bash", "-eu", "-o", "pipefail", "-c"]

# ---- cluster lifecycle ----
k3d-up:
    k3d registry create registry.localhost --port 5050 || true
    k3d cluster create dev \
      --servers 1 --agents 1 \
      --api-port 6550 \
      -p "80:80@loadbalancer" -p "443:443@loadbalancer" \
      --registry-use k3d-registry.localhost:5050 || true
    kubectl get nodes

k3d-down:
    k3d cluster delete dev || true
    k3d registry delete registry.localhost || true

# ---- operator image ----
build-operator:
    cd operator && cargo build --release

image-operator:
    docker build -t localhost:5050/model-operator:latest -f operator/Dockerfile .

push-operator:
    docker push localhost:5050/model-operator:latest

deploy-rbac:
    kubectl apply -f deploy/rbac.yaml

deploy-operator: deploy-rbac
    kubectl apply -f deploy/operator-deployment.yaml
    kubectl rollout status deploy/model-operator -n default

logs-operator:
    stern model-operator -n default || kubectl logs -l app=model-operator -f

# ---- model server (sample) ----
image-model:
    docker build -t localhost:5050/model-server:latest model-server
    docker push localhost:5050/model-server:latest

# ---- quick checks ----
cluster-info:
    kubectl cluster-info
    kubectl get sc
    kubectl get ns
