mod crd;
mod error;
mod event;
mod finalizer;
mod reconsile;

use std::sync::Arc;

use crd::ModelDeployment;
use event::{Ctx, make_reporter};
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

    let reporter = make_reporter();
    let recorder = kube_runtime::events::Recorder::new(client.clone(), reporter);
    let ctx = Arc::new(Ctx { client, recorder });

    Controller::new(api, watcher::Config::default())
        .run(reconsile, error_policy, ctx)
        .for_each(|res| async move {
            match res {
                Ok(obj) => println!("Reconciled {:?}", obj),
                Err(e) => println!("Reconsile error {:?}", e),
            }
        })
        .await;
    Ok(())
}
