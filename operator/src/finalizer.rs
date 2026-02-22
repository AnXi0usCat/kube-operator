use crate::error::Error;
use crate::{crd::ModelDeployment, event::Outcome};
use kube::{
    Api, Client, Resource, ResourceExt,
    api::{Patch, PatchParams},
};
use serde_json::json;

pub const FINALIZER: &str = "ml.jedimindtricks.example/finalizer";

pub fn is_deleting(md: &ModelDeployment) -> bool {
    md.meta().deletion_timestamp.is_some()
}

pub fn has_finalizer(md: &ModelDeployment, finalizer: &str) -> bool {
    md.meta()
        .finalizers
        .as_ref()
        .map(|fs| fs.iter().any(|x| x == finalizer))
        .unwrap_or(false)
}

pub async fn ensure_finalizer_present(
    client: &Client,
    md: &ModelDeployment,
    ns: &str,
    finalizer: &str,
) -> Result<Outcome, Error> {
    if has_finalizer(md, finalizer) {
        return Ok(Outcome::NoOp);
    }

    let api: Api<ModelDeployment> = Api::namespaced(client.clone(), ns);
    let name = md.name_any();

    let mut finalizers = md.meta().finalizers.clone().unwrap_or_default();
    finalizers.push(finalizer.into());

    let patch = json!({
        "metadata": {"finalizers": finalizers}
    });

    api.patch_metadata(&name, &PatchParams::default(), &Patch::Merge(&patch))
        .await?;
    Ok(Outcome::Created)
}

pub async fn remove_finalizer(
    client: &Client,
    md: &ModelDeployment,
    ns: &str,
    finalizer: &str,
) -> Result<Outcome, Error> {
    let api: Api<ModelDeployment> = Api::namespaced(client.clone(), ns);
    let name = md.name_any();

    let mut finalizers = md.meta().finalizers.clone().unwrap_or_default();
    finalizers.retain(|x| x != finalizer);

    let patch = json!({
        "metadata": {"finalizers": finalizers}
    });

    api.patch_metadata(&name, &PatchParams::default(), &Patch::Merge(&patch))
        .await?;
    Ok(Outcome::Updated)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crd::{ModelDeployment, ModelDeploymentSpec, ModelVariant};
    use serde_json::json;

    fn model_deployment_with_finalizers(finalizers: Option<Vec<&str>>) -> ModelDeployment {
        let mut md = ModelDeployment::new(
            "sample-md",
            ModelDeploymentSpec {
                live: ModelVariant {
                    image: "ghcr.io/acme/live:1.0.0".into(),
                    replicas: 1,
                },
                ..Default::default()
            },
        );

        md.metadata.finalizers = finalizers.map(|items| {
            items
                .into_iter()
                .map(std::string::ToString::to_string)
                .collect()
        });

        md
    }

    #[test]
    fn has_finalizer_returns_true_when_present() {
        let md = model_deployment_with_finalizers(Some(vec![FINALIZER]));
        assert!(has_finalizer(&md, FINALIZER));
    }

    #[test]
    fn has_finalizer_returns_false_when_missing() {
        let md = model_deployment_with_finalizers(Some(vec!["other.example/finalizer"]));
        assert!(!has_finalizer(&md, FINALIZER));
    }

    #[test]
    fn is_deleting_detects_deletion_timestamp() {
        let deleting: ModelDeployment = serde_json::from_value(json!({
            "apiVersion": "ml.jedimindtricks.example/v1alpha1",
            "kind": "ModelDeployment",
            "metadata": {
                "name": "sample-md",
                "deletionTimestamp": "2026-02-22T00:00:00Z"
            },
            "spec": {
                "live": {
                    "image": "ghcr.io/acme/live:1.0.0"
                }
            }
        }))
        .expect("model deployment should deserialize");

        let active = model_deployment_with_finalizers(None);

        assert!(is_deleting(&deleting));
        assert!(!is_deleting(&active));
    }
}
