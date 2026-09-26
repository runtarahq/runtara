use runtara_dsl::SchemaField;
use runtara_text_parser::{ParseResult, evaluate_visible_when, sort_fields};
use serde_json::{Map, Value};
use std::collections::HashMap;
use tokio::sync::mpsc;

use super::channel::Channel;
use super::session::InboundMessage;

/// The collector already notified the user; leave the workflow wait unanswered.
#[derive(Debug, thiserror::Error)]
#[error("Field collection stopped without a response")]
pub struct CollectionStopped;

/// Reads replies already buffered by this actor before a wait was discovered.
/// Field progress itself remains in memory; restartable collection is deferred.
pub struct BufferedReplies<'a> {
    pub conn: &'a mut redis::aio::ConnectionManager,
    pub scope: &'a crate::api::services::session_queue::managed::QueueScope,
    pub instance: &'a str,
    /// Only replies received for this request may be consumed as its answers.
    pub request: &'a str,
}

/// Collect structured field values from a user via sequential text prompts.
pub async fn collect_fields<F, Fut>(
    schema_value: &Value,
    channel: &dyn Channel,
    conv_id: &str,
    user_rx: &mut mpsc::Receiver<InboundMessage>,
    intake: Option<&super::intake::IntakeStore>,
    mut buffered: Option<BufferedReplies<'_>>,
    ensure_open: F,
) -> anyhow::Result<Value>
where
    F: Fn() -> Fut + Send + Sync,
    Fut: std::future::Future<Output = anyhow::Result<()>> + Send,
{
    let schema: HashMap<String, SchemaField> = serde_json::from_value(schema_value.clone())?;

    let fields = sort_fields(&schema);
    let mut collected = Map::new();
    let max_retries: u32 = 3;

    for (name, field) in &fields {
        if let Some(vw) = &field.visible_when
            && !evaluate_visible_when(vw, &collected)
        {
            if let Some(default) = &field.default {
                collected.insert(name.to_string(), default.clone());
            }
            continue;
        }

        // Presentation is conditional on authoritative state, not the event
        // which caused this collection attempt. Recheck at every field.
        ensure_open().await?;
        let prompt = runtara_text_parser::build_prompt(name, field);
        channel.send_text(conv_id, &prompt).await?;

        let mut retries = 0;
        loop {
            ensure_open().await?;
            let queued = if let Some(buffer) = buffered.as_mut() {
                use crate::api::services::session_queue;
                if let Some(source) = session_queue::peek_event(
                    buffer.conn,
                    buffer.scope.tenant_id(),
                    buffer.scope.session_id(),
                    Some(buffer.instance),
                )
                .await?
                {
                    anyhow::ensure!(
                        source.event.target.is_none()
                            && source.event.for_request.as_deref() == Some(buffer.request),
                        "Buffered reply belongs to another target"
                    );
                    let text = source
                        .event
                        .payload
                        .get("message")
                        .and_then(Value::as_str)
                        .ok_or_else(|| anyhow::anyhow!("Buffered reply has no text"))?
                        .to_owned();
                    ensure_open().await?;
                    session_queue::acknowledge_event(buffer.conn, &source).await?;
                    if let Some(intake) = intake {
                        intake
                            .settle(
                                uuid::Uuid::parse_str(&source.event.message_id).ok(),
                                "collected",
                                Some(buffer.instance),
                            )
                            .await;
                    }
                    Some(text)
                } else {
                    None
                }
            } else {
                None
            };
            let input = if let Some(text) = queued {
                text
            } else {
                tokio::select! {
                    message = user_rx.recv() => {
                        let message = message.ok_or_else(|| anyhow::anyhow!("Channel closed during field collection"))?;
                        // Consumed for collection; field progress itself is actor-local.
                        if let Some(intake) = intake {
                            intake.settle(message.intake_id, "collected", None).await;
                        }
                        message.text
                    }
                    _ = tokio::time::sleep(std::time::Duration::from_secs(1)) => {
                        ensure_open().await?;
                        continue;
                    }
                }
            };
            ensure_open().await?;

            let trimmed = input.trim();

            if trimmed.eq_ignore_ascii_case("/cancel") {
                channel.send_text(conv_id, "Input cancelled.").await?;
                return Err(CollectionStopped.into());
            }

            if trimmed.eq_ignore_ascii_case("/skip") && !field.required {
                if let Some(default) = &field.default {
                    collected.insert(name.to_string(), default.clone());
                }
                break;
            }

            match runtara_text_parser::parse_text(&input, field) {
                ParseResult::Ok(value) => {
                    collected.insert(name.to_string(), value);
                    break;
                }
                ParseResult::Retry(hint) => {
                    retries += 1;
                    if retries >= max_retries {
                        channel
                            .send_text(
                                conv_id,
                                &format!(
                                    "Too many attempts for '{}'. Cancelling input.",
                                    field.label.as_deref().unwrap_or(name.as_str())
                                ),
                            )
                            .await?;
                        return Err(CollectionStopped.into());
                    }
                    let skip_hint = if !field.required { " (or /skip)" } else { "" };
                    channel
                        .send_text(conv_id, &format!("{}{}", hint, skip_hint))
                        .await?;
                }
            }
        }
    }

    ensure_open().await?;
    Ok(Value::Object(collected))
}
