# Model Serving Operator

Kubernetes operator (Rust + `kube-rs`) for a custom resource named `ModelDeployment`.

The goal is to manage model serving under one umbrella resource:
- `live` deployment: serves responses to clients
- `shadow` deployment (optional): receives mirrored traffic for comparison/testing
- Traefik routing layer in front that mirrors requests to shadow while returning only live responses

## Repository Structure

- `operator/`: Rust operator/controller code
- `crds/modeldeployment.yaml`: CRD definition for `ModelDeployment`
- `examples/modeldeployment-sample.yaml`: sample custom resource
- `deploy/`: raw Kubernetes manifests (RBAC + operator deployment)
- `charts/model-operator/`: Helm chart for one-command install
- `model-server/`: simple sample model server image
- `justfile`: local workflow commands
- `docs/architecture.md`: architecture diagram and component flow

## Architecture Diagram

See `docs/architecture.md` for a Mermaid diagram of the reconciliation and traffic flow.

## How It Works

When you apply a `ModelDeployment`, the operator reconciles and ensures:
- Finalizer is present
- `Service` for live variant
- `Deployment` for live variant
- `Service` and `Deployment` for shadow variant (if configured)
- `TraefikService` and `IngressRoute` when `trafficMirror: true`
- Status updates on the custom resource (`phase`, `conditions`, live/shadow replica status)

Status phase logic:
- `Available`: desired replicas ready for live (and shadow if present)
- `Progressing`: rollout/scaling still in progress
- `Degraded`: live desired replicas > 0 but available live replicas == 0

## Prerequisites

- Kubernetes cluster (the project uses `k3d` in examples)
- `kubectl`
- `docker`
- `helm` (recommended install path)
- `just` (optional, for helper commands)
- Traefik CRDs/controller installed in your cluster (for mirroring path)

## Install Option 1: Helm (Recommended)

Install operator + RBAC + CRD:

```bash
helm upgrade --install model-operator ./charts/model-operator \
  --namespace model-serving \
  --create-namespace
```

To use a custom operator image:

```bash
helm upgrade --install model-operator ./charts/model-operator \
  --namespace model-serving --create-namespace \
  --set image.repository=k3d-registry.localhost:5000/model-operator \
  --set image.tag=latest
```

Render manifests locally:

```bash
just helm-template
```

Uninstall:

```bash
just helm-uninstall
```

## Install Option 2: Raw Manifests

```bash
kubectl apply -f deploy/rbac.yaml
kubectl apply -f deploy/operator-deployment.yaml
kubectl rollout status deploy/model-operator -n default
```

## Local Dev Workflow (k3d + images)

Bring up local cluster:

```bash
just k3d-up
```

Build and push operator image:

```bash
just build-operator
just image-operator
just push-operator
```

Build and push sample model image:

```bash
just image-model
```

## Using the Custom Resource

Sample resource is in `examples/modeldeployment-sample.yaml`.

Apply:

```bash
kubectl apply -f examples/modeldeployment-sample.yaml
```

Inspect:

```bash
kubectl get modeldeployments -A
kubectl get deploy,svc -A | grep sentiment-analyzer
kubectl describe modeldeployment sentiment-analyzer -n default
```

If you installed the operator in `model-serving` namespace but your sample CR is in `default`, that is fine for a cluster-scoped controller. Keep namespace references consistent in your own manifests.

## ModelDeployment Spec (Current)

Main fields:
- `spec.live` (required): image + replicas
- `spec.shadow` (optional): image + replicas
- `spec.trafficMirror` (default `false`)
- `spec.rolloutStrategy` (default `rolling`)
- `spec.resources` (optional)
- `spec.autoscaling` (optional schema field; not fully reconciled yet)
- `spec.probes` (optional schema field; not fully reconciled yet)
- `spec.configRef` (optional schema field; not fully reconciled yet)

Minimal example:

```yaml
apiVersion: ml.jedimindtricks.example/v1alpha1
kind: ModelDeployment
metadata:
  name: sentiment-analyzer
  namespace: default
spec:
  live:
    image: k3d-registry.localhost:5000/model-server:latest
    replicas: 2
  shadow:
    image: k3d-registry.localhost:5000/model-server:latest
    replicas: 1
  trafficMirror: true
```

## Testing

Run Rust unit tests:

```bash
cargo test -p operator
```

Current tests cover:
- CRD/spec defaulting and serialization behavior
- finalizer helper logic
- status phase/condition computation for live/shadow scenarios

## Notes

- CRDs under Helm `crds/` are installed on first install. CRD schema upgrades should be handled explicitly.
- Mirroring behavior depends on Traefik CRDs (`IngressRoute`, `TraefikService`) being available in cluster.
