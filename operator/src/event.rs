use kube::runtime::events::{Event, EventType};
use kube::{Client, Resource};
use kube_runtime::events::{Recorder, Reporter};

use crate::error::Error;

#[derive(Clone)]
pub struct Ctx {
    pub client: Client,
    pub recorder: Recorder,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    NoOp,
    Created,
    Updated,
}

pub fn make_reporter() -> Reporter {
    Reporter {
        controller: "model-operator".into(),
        instance: Some("dev".into()),
    }
}

pub async fn emit_event<K>(
    ctx: &Ctx,
    obj: &K,
    reason: &str,
    note: &str,
    event_type: EventType,
) -> Result<(), Error>
where
    K: Resource<DynamicType = ()> + std::fmt::Debug,
{
    ctx.recorder
        .publish(
            &Event {
                type_: event_type,
                reason: reason.into(),
                note: Some(note.into()),
                action: reason.into(),
                secondary: None,
            },
            &obj.object_ref(&()),
        )
        .await?;

    Ok(())
}

pub async fn with_event<E, K>(
    ctx: &Ctx,
    obj: &K,
    success_msg: &str,
    success_reason: &str,
    fail_reason: &str,
    op: impl std::future::Future<Output = Result<Outcome, E>>,
) -> Result<Outcome, E>
where
    E: std::fmt::Display,
    K: Resource<DynamicType = ()> + std::fmt::Debug,
{
    match op.await {
        Ok(outcome) => {
            match outcome {
                Outcome::Created | Outcome::Updated => {
                    let _ =
                        emit_event(ctx, obj, success_reason, success_msg, EventType::Normal).await;
                }
                Outcome::NoOp => {}
            }
            Ok(outcome)
        }
        Err(e) => {
            let _ = emit_event(ctx, obj, fail_reason, &e.to_string(), EventType::Warning).await;
            Err(e)
        }
    }
}
