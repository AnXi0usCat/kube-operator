use std::{collections::BTreeMap, fmt::Display, sync::Arc, time::Duration};

use crate::{
    crd::{ChildStatus, Condition, ModelDeployment, ModelDeploymentSpec, ModelDeploymentStatus},
    error::Error,
    event::{Ctx, Outcome, emit_event, with_event},
    finalizer::{
        FINALIZER, ensure_finalizer_present, has_finalizer, is_deleting, remove_finalizer,
    },
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
use kcr_traefik_io::v1alpha1::{
    ingressroutes::{
        IngressRoute, IngressRouteRoutes, IngressRouteRoutesKind, IngressRouteRoutesServices,
        IngressRouteRoutesServicesKind, IngressRouteSpec,
    },
    traefikservices::{
        TraefikService, TraefikServiceMirroring, TraefikServiceMirroringKind,
        TraefikServiceMirroringMirrors, TraefikServiceMirroringMirrorsKind, TraefikServiceSpec,
    },
};
use kube::{
    Api, Client,
    api::{ObjectMeta, Patch, PatchParams},
    core::object::HasSpec,
};
use kube::{Resource, ResourceExt};
use kube_runtime::{controller::Action, events::EventType};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::json;
use sha2::{Digest, Sha256};

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

    tracing::info!("Reconciling ModelDeployment {}/{}", ns, base_name);
    let mut changed = false;

    if is_deleting(&md) {
        if has_finalizer(&md, FINALIZER) {
            emit_event(
                &ctx,
                &*md,
                "Finalizing",
                "Deletion requested; running finalizer.",
                EventType::Normal,
            )
            .await?;
            let _ = with_event(
                &ctx,
                &*md,
                "Finalizer complete; allowing deletion.",
                "Finalized",
                "FinalizingFailed",
                remove_finalizer(&ctx.client, &md, &ns, FINALIZER),
            )
            .await?;
        }
        return Ok(Action::await_change());
    }

    let out = with_event(
        &ctx,
        &*md,
        "Created finalizer for ModelDeployment",
        "FinalizerCreated",
        "FinalizerFailed",
        ensure_finalizer_present(&ctx.client, &md, &ns, FINALIZER),
    )
    .await?;
    changed |= out != Outcome::NoOp;

    let svc_api: Api<Service> = Api::namespaced(ctx.client.clone(), &ns);
    let out = with_event(
        &ctx,
        &*md,
        "Created live svc for ModelDeployment",
        "LiveSvcCreated",
        "LiveSvcFailed",
        ensure_service(&svc_api, &md, &base_name, DeploymentType::Live),
    )
    .await?;
    changed |= out != Outcome::NoOp;

    if spec.shadow.is_some() {
        let out = with_event(
            &ctx,
            &*md,
            "Created shadow svc for ModelDeployment",
            "ShadowSvcCreated",
            "ShadowSvcFailed",
            ensure_service(&svc_api, &md, &base_name, DeploymentType::Shadow),
        )
        .await?;
        changed |= out != Outcome::NoOp;
    }

    let deployment_api: Api<Deployment> = Api::namespaced(ctx.client.clone(), &ns);
    let out = with_event(
        &ctx,
        &*md,
        "Created live Deployment",
        "LiveDeploymentCreated",
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
    changed |= out != Outcome::NoOp;

    if let Some(shadow) = &spec.shadow {
        let out = with_event(
            &ctx,
            &*md,
            "Created shadow Deployment",
            "ShadowDeploymentCreated",
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
        changed |= out != Outcome::NoOp;
    }

    if spec.traffic_mirror {
        let ts_api: Api<TraefikService> = Api::namespaced(ctx.client.clone(), &ns);
        let out = with_event(
            &ctx,
            &*md,
            "Created Traefik Service",
            "TraefikServiceCreated",
            "TraefikServiceFailed",
            ensure_traefik_service(&ts_api, &md, &base_name, &ns),
        )
        .await?;
        changed |= out != Outcome::NoOp;

        let ir_api: Api<IngressRoute> = Api::namespaced(ctx.client.clone(), &ns);
        let out = with_event(
            &ctx,
            &*md,
            "Created Ingress Route",
            "IngressRouteCreated",
            "IngressRouteFailed",
            ensure_ingress_route(&ir_api, &md, &base_name, &ns),
        )
        .await?;
        changed |= out != Outcome::NoOp;
    }

    let (live_status, shadow_status) = get_child_status(&ctx.client, &base_name, &ns).await?;
    let model_deployment_status =
        compute_model_deployment_status(spec, &live_status, &shadow_status).await;
    update_status(&ctx.client, &md, &ns, &model_deployment_status).await?;

    if changed {
        emit_event(
            &ctx,
            &*md,
            "Reconciled",
            "Reconciliation completed",
            EventType::Normal,
        )
        .await?;
    }

    tracing::info!("Reconsiliation completed.");

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
) -> Result<Outcome, Error> {
    let svc_name = format!("{}-{}-svc", base_name, role);

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

    let result = reconsile_resource(api, &svc).await?;
    tracing::info!("Created Service {:?}", svc_name);

    Ok(result)
}

async fn ensure_deployment(
    api: &Api<Deployment>,
    md: &ModelDeployment,
    deployment_name: &str,
    base_name: &str,
    image: &str,
    replicas: i32,
    role: DeploymentType,
) -> Result<Outcome, Error> {
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
    let result = reconsile_resource(api, &deploy).await?;
    if result != Outcome::NoOp {
        tracing::info!("Created Deployment: {}", deployment_name);
    }

    Ok(result)
}

async fn ensure_traefik_service(
    api: &Api<TraefikService>,
    md: &ModelDeployment,
    base_name: &str,
    ns: &str,
) -> Result<Outcome, Error> {
    let ts_name = base_name.to_string();

    let live_svc_name = format!("{}-live-svc", base_name);
    let shadow_svc_name = format!("{}-shadow-svc", base_name);

    let obj = TraefikService {
        metadata: ObjectMeta {
            name: Some(ts_name.clone()),
            namespace: Some(ns.into()),
            owner_references: Some(vec![owner_ref(md)]),
            ..Default::default()
        },
        spec: TraefikServiceSpec {
            mirroring: Some(TraefikServiceMirroring {
                name: live_svc_name,
                kind: Some(TraefikServiceMirroringKind::Service),
                port: Some(IntOrString::Int(8000)),
                mirrors: Some(vec![TraefikServiceMirroringMirrors {
                    name: shadow_svc_name,
                    kind: Some(TraefikServiceMirroringMirrorsKind::Service),
                    port: Some(IntOrString::Int(8000)),
                    percent: Some(100),
                    ..Default::default()
                }]),
                ..Default::default()
            }),
            ..Default::default()
        },
    };

    let result = reconsile_resource(api, &obj).await?;
    if result != Outcome::NoOp {
    tracing::info!("created TraefikService {}", ts_name);
    }
    Ok(result)
}

async fn ensure_ingress_route(
    api: &Api<IngressRoute>,
    md: &ModelDeployment,
    base_name: &str,
    ns: &str,
) -> Result<Outcome, Error> {
    let ir_name = base_name.to_string();
    let host_rule = format!("Host(`{}.{}`)", base_name, "local");

    let obj = IngressRoute {
        metadata: ObjectMeta {
            name: Some(ir_name.clone()),
            namespace: Some(ns.into()),
            owner_references: Some(vec![owner_ref(md)]),
            ..Default::default()
        },
        spec: IngressRouteSpec {
            entry_points: Some(vec!["web".into()]),
            routes: vec![IngressRouteRoutes {
                kind: Some(IngressRouteRoutesKind::Rule),
                r#match: host_rule,
                services: Some(vec![IngressRouteRoutesServices {
                    name: base_name.into(),
                    kind: Some(IngressRouteRoutesServicesKind::TraefikService),
                    port: Some(IntOrString::Int(8000)),
                    ..Default::default()
                }]),
                ..Default::default()
            }],
            ..Default::default()
        },
    };

    let result = reconsile_resource(api, &obj).await?;
    if result != Outcome::NoOp {
    tracing::info!("created IngressRoute {}", ir_name);
    }
    Ok(result)
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

async fn reconsile_resource<K>(api: &Api<K>, desired: &K) -> Result<Outcome, Error>
where
    K: Resource + std::fmt::Debug + Clone + serde::Serialize + DeserializeOwned,
{
    const FP_ANN: &str = "ml.jedimindtricks.example/desired-fingerprint";

    pub fn desired_fingerprint<T: Serialize>(t: &T) -> String {
        let json = serde_json::to_string(t).unwrap_or_default();

        let mut hasher = Sha256::new();
        hasher.update(json.as_bytes());

        let hash = hasher.finalize();
        format!("{:x}", hash)
    }

    let name = desired.name_any();
    let existing = api.get_opt(&name).await?;
    let fp = desired_fingerprint(&desired);

    if let Some(ref resource) = existing {
        if let Some(ref anno) = resource.meta().annotations {
            if let Some(old) = anno.get(FP_ANN) {
                if old == &fp {
                    return Ok(Outcome::NoOp);
                }
            }
        }
    }

    let mut desired = desired.clone();
    desired
        .meta_mut()
        .annotations
        .get_or_insert_with(|| Default::default())
        .insert(FP_ANN.into(), fp);

    let pp = PatchParams::apply("model-operator");
    api.patch(&name, &pp, &Patch::Apply(&desired)).await?;

    Ok(if existing.is_none() {
        Outcome::Created
    } else {
        Outcome::Updated
    })
}
