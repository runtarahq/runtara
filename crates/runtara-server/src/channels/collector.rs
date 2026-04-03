use runtara_dsl::SchemaField;
use runtara_text_parser::{ParseResult, evaluate_visible_when, sort_fields};
use serde_json::{Map, Value};
use std::collections::HashMap;
use tokio::sync::mpsc;

use super::channel::Channel;
use super::session::InboundMessage;

/// Collect structured field values from a user via sequential text prompts.
pub async fn collect_fields(
    schema_value: &Value,
    channel: &dyn Channel,
    conv_id: &str,
    user_rx: &mut mpsc::Receiver<InboundMessage>,
) -> anyhow::Result<Value> {
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

        let prompt = runtara_text_parser::build_prompt(name, field);
        channel.send_text(conv_id, &prompt).await?;

        let mut retries = 0;
        loop {
            let inbound = user_rx
                .recv()
                .await
                .ok_or_else(|| anyhow::anyhow!("Channel closed during field collection"))?;
            let input = &inbound.text;

            let trimmed = input.trim();

            if trimmed.eq_ignore_ascii_case("/cancel") {
                channel.send_text(conv_id, "Input cancelled.").await?;
                anyhow::bail!("Cancelled by user");
            }

            if trimmed.eq_ignore_ascii_case("/skip") && !field.required {
                if let Some(default) = &field.default {
                    collected.insert(name.to_string(), default.clone());
                }
                break;
            }

            match runtara_text_parser::parse_text(input, field) {
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
                        anyhow::bail!("Max retries exceeded for field '{}'", name);
                    }
                    let skip_hint = if !field.required { " (or /skip)" } else { "" };
                    channel
                        .send_text(conv_id, &format!("{}{}", hint, skip_hint))
                        .await?;
                }
            }
        }
    }

    Ok(Value::Object(collected))
}
