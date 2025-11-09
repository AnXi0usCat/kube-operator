use std::{collections::BTreeMap, fmt::Display, sync::Arc, time::Duration};

use crate::crd::ModelDeployment;
use k8s_openapi::{
    api::{
        apps::v1::{Deployment, DeploymentSpec, DeploymentStrategy, RollingUpdateDeployment},
        core::v1::{
            Container, ContainerPort, PodSpec, PodTemplateSpec, Service, ServicePort, ServiceSpec,
        },
        networking::v1::{
            HTTPIngressPath, HTTPIngressRuleValue, Ingress, IngressBackend, IngressRule,
            IngressServiceBackend, IngressSpec, ServiceBackendPort,
        },
    },
    apimachinery::pkg::{
        apis::meta::v1::{LabelSelector, OwnerReference},
        util::intstr::IntOrString,
    },
};
use kube::{
    Api, Client, ResourceExt,
    api::{ObjectMeta, Patch, PatchParams, PostParams},
    core::object::HasSpec,
};
use kube::{Error as KubeError, Resource};
use kube_runtime::controller::Action;
use serde_json::json;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("Kubernetes API error: {0}")]
    Kube(#[from] KubeError),
}

#[derive(Debug, PartialEq)]
enum DeploymentType {
    Live,
    Shadow,
}

impl Display for DeploymentType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DeploymentType::Live => write!(f, "live"),
            DeploymentType::Shadow => write!(f, "shadow"),
        }
    }
}

impl AsRef<str> for DeploymentType {
    fn as_ref(&self) -> &str {
        match self {
            DeploymentType::Live => "live",
            DeploymentType::Shadow => "Shadow",
        }
    }
}

fn owner_ref(md: &ModelDeployment) -> OwnerReference {
    md.controller_owner_ref(&()).unwrap()
}

pub async fn reconsile(md: Arc<ModelDeployment>, ctx: Arc<Client>) -> Result<Action, Error> {
    let ns = md.namespace().unwrap_or_else(|| "default".into());
    let base_name = md.name_any();
    let spec = md.spec();

    println!("Reconciling ModelDeployment {}/{}", ns, base_name);

    let svc_api: Api<Service> = Api::namespaced(ctx.as_ref().clone(), &ns);
    ensure_service(&svc_api, &md, &base_name, DeploymentType::Live).await?;

    if spec.shadow.is_some() {
        ensure_service(&svc_api, &md, &base_name, DeploymentType::Shadow).await?;
    }

    let deployment_api: Api<Deployment> = Api::namespaced(ctx.as_ref().clone(), &ns);
    ensure_deployment(
        &deployment_api,
        &md,
        &format!("{}-live", base_name),
        &base_name,
        &spec.live.image,
        spec.live.replicas,
        DeploymentType::Live,
    )
    .await?;

    if let Some(shadow) = &spec.shadow {
        ensure_deployment(
            &deployment_api,
            &md,
            &format!("{}-shadow", base_name),
            &base_name,
            &shadow.image,
            shadow.replicas,
            DeploymentType::Shadow,
        )
        .await?;
    }

    if spec.traffic_mirror {
        let ingress_api: Api<Ingress> = Api::namespaced(ctx.as_ref().clone(), &ns);
        ensure_ingress(&ingress_api, &md, &base_name).await?;
    }
    Ok(Action::requeue(Duration::from_secs(60)))
}

pub fn error_policy(_object: Arc<ModelDeployment>, _error: &Error, _ctx: Arc<Client>) -> Action {
    Action::requeue(Duration::from_secs(10))
}

async fn ensure_service(
    api: &Api<Service>,
    md: &ModelDeployment,
    base_name: &str,
    role: DeploymentType,
) -> Result<(), Error> {
    let svc_name = format!("{}-{}-svc", base_name, role);

    if api.get_opt(&svc_name).await?.is_some() {
        return Ok(());
    }

    let mut labels = BTreeMap::new();
    labels.insert("app".into(), base_name.to_string());
    labels.insert("role".into(), role.to_string());

    let svc = Service {
        metadata: ObjectMeta {
            name: Some(svc_name.clone()),
            labels: Some(labels.clone()),
            owner_references: Some(vec![owner_ref(md)]),
            ..Default::default()
        },
        spec: Some(ServiceSpec {
            selector: Some(labels),
            ports: Some(vec![ServicePort {
                port: 8000,
                target_port: Some(IntOrString::Int(8000)),
                ..Default::default()
            }]),
            ..Default::default()
        }),
        ..Default::default()
    };

    api.create(&PostParams::default(), &svc).await?;
    println!("Created service {}", svc_name);
    Ok(())
}

async fn ensure_deployment(
    api: &Api<Deployment>,
    md: &ModelDeployment,
    deployment_name: &str,
    base_name: &str,
    image: &str,
    replicas: i32,
    role: DeploymentType,
) -> Result<(), Error> {
    if api.get_opt(deployment_name).await?.is_some() {
        return Ok(());
    }

    let mut labels = BTreeMap::new();
    labels.insert("app".into(), base_name.to_string());
    labels.insert("role".into(), role.to_string());

    let container = Container {
        name: deployment_name.into(),
        image: Some(image.into()),
        ports: Some(vec![ContainerPort {
            container_port: 8000,
            ..Default::default()
        }]),
        ..Default::default()
    };

    let deploy = Deployment {
        metadata: ObjectMeta {
            name: Some(deployment_name.into()),
            labels: Some(labels.clone()),
            owner_references: Some(vec![owner_ref(md)]),
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
    println!("created Deployment: {}", deployment_name);
    Ok(())
}

async fn ensure_ingress(
    api: &Api<Ingress>,
    md: &ModelDeployment,
    base_name: &str,
) -> Result<(), Error> {
    let ing_name = base_name.to_string();

    if api.get_opt(&ing_name).await?.is_some() {
        return Ok(());
    }

    let live_svc_name = format!("{}-live-svc", base_name);
    let live_backend = IngressBackend {
        service: Some(IngressServiceBackend {
            name: live_svc_name.into(),
            port: Some(ServiceBackendPort {
                number: Some(8000),
                name: None,
            }),
        }),
        resource: None,
    };

    let rule = IngressRule {
        host: Some(format!("{}.local", base_name)),
        http: Some(HTTPIngressRuleValue {
            paths: vec![HTTPIngressPath {
                path: Some("/".into()),
                path_type: "Prefix".to_string(),
                backend: live_backend.clone(),
            }],
        }),
    };

    let shadow_svc_name = format!("{}-shadow-svc", base_name);
    let mut annotations = std::collections::BTreeMap::new();
    annotations.insert(
        "nginx.ingress.kubernetes.io/mirror-target".into(),
        shadow_svc_name,
    );

    let ing = Ingress {
        metadata: kube::core::ObjectMeta {
            name: Some(ing_name.to_string()),
            annotations: Some(annotations),
            owner_references: Some(vec![owner_ref(md)]),
            ..Default::default()
        },
        spec: Some(IngressSpec {
            rules: Some(vec![rule]),
            ..Default::default()
        }),
        ..Default::default()
    };

    api.create(&PostParams::default(), &ing).await?;
    println!("created Ingress {}", base_name);
    Ok(())
}

async fn update_status(
    api: &Api<ModelDeployment>,
    md: &ModelDeployment,
    live_ready: i32,
    shadow_ready: i32,
) -> Result<(), Error> {
    let status = json!({
        "status": {
            "phase": "Available",
            "liveStatus": { "availableReplicas": live_ready },
            "shadow_status": {"availableReplicas": shadow_ready }
        }
    });
    api.patch_status(
        &md.name_any(),
        &PatchParams::apply("nodel-operator"),
        &Patch::Merge(&status),
    )
    .await?;

    Ok(())
}
