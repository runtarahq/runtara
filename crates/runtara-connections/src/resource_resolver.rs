//! Read-only, connection-backed resource discovery.
//!
//! The semantic resource key (`llm.models`, `messaging.queues`, …) is mapped
//! to a resolver by the connection descriptor. This module owns the resolver
//! registry and provider-specific wire protocols; callers only deal in the
//! generic request/page contracts from [`crate::resolution`].

use std::collections::HashMap;

use chrono::{Duration, Utc};
use reqwest::header::{ACCEPT, AUTHORIZATION};
use serde_json::{Map, Value, json};

use crate::auth::aws_signing::sign_request_v4;
use crate::error::ConnectionsError;
use crate::resolution::{
    ConnectionResourceItem, ConnectionResourcePage, ConnectionResourceRequest,
};

const OPENAI_MODELS_RESOLVER: &str = "openai.models";
const BEDROCK_MODELS_RESOLVER: &str = "aws.bedrock.models";
const SQS_QUEUES_RESOLVER: &str = "aws.sqs.queues";
const MAX_RESOURCE_LIMIT: usize = 1_000;

/// Dispatch one registered resource resolver.
///
/// Resolver ids are declared by connection features, rather than inferred
/// from a workflow step. Adding another connection-backed resource therefore
/// only requires a feature declaration and one registry implementation here.
pub async fn resolve_resource(
    client: &reqwest::Client,
    resolver_id: &str,
    parameters: &Value,
    request: &ConnectionResourceRequest,
) -> Result<ConnectionResourcePage, ConnectionsError> {
    match resolver_id {
        OPENAI_MODELS_RESOLVER => resolve_openai_models(client, parameters, request).await,
        BEDROCK_MODELS_RESOLVER => resolve_bedrock_models(client, parameters, request).await,
        SQS_QUEUES_RESOLVER => resolve_sqs_queues(client, parameters, request).await,
        other => Err(ConnectionsError::Validation(format!(
            "Connection resource resolver '{other}' is not registered"
        ))),
    }
}

fn resource_page(
    items: Vec<ConnectionResourceItem>,
    next_cursor: Option<String>,
) -> ConnectionResourcePage {
    let fetched_at = Utc::now();
    ConnectionResourcePage {
        items,
        next_cursor,
        fetched_at: fetched_at.to_rfc3339(),
        expires_at: Some((fetched_at + Duration::minutes(5)).to_rfc3339()),
        stale: false,
    }
}

fn request_limit(request: &ConnectionResourceRequest) -> usize {
    request
        .limit
        .map(|limit| limit as usize)
        .unwrap_or(MAX_RESOURCE_LIMIT)
        .min(MAX_RESOURCE_LIMIT)
}

fn string_argument<'a>(request: &'a ConnectionResourceRequest, key: &str) -> Option<&'a str> {
    request
        .arguments
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn parameter<'a>(parameters: &'a Value, names: &[&str]) -> Option<&'a str> {
    names
        .iter()
        .find_map(|name| parameters.get(*name).and_then(Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn require_parameter<'a>(
    parameters: &'a Value,
    names: &[&str],
    display_name: &str,
) -> Result<&'a str, ConnectionsError> {
    parameter(parameters, names).ok_or_else(|| {
        ConnectionsError::AuthResolution(format!(
            "Connection is missing required {display_name} credentials"
        ))
    })
}

fn append_path(base_url: &str, segment: &str) -> Result<reqwest::Url, ConnectionsError> {
    let mut url = reqwest::Url::parse(base_url).map_err(|error| {
        ConnectionsError::Validation(format!("Connection endpoint is invalid: {error}"))
    })?;
    let mut segments = url.path_segments_mut().map_err(|_| {
        ConnectionsError::Validation("Connection endpoint must be a hierarchical URL".into())
    })?;
    segments.pop_if_empty();
    segments.push(segment);
    drop(segments);
    Ok(url)
}

async fn response_json(
    response: reqwest::Response,
    provider: &str,
) -> Result<Value, ConnectionsError> {
    let status = response.status();
    let bytes = response.bytes().await.map_err(|error| {
        ConnectionsError::AuthResolution(format!("Failed to read {provider} response: {error}"))
    })?;
    if !status.is_success() {
        let detail = String::from_utf8_lossy(&bytes);
        let detail: String = detail.chars().take(512).collect();
        return Err(ConnectionsError::AuthResolution(format!(
            "{provider} resource discovery failed with HTTP {status}: {detail}"
        )));
    }
    serde_json::from_slice(&bytes).map_err(|error| {
        ConnectionsError::AuthResolution(format!(
            "{provider} resource discovery returned invalid JSON: {error}"
        ))
    })
}

async fn resolve_openai_models(
    client: &reqwest::Client,
    parameters: &Value,
    request: &ConnectionResourceRequest,
) -> Result<ConnectionResourcePage, ConnectionsError> {
    let api_key = require_parameter(parameters, &["api_key"], "OpenAI API key")?;
    let base_url = parameter(parameters, &["base_url"]).unwrap_or("https://api.openai.com/v1");
    let url = append_path(base_url, "models")?;
    let mut builder = client
        .get(url)
        .header(AUTHORIZATION, format!("Bearer {api_key}"))
        .header(ACCEPT, "application/json");
    if let Some(organization_id) = parameter(parameters, &["organization_id"]) {
        builder = builder.header("OpenAI-Organization", organization_id);
    }
    let response = builder.send().await.map_err(|error| {
        ConnectionsError::AuthResolution(format!("OpenAI model discovery failed: {error}"))
    })?;
    let body = response_json(response, "OpenAI").await?;
    let query = string_argument(request, "query").map(str::to_ascii_lowercase);
    let mut models: Vec<&Value> = body
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            ConnectionsError::AuthResolution(
                "OpenAI model discovery response is missing 'data'".into(),
            )
        })?
        .iter()
        .filter(|model| {
            let id = model.get("id").and_then(Value::as_str).unwrap_or_default();
            query
                .as_ref()
                .is_none_or(|query| id.to_ascii_lowercase().contains(query))
        })
        .collect();
    models.sort_by_key(|model| model.get("id").and_then(Value::as_str).unwrap_or_default());
    let items = models
        .into_iter()
        .take(request_limit(request))
        .filter_map(|model| {
            let id = model.get("id")?.as_str()?;
            Some(ConnectionResourceItem {
                value: Value::String(id.to_string()),
                label: id.to_string(),
                description: model
                    .get("owned_by")
                    .and_then(Value::as_str)
                    .map(|owner| format!("Owned by {owner}")),
                metadata: model.clone(),
            })
        })
        .collect();
    Ok(resource_page(items, None))
}

struct AwsCredentials<'a> {
    access_key_id: &'a str,
    secret_access_key: &'a str,
    session_token: Option<&'a str>,
    region: &'a str,
    endpoint: Option<&'a str>,
}

fn aws_credentials(parameters: &Value) -> Result<AwsCredentials<'_>, ConnectionsError> {
    Ok(AwsCredentials {
        access_key_id: require_parameter(
            parameters,
            &["access_key_id", "aws_access_key_id"],
            "AWS access key id",
        )?,
        secret_access_key: require_parameter(
            parameters,
            &["secret_access_key", "aws_secret_access_key"],
            "AWS secret access key",
        )?,
        session_token: parameter(parameters, &["session_token", "aws_session_token"]),
        region: parameter(parameters, &["region", "aws_region"]).unwrap_or("us-east-1"),
        endpoint: parameter(parameters, &["endpoint"]),
    })
}

async fn signed_aws_json(
    client: &reqwest::Client,
    method: reqwest::Method,
    url: reqwest::Url,
    service: &str,
    credentials: &AwsCredentials<'_>,
    body: &[u8],
    additional_headers: &[(&str, &str)],
) -> Result<Value, ConnectionsError> {
    let mut headers = HashMap::new();
    headers.insert("Content-Type".to_string(), "application/json".to_string());
    for (name, value) in additional_headers {
        headers.insert((*name).to_string(), (*value).to_string());
    }
    sign_request_v4(
        method.as_str(),
        &url,
        &mut headers,
        body,
        credentials.access_key_id,
        credentials.secret_access_key,
        credentials.region,
        service,
        credentials.session_token,
    );

    let mut builder = client.request(method, url).body(body.to_vec());
    for (name, value) in headers {
        builder = builder.header(name, value);
    }
    let response = builder.send().await.map_err(|error| {
        ConnectionsError::AuthResolution(format!("AWS resource discovery failed: {error}"))
    })?;
    response_json(response, "AWS").await
}

async fn resolve_bedrock_models(
    client: &reqwest::Client,
    parameters: &Value,
    request: &ConnectionResourceRequest,
) -> Result<ConnectionResourcePage, ConnectionsError> {
    let credentials = aws_credentials(parameters)?;
    let base_url = credentials
        .endpoint
        .map(str::to_string)
        .unwrap_or_else(|| format!("https://bedrock.{}.amazonaws.com", credentials.region));
    let mut url = append_path(&base_url, "foundation-models")?;
    {
        let mut query = url.query_pairs_mut();
        for (argument, provider_field) in [
            ("provider", "byProvider"),
            ("outputModality", "byOutputModality"),
            ("inferenceType", "byInferenceType"),
            ("customizationType", "byCustomizationType"),
        ] {
            if let Some(value) = string_argument(request, argument) {
                query.append_pair(provider_field, value);
            }
        }
    }
    let body = signed_aws_json(
        client,
        reqwest::Method::GET,
        url,
        "bedrock",
        &credentials,
        &[],
        &[],
    )
    .await?;
    let mut models: Vec<&Value> = body
        .get("modelSummaries")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            ConnectionsError::AuthResolution(
                "Bedrock model discovery response is missing 'modelSummaries'".into(),
            )
        })?
        .iter()
        .collect();
    models.sort_by_key(|model| {
        model
            .get("modelName")
            .or_else(|| model.get("modelId"))
            .and_then(Value::as_str)
            .unwrap_or_default()
    });
    let items = models
        .into_iter()
        .take(request_limit(request))
        .filter_map(|model| {
            let id = model.get("modelId")?.as_str()?;
            let label = model
                .get("modelName")
                .and_then(Value::as_str)
                .unwrap_or(id)
                .to_string();
            Some(ConnectionResourceItem {
                value: Value::String(id.to_string()),
                label,
                description: model
                    .get("providerName")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                metadata: model.clone(),
            })
        })
        .collect();
    Ok(resource_page(items, None))
}

async fn resolve_sqs_queues(
    client: &reqwest::Client,
    parameters: &Value,
    request: &ConnectionResourceRequest,
) -> Result<ConnectionResourcePage, ConnectionsError> {
    let credentials = aws_credentials(parameters)?;
    let endpoint = credentials
        .endpoint
        .map(str::to_string)
        .unwrap_or_else(|| format!("https://sqs.{}.amazonaws.com", credentials.region));
    let url = reqwest::Url::parse(&endpoint).map_err(|error| {
        ConnectionsError::Validation(format!("Connection endpoint is invalid: {error}"))
    })?;
    let mut payload = Map::new();
    if let Some(prefix) = string_argument(request, "queueNamePrefix") {
        payload.insert("QueueNamePrefix".into(), Value::String(prefix.to_string()));
    }
    if let Some(cursor) = request.cursor.as_deref() {
        payload.insert("NextToken".into(), Value::String(cursor.to_string()));
    }
    payload.insert(
        "MaxResults".into(),
        json!(request_limit(request).clamp(1, MAX_RESOURCE_LIMIT)),
    );
    let payload = serde_json::to_vec(&Value::Object(payload)).map_err(|error| {
        ConnectionsError::Internal(format!("Failed to encode SQS request: {error}"))
    })?;
    let body = signed_aws_json(
        client,
        reqwest::Method::POST,
        url,
        "sqs",
        &credentials,
        &payload,
        &[
            ("Content-Type", "application/x-amz-json-1.0"),
            ("X-Amz-Target", "AmazonSQS.ListQueues"),
        ],
    )
    .await?;
    let items = body
        .get("QueueUrls")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(|queue_url| {
            let label = queue_url
                .trim_end_matches('/')
                .rsplit('/')
                .next()
                .unwrap_or(queue_url)
                .to_string();
            ConnectionResourceItem {
                value: Value::String(queue_url.to_string()),
                label,
                description: None,
                metadata: Value::Null,
            }
        })
        .collect();
    let next_cursor = body
        .get("NextToken")
        .and_then(Value::as_str)
        .map(str::to_string);
    Ok(resource_page(items, next_cursor))
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{body_json, header, header_exists, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn request(resource: &str) -> ConnectionResourceRequest {
        ConnectionResourceRequest {
            resource: resource.to_string(),
            arguments: Value::Null,
            cursor: None,
            limit: None,
            refresh: false,
        }
    }

    #[tokio::test]
    async fn openai_models_are_normalized_and_filtered() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .and(header("authorization", "Bearer secret"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [
                    {"id": "gpt-4o-mini", "owned_by": "openai"},
                    {"id": "text-embedding-3-small", "owned_by": "openai"}
                ]
            })))
            .mount(&server)
            .await;
        let mut request = request("llm.models");
        request.arguments = json!({"query": "gpt"});

        let page = resolve_resource(
            &reqwest::Client::new(),
            OPENAI_MODELS_RESOLVER,
            &json!({"api_key": "secret", "base_url": format!("{}/v1", server.uri())}),
            &request,
        )
        .await
        .unwrap();

        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].value, "gpt-4o-mini");
        assert_eq!(page.items[0].label, "gpt-4o-mini");
        assert!(!page.stale);
    }

    #[tokio::test]
    async fn bedrock_models_use_control_plane_and_normalize_ids() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/foundation-models"))
            .and(query_param("byProvider", "Anthropic"))
            .and(header_exists("authorization"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "modelSummaries": [{
                    "modelId": "anthropic.claude-3-haiku",
                    "modelName": "Claude 3 Haiku",
                    "providerName": "Anthropic"
                }]
            })))
            .mount(&server)
            .await;
        let mut request = request("llm.models");
        request.arguments = json!({"provider": "Anthropic"});

        let page = resolve_resource(
            &reqwest::Client::new(),
            BEDROCK_MODELS_RESOLVER,
            &json!({
                "aws_access_key_id": "key",
                "aws_secret_access_key": "secret",
                "aws_region": "us-east-1",
                "endpoint": server.uri()
            }),
            &request,
        )
        .await
        .unwrap();

        assert_eq!(page.items[0].value, "anthropic.claude-3-haiku");
        assert_eq!(page.items[0].label, "Claude 3 Haiku");
    }

    #[tokio::test]
    async fn sqs_queues_preserve_pagination_and_normalize_labels() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/"))
            .and(header("x-amz-target", "AmazonSQS.ListQueues"))
            .and(body_json(json!({
                "QueueNamePrefix": "jobs-",
                "NextToken": "before",
                "MaxResults": 25
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "QueueUrls": ["https://sqs.us-east-1.amazonaws.com/123/jobs-main"],
                "NextToken": "after"
            })))
            .mount(&server)
            .await;
        let mut request = request("messaging.queues");
        request.arguments = json!({"queueNamePrefix": "jobs-"});
        request.cursor = Some("before".into());
        request.limit = Some(25);

        let page = resolve_resource(
            &reqwest::Client::new(),
            SQS_QUEUES_RESOLVER,
            &json!({
                "aws_access_key_id": "key",
                "aws_secret_access_key": "secret",
                "aws_region": "us-east-1",
                "endpoint": server.uri()
            }),
            &request,
        )
        .await
        .unwrap();

        assert_eq!(page.items[0].label, "jobs-main");
        assert_eq!(page.next_cursor.as_deref(), Some("after"));
    }

    #[tokio::test]
    async fn unknown_resolver_is_rejected_before_egress() {
        let error = resolve_resource(
            &reqwest::Client::new(),
            "custom.missing",
            &json!({}),
            &request("custom.resources"),
        )
        .await
        .unwrap_err();
        assert!(matches!(error, ConnectionsError::Validation(_)));
    }
}
