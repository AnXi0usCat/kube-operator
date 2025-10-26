use std::{sync::Arc, time::Duration};

use kube::Client;
use kube_runtime::controller;

use crate::crd::ModelDeployment;

pub async fn reconsile(
    md: Arc<ModelDeployment>,
    client: Arc<Client>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    Ok(())
}

pub async fn error_policy(
    _object: Arc<ModelDeployment>,
    _error: &Box<dyn std::error::Error + Send + Sync>,
) -> controller::Action {
    controller::Action::requeue(Duration::from_secs(10))
}
