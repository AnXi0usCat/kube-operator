# model-operator Helm chart

This chart installs:

- `ModelDeployment` CRD
- RBAC for the operator
- `model-operator` Deployment

## Install

```bash
helm install model-operator ./charts/model-operator \
  --namespace model-serving \
  --create-namespace
```

## Upgrade

```bash
helm upgrade model-operator ./charts/model-operator \
  --namespace model-serving
```

## Uninstall

```bash
helm uninstall model-operator --namespace model-serving
```

## Configure image

```bash
helm upgrade --install model-operator ./charts/model-operator \
  --namespace model-serving --create-namespace \
  --set image.repository=localhost:5050/model-operator \
  --set image.tag=latest
```

## Notes on CRDs

CRDs in `crds/` are installed on `helm install`. Helm does not automatically update them in-place on all upgrades, so treat CRD schema changes as an explicit migration step.
