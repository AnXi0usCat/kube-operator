use std::{collections::BTreeMap, fmt::Display, sync::Arc, time::Duration};

use crate::{
    crd::{ChildStatus, Condition, ModelDeployment, ModelDeploymentSpec, ModelDeploymentStatus},
    error::Error,
    event::{Ctx, emit_event, with_event},
};
use k8s_openapi::{
    api::{
        apps::v1::{Deployment, DeploymentSpec, DeploymentStrategy, RollingUpdateDeployment},
        core::v1::{
            Container, ContainerPort, PodSpec, PodTemplateSpec, Service, ServicePort, ServiceSpec,
        },
    },
    apimachinery::pkg::{
        apis::meta::v1::{LabelSelector, OwnerReference},
        util::intstr::IntOrString,
    },
};
use kube::Resource;
use kube::{
    Api, Client,
    api::{ApiResource, DynamicObject, ObjectMeta, Patch, PatchParams, PostParams, ResourceExt},
    core::object::HasSpec,
};
use kube_runtime::{controller::Action, events::EventType};
use serde_json::json;

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

pub async fn reconsile(md: Arc<ModelDeployment>, ctx: Arc<Ctx>) -> Result<Action, Error> {
    let ns = md.namespace().unwrap_or_else(|| "default".into());
    let base_name = md.name_any();
    let spec = md.spec();

    println!("Reconciling ModelDeployment {}/{}", ns, base_name);

    let svc_api: Api<Service> = Api::namespaced(ctx.client.clone(), &ns);
    with_event(
        &ctx,
        &*md,
        "Created live Service",
        "LiveSvcFailed",
        ensure_service(&svc_api, &md, &base_name, DeploymentType::Live),
    )
    .await?;

    if spec.shadow.is_some() {
        with_event(
            &ctx,
            &*md,
            "Created shadow Service",
            "ShadowSvcFailed",
            ensure_service(&svc_api, &md, &base_name, DeploymentType::Shadow),
        )
        .await?;
    }

    let deployment_api: Api<Deployment> = Api::namespaced(ctx.client.clone(), &ns);
    with_event(
        &ctx,
        &*md,
        "Created live Deployment",
        "LiveDeploymentFailed",
        ensure_deployment(
            &deployment_api,
            &md,
            &format!("{}-live", base_name),
            &base_name,
            &spec.live.image,
            spec.live.replicas,
            DeploymentType::Live,
        ),
    )
    .await?;

    if let Some(shadow) = &spec.shadow {
        with_event(
            &ctx,
            &*md,
            "Created shadow Deployment",
            "ShadowDeploymentFailed",
            ensure_deployment(
                &deployment_api,
                &md,
                &format!("{}-shadow", base_name),
                &base_name,
                &shadow.image,
                shadow.replicas,
                DeploymentType::Shadow,
            ),
        )
        .await?;
    }

    if spec.traffic_mirror {
        let ts_api = traefik_service_api(ctx.client.clone(), &ns);
        with_event(
            &ctx,
            &*md,
            "Created Traefik Service",
            "TraefikServiceFailed",
            ensure_traefik_service(&ts_api, &md, &base_name),
        )
        .await?;

        let ir_api = ingress_route_api(ctx.client.clone(), &ns);
        with_event(
            &ctx,
            &*md,
            "Created Ingress Route",
            "IngressRouteFailed",
            ensure_ingress_route(&ir_api, &md, &base_name),
        )
        .await?;
    }

    let (live_status, shadow_status) = get_child_status(&ctx.client, &base_name, &ns).await?;
    let model_deployment_status =
        compute_model_deployment_status(spec, &live_status, &shadow_status).await;
    update_status(&ctx.client, &md, &ns, &model_deployment_status).await?;
    emit_event(
        &ctx,
        &*md,
        "Reconciled",
        "Reconciliation completed",
        EventType::Normal,
    )
    .await?;

    Ok(Action::requeue(Duration::from_secs(60)))
}

pub fn error_policy(_object: Arc<ModelDeployment>, _error: &Error, _ctx: Arc<Ctx>) -> Action {
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

async fn ensure_traefik_service(
    api: &Api<DynamicObject>,
    md: &ModelDeployment,
    base_name: &str,
) -> Result<(), Error> {
    let ts_name = base_name.to_string();

    if api.get_opt(&ts_name).await?.is_some() {
        return Ok(());
    }

    let live_svc_name = format!("{}-live-svc", base_name);
    let shadow_svc_name = format!("{}-shadow-svc", base_name);

    let md_owner = owner_ref(md);

    let data = json!({
        "apiVersion": "traefik.containo.us/v1alpha1",
        "kind": "TraefikService",
        "metadata": {
            "name": ts_name,
            "ownerReferences": [md_owner],
        },
        "spec": {
            "mirroring": {
                "name": live_svc_name,
                "kind": "Service",
                "port": 8000,
                "mirrors": [
                    {
                        "name": shadow_svc_name,
                        "kind": "Service",
                        "port": 8000,
                        "percent": 100
                    }
                ]
            }
        }
    });

    // DynamicObject::new sets kind/apiVersion from ApiResource, but we already
    // included those in `data` for clarity. kube will merge them.
    let obj = DynamicObject {
        types: None,
        metadata: ObjectMeta::default(), // will be filled from `data`
        data,
    };

    api.create(&PostParams::default(), &obj).await?;
    println!("created TraefikService {}", ts_name);
    Ok(())
}

async fn ensure_ingress_route(
    api: &Api<DynamicObject>,
    md: &ModelDeployment,
    base_name: &str,
) -> Result<(), Error> {
    let ir_name = base_name.to_string();

    if api.get_opt(&ir_name).await?.is_some() {
        return Ok(());
    }

    let md_owner = owner_ref(md);

    let host_rule = format!("Host(`{}.{}`)", base_name, "local");

    let data = json!({
        "apiVersion": "traefik.containo.us/v1alpha1",
        "kind": "IngressRoute",
        "metadata": {
            "name": ir_name,
            "ownerReferences": [md_owner],
        },
        "spec": {
            "entryPoints": ["web"],
            "routes": [
                {
                    "match": host_rule,
                    "kind": "Rule",
                    "services": [
                        {
                            "name": base_name,
                            "kind": "TraefikService",
                        }
                    ]
                }
            ]
        }
    });

    let obj = DynamicObject {
        types: None,
        metadata: ObjectMeta::default(),
        data,
    };

    api.create(&PostParams::default(), &obj).await?;
    println!("created IngressRoute {}", ir_name);
    Ok(())
}

fn traefik_service_api(client: Client, ns: &str) -> Api<DynamicObject> {
    let ar = ApiResource::from_gvk_with_plural(
        &kube::core::gvk::GroupVersionKind::gvk(
            "traefik.containo.us",
            "v1alpha1",
            "TraefikService",
        ),
        "traefikservices",
    );
    Api::namespaced_with(client, ns, &ar)
}

fn ingress_route_api(client: Client, ns: &str) -> Api<DynamicObject> {
    let ar = ApiResource::from_gvk_with_plural(
        &kube::core::gvk::GroupVersionKind::gvk("traefik.containo.us", "v1alpha1", "IngressRoute"),
        "ingressroutes",
    );
    Api::namespaced_with(client, ns, &ar)
}

async fn update_status(
    client: &Client,
    md: &ModelDeployment,
    ns: &str,
    status: &ModelDeploymentStatus,
) -> Result<(), Error> {
    let api: Api<ModelDeployment> = Api::namespaced(client.clone(), ns);

    let patch = json!({
        "status": status
    });

    api.patch_status(
        &md.name_any(),
        &PatchParams::default(),
        &Patch::Merge(&patch),
    )
    .await?;

    Ok(())
}

async fn get_child_status(
    client: &Client,
    base_name: &str,
    ns: &str,
) -> Result<(Option<ChildStatus>, Option<ChildStatus>), Error> {
    let deploy_api: Api<Deployment> = Api::namespaced(client.clone(), ns);

    let live_name = format!("{}-live", base_name);
    let shadow_name = format!("{}-shadow", base_name);

    fn convert_to_child_status(deployment: &Deployment) -> ChildStatus {
        let status = deployment.status.as_ref();

        ChildStatus {
            available_replicas: status.and_then(|st| st.available_replicas),
            updated_replicas: status.and_then(|st| st.updated_replicas),
        }
    }

    let live_status = match deploy_api.get_opt(&live_name).await? {
        Some(dep) => Some(convert_to_child_status(&dep)),
        None => None,
    };

    let shadow_status = match deploy_api.get_opt(&shadow_name).await? {
        Some(dep) => Some(convert_to_child_status(&dep)),
        None => None,
    };

    Ok((live_status, shadow_status))
}

async fn compute_model_deployment_status(
    spec: &ModelDeploymentSpec,
    live: &Option<ChildStatus>,
    shadow: &Option<ChildStatus>,
) -> ModelDeploymentStatus {
    // helper
    fn availabld_replicas(cs: &Option<ChildStatus>) -> i32 {
        cs.as_ref().and_then(|s| s.available_replicas).unwrap_or(0)
    }

    let live_available = availabld_replicas(live);
    let live_desired = spec.live.replicas;

    let shadow_available = availabld_replicas(shadow);
    let shadow_desired = spec.shadow.as_ref().map(|r| r.replicas).unwrap_or(0);

    // calculate Phase of deployment
    let phase = if live_available == live_desired
        && (spec.shadow.is_none() || shadow_available == shadow_desired)
    {
        Some("Available".into())
    } else if live_available == 0 && live_desired > 0 {
        Some("Degraded".into())
    } else {
        Some("Progressing".into())
    };

    // create Conditions
    let mut conditions = Vec::with_capacity(3);

    let ready = live_available == live_desired
        && (spec.shadow.is_none() || shadow_available == shadow_desired);

    conditions.push(Condition {
        r#type: "Ready".into(),
        status: if ready { "True".into() } else { "False".into() },
        reason: Some(if ready {
            "AllReplicasAvailable".into()
        } else {
            "ReplicasNotReady".into()
        }),
        message: Some(format!(
            "live {}/{} shadow {}/{} available",
            live_available, live_desired, shadow_available, shadow_desired
        )),
    });

    let progressing = !ready;
    conditions.push(Condition {
        r#type: "Progressing".into(),
        status: if progressing {
            "True".into()
        } else {
            "False".into()
        },
        reason: Some("Reconciling".into()),
        message: Some("Deployment is rolling out or scaling.".into()),
    });

    let degraded = live_available == 0 && live_desired > 0;
    conditions.push(Condition {
        r#type: "Degraded".into(),
        status: if degraded {
            "True".into()
        } else {
            "False".into()
        },
        reason: Some("NoAvailableReplicas".into()),
        message: Some("No live replicas are currently available.".into()),
    });

    ModelDeploymentStatus {
        phase,
        live_status: live.clone(),
        shadow_status: shadow.clone(),
        conditions: Some(conditions),
    }
}
