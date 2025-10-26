use std::{sync::Arc, time::Duration};

use kube::Client;
use kube_runtime::controller::Action;
use thiserror::Error;
use crate::crd::ModelDeployment;

#[derive(Debug, Error)]
pub enum Error {}

pub async fn reconsile(
    md: Arc<ModelDeployment>,
    _ctx: Arc<Client>,
) -> Result<Action, Error> {
    Ok(Action::requeue(Duration::from_secs(300)))
}

pub fn error_policy(
    _object: Arc<ModelDeployment>,
    _error: &Error,
    _ctx: Arc<Client>,
) -> Action {
    Action::requeue(Duration::from_secs(10))
}
