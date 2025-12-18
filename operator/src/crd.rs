use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(CustomResource, Serialize, Deserialize, Debug, Clone, JsonSchema, Default)]
#[serde(rename_all = "camelCase")]
#[kube(
    group = "ml.jedimindtricks.example",
    version = "v1alpha1",
    kind = "ModelDeployment",
    plural = "modeldeployments",
    derive = "Default",
    status = "ModelDeploymentStatus",
    shortname = "md",
    namespaced
)]
pub struct ModelDeploymentSpec {
    pub live: ModelVariant,
    pub shadow: Option<ModelVariant>,

    #[serde(default)]
    pub traffic_mirror: bool,

    #[serde(default = "default_rollout")]
    pub rollout_strategy: String,

    #[serde(default)]
    pub resources: Option<ResourceSpec>,

    #[serde(default)]
    pub autoscaling: Option<AutoScalingSpec>,

    #[serde(default)]
    pub probes: Option<ProbeSpec>,

    #[serde(default)]
    pub config_ref: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, JsonSchema, Default)]
#[serde(rename_all = "camelCase")]
pub struct ModelVariant {
    pub image: String,
    #[serde(default = "default_replicas")]
    pub replicas: i32,
}

#[derive(Serialize, Deserialize, Debug, Clone, JsonSchema, Default)]
#[serde(rename_all = "camelCase")]
pub struct ResourceSpec {
    pub limits: Option<ResourceLimits>,
    pub requests: Option<ResourceLimits>,
}

#[derive(Serialize, Deserialize, Debug, Clone, JsonSchema, Default)]
#[serde(rename_all = "camelCase")]
pub struct ResourceLimits {
    pub cpu: Option<String>,
    pub memory: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, JsonSchema, Default)]
#[serde(rename_all = "camelCase")]
pub struct AutoScalingSpec {
    pub enabled: bool,
    pub min_replicas: Option<i32>,
    pub max_replicas: Option<i32>,
    pub target_cpu_utilization_percentage: Option<i32>,
}

#[derive(Serialize, Deserialize, Debug, Clone, JsonSchema, Default)]
#[serde(rename_all = "camelCase")]
pub struct ProbeSpec {
    #[serde(default = "default_liveness")]
    pub liveness_path: String,
    #[serde(default = "default_readiness")]
    pub readiness_path: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, JsonSchema, Default)]
#[serde(rename_all = "camelCase")]
pub struct ModelDeploymentStatus {
    pub phase: Option<String>,
    pub live_status: Option<ChildStatus>,
    pub shadow_status: Option<ChildStatus>,
    pub conditions: Option<Vec<Condition>>,
}

#[derive(Serialize, Deserialize, Debug, Clone, JsonSchema, Default)]
#[serde(rename_all = "camelCase")]
pub struct ChildStatus {
    pub available_replicas: Option<i32>,
    pub updated_replicas: Option<i32>,
}

#[derive(Serialize, Deserialize, Debug, Clone, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Condition {
    pub r#type: String,
    pub status: String,
    pub reason: Option<String>,
    pub message: Option<String>,
}

fn default_replicas() -> i32 {
    1
}
fn default_rollout() -> String {
    "rolling".into()
}
fn default_liveness() -> String {
    "/health".into()
}
fn default_readiness() -> String {
    "/ready".into()
}
