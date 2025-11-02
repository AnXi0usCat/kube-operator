use std::{collections::BTreeMap, sync::Arc, time::Duration};

use crate::crd::ModelDeployment;
use k8s_openapi::{
    api::{
        apps::v1::{Deployment, DeploymentSpec, DeploymentStrategy, RollingUpdateDeployment},
        core::v1::{
            Container, ContainerPort, PodSpec, PodTemplateSpec, Service, ServicePort, ServiceSpec,
        },
    },
    apimachinery::pkg::{apis::meta::v1::LabelSelector, util::intstr::IntOrString},
};
use kube::Error as KubeError;
use kube::{
    Api, Client,
    api::{ObjectMeta, PostParams},
};
use kube_runtime::controller::Action;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("Kubernetes API error: {0}")]
    Kube(#[from] KubeError),
}

pub async fn reconsile(md: Arc<ModelDeployment>, _ctx: Arc<Client>) -> Result<Action, Error> {
    Ok(Action::requeue(Duration::from_secs(300)))
}

pub fn error_policy(_object: Arc<ModelDeployment>, _error: &Error, _ctx: Arc<Client>) -> Action {
    Action::requeue(Duration::from_secs(10))
}

async fn ensure_service(api: &Api<Service>, svc_name: &str, app_name: &str) -> Result<(), Error> {
    if api.get_opt(svc_name).await?.is_some() {
        return Ok(());
    }

    let service = Service {
        metadata: ObjectMeta {
            name: Some(svc_name.to_string()),
            labels: Some(BTreeMap::from([("app".into(), app_name.into())])),
            ..Default::default()
        },
        spec: Some(ServiceSpec {
            selector: Some(BTreeMap::from([("app".into(), app_name.into())])),
            ports: Some(vec![ServicePort {
                port: 8000,
                target_port: Some(IntOrString::Int(8000)),
                ..Default::default()
            }]),
            ..Default::default()
        }),
        ..Default::default()
    };

    api.create(&PostParams::default(), &service).await?;
    println!("Created service {}", svc_name);
    Ok(())
}

async fn ensure_deployment(
    api: &Api<Deployment>,
    name: &str,
    image: &str,
    replicas: i32,
) -> Result<(), Error> {
    if api.get_opt(name).await?.is_some() {
        return Ok(());
    }

    let labels = BTreeMap::from([("app".into(), name.into())]);
    let container = Container {
        name: name.into(),
        image: Some(image.into()),
        ports: Some(vec![ContainerPort {
            container_port: 8000,
            ..Default::default()
        }]),
        ..Default::default()
    };

    let deploy = Deployment {
        metadata: ObjectMeta {
            name: Some(name.into()),
            labels: Some(labels.clone()),
            ..Default::default()
        },
        spec: Some(DeploymentSpec {
            replicas: Some(replicas),
            selector: LabelSelector {
                match_labels: Some(labels.clone()),
                ..Default::default()
            },
            template: PodTemplateSpec {
                metadata: Some(ObjectMeta {
                    labels: Some(labels.clone()),
                    ..Default::default()
                }),
                spec: Some(PodSpec {
                    containers: vec![container],
                    ..Default::default()
                }),
            },
            strategy: Some(DeploymentStrategy {
                rolling_update: Some(RollingUpdateDeployment::default()),
                ..Default::default()
            }),
            ..Default::default()
        }),
        ..Default::default()
    };

    api.create(&PostParams::default(), &deploy).await?;
    println!("created Deployment: {}", name);
    Ok(())
}
