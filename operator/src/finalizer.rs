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
