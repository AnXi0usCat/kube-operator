mod crd;
mod reconsile;

use std::sync::Arc;

use crd::ModelDeployment;
use futures::stream::StreamExt;
use kube::{Api, Client};
use kube_runtime::{Controller, watcher};
use reconsile::{error_policy, reconsile};
use tracing_subscriber::{EnvFilter, fmt};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    fmt().with_env_filter(EnvFilter::from_default_env()).init();

    let client = Client::try_default().await?;
    let api = Api::<ModelDeployment>::all(client.clone());

    Controller::new(api, watcher::Config::default())
        .run(reconsile, error_policy, Arc::new(client))
        .for_each(|res| async move {
            match res {
                Ok(obj) => println!("Reconciled {:?}", obj),
                Err(e) => println!("Reconsile error {:?}", e),
            }
        })
        .await;
    Ok(())
}
