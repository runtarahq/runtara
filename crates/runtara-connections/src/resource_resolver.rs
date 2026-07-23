//! Read-only, connection-backed resource discovery.
//!
//! Resource declarations and provider behavior are owned by connection-specific
//! extractors. The facade dispatches only by the connection's authoritative
//! `integration_id`; it does not know provider names or resolver identifiers.

use std::collections::HashMap;

use async_trait::async_trait;
use chrono::{Duration, Utc};
use reqwest::header::{ACCEPT, AUTHORIZATION};
use runtara_agents::extractors::connection_types::{
    AwsCredentialsExtractor, AwsCredentialsParams, OpenAiApiKeyParams, OpenAiExtractor,
};
use serde_json::{Map, Value, json};

use crate::auth::aws_signing::sign_request_v4;
use crate::error::ConnectionsError;
use crate::resolution::{
    ConnectionResourceDefinition, ConnectionResourceItem, ConnectionResourcePage,
    ConnectionResourceRequest,
};

const MAX_RESOURCE_LIMIT: usize = 1_000;

#[derive(Debug, Clone, Copy)]
struct ResourceSpec {
    name: &'static str,
    description: &'static str,
}

const OPENAI_RESOURCES: &[ResourceSpec] = &[ResourceSpec {
    name: "models",
    description: "Available OpenAI models",
}];

const AWS_RESOURCES: &[ResourceSpec] = &[
    ResourceSpec {
        name: "bedrock.models",
        description: "Available Amazon Bedrock models",
    },
    ResourceSpec {
        name: "sqs.queues",
        description: "Available Amazon SQS queues",
    },
];

#[async_trait]
trait ConnectionResourceExtractor: Send + Sync {
    fn integration_id(&self) -> &'static str;
    fn resources(&self) -> &'static [ResourceSpec];

    async fn resolve(
        &self,
        client: &reqwest::Client,
        parameters: &Value,
        request: &ConnectionResourceRequest,
    ) -> Result<ConnectionResourcePage, ConnectionsError>;
}

static RESOURCE_EXTRACTORS: &[&dyn ConnectionResourceExtractor] =
    &[&OpenAiExtractor, &AwsCredentialsExtractor];

fn extractor_for_integration(
    integration_id: &str,
) -> Option<&'static dyn ConnectionResourceExtractor> {
    RESOURCE_EXTRACTORS
        .iter()
        .copied()
        .find(|extractor| extractor.integration_id() == integration_id)
}

/// Resources advertised by the connection-specific extractor.
pub fn resources_for_integration(integration_id: &str) -> Vec<ConnectionResourceDefinition> {
    extractor_for_integration(integration_id)
        .map(|extractor| {
            extractor
                .resources()
                .iter()
                .map(|resource| ConnectionResourceDefinition {
                    name: resource.name.to_string(),
                    description: resource.description.to_string(),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Resolve one resource through the owning connection-specific extractor.
pub async fn resolve_resource(
    client: &reqwest::Client,
    integration_id: &str,
    parameters: &Value,
    request: &ConnectionResourceRequest,
) -> Result<ConnectionResourcePage, ConnectionsError> {
    let extractor = extractor_for_integration(integration_id).ok_or_else(|| {
        ConnectionsError::Validation(format!(
            "Connection integration '{integration_id}' does not expose resources"
        ))
    })?;
    if !extractor
        .resources()
        .iter()
        .any(|resource| resource.name == request.resource_name)
    {
        return Err(ConnectionsError::Validation(format!(
            "Connection integration '{integration_id}' does not expose resource '{}'",
            request.resource_name
        )));
    }
    extractor.resolve(client, parameters, request).await
}

fn parse_parameters<T: serde::de::DeserializeOwned>(
    integration_id: &str,
    parameters: &Value,
) -> Result<T, ConnectionsError> {
    serde_json::from_value(parameters.clone()).map_err(|error| {
        ConnectionsError::AuthResolution(
            format!("Invalid {integration_id} connection parameters: {error}").into(),
        )
    })
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
        .clamp(1, MAX_RESOURCE_LIMIT)
}

fn contains_search(request: &ConnectionResourceRequest, values: &[&str]) -> bool {
    let Some(search) = request.search().map(str::to_ascii_lowercase) else {
        return true;
    };
    values
        .iter()
        .any(|value| value.to_ascii_lowercase().contains(&search))
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
        ConnectionsError::AuthResolution(
            format!("Failed to read {provider} response: {error}").into(),
        )
    })?;
    if !status.is_success() {
        let detail = String::from_utf8_lossy(&bytes);
        let detail: String = detail.chars().take(512).collect();
        return Err(ConnectionsError::AuthResolution(
            format!("{provider} resource discovery failed with HTTP {status}: {detail}").into(),
        ));
    }
    serde_json::from_slice(&bytes).map_err(|error| {
        ConnectionsError::AuthResolution(
            format!("{provider} resource discovery returned invalid JSON: {error}").into(),
        )
    })
}

#[async_trait]
impl ConnectionResourceExtractor for OpenAiExtractor {
    fn integration_id(&self) -> &'static str {
        "openai_api_key"
    }

    fn resources(&self) -> &'static [ResourceSpec] {
        OPENAI_RESOURCES
    }

    async fn resolve(
        &self,
        client: &reqwest::Client,
        parameters: &Value,
        request: &ConnectionResourceRequest,
    ) -> Result<ConnectionResourcePage, ConnectionsError> {
        let parameters: OpenAiApiKeyParams = parse_parameters(self.integration_id(), parameters)?;
        match request.resource_name.as_str() {
            "models" => resolve_openai_models(client, &parameters, request).await,
            _ => unreachable!("resource name was validated against OpenAI extractor metadata"),
        }
    }
}

async fn resolve_openai_models(
    client: &reqwest::Client,
    parameters: &OpenAiApiKeyParams,
    request: &ConnectionResourceRequest,
) -> Result<ConnectionResourcePage, ConnectionsError> {
    let url = append_path(&parameters.base_url, "models")?;
    let mut builder = client
        .get(url)
        .header(AUTHORIZATION, format!("Bearer {}", parameters.api_key))
        .header(ACCEPT, "application/json");
    if let Some(organization_id) = parameters
        .organization_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        builder = builder.header("OpenAI-Organization", organization_id);
    }
    let response = builder.send().await.map_err(|error| {
        ConnectionsError::AuthResolution(format!("OpenAI model discovery failed: {error}").into())
    })?;
    let body = response_json(response, "OpenAI").await?;
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
            let owner = model
                .get("owned_by")
                .and_then(Value::as_str)
                .unwrap_or_default();
            contains_search(request, &[id, owner])
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

#[async_trait]
impl ConnectionResourceExtractor for AwsCredentialsExtractor {
    fn integration_id(&self) -> &'static str {
        "aws_credentials"
    }

    fn resources(&self) -> &'static [ResourceSpec] {
        AWS_RESOURCES
    }

    async fn resolve(
        &self,
        client: &reqwest::Client,
        parameters: &Value,
        request: &ConnectionResourceRequest,
    ) -> Result<ConnectionResourcePage, ConnectionsError> {
        let parameters: AwsCredentialsParams = parse_parameters(self.integration_id(), parameters)?;
        match request.resource_name.as_str() {
            "bedrock.models" => resolve_bedrock_models(client, &parameters, request).await,
            "sqs.queues" => resolve_sqs_queues(client, &parameters, request).await,
            _ => unreachable!("resource name was validated against AWS extractor metadata"),
        }
    }
}

async fn signed_aws_json(
    client: &reqwest::Client,
    method: reqwest::Method,
    url: reqwest::Url,
    service: &str,
    credentials: &AwsCredentialsParams,
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
        &credentials.aws_access_key_id,
        &credentials.aws_secret_access_key,
        &credentials.aws_region,
        service,
        credentials.aws_session_token.as_deref(),
    );

    let mut builder = client.request(method, url).body(body.to_vec());
    for (name, value) in headers {
        builder = builder.header(name, value);
    }
    let response = builder.send().await.map_err(|error| {
        ConnectionsError::AuthResolution(format!("AWS resource discovery failed: {error}").into())
    })?;
    response_json(response, "AWS").await
}

async fn resolve_bedrock_models(
    client: &reqwest::Client,
    credentials: &AwsCredentialsParams,
    request: &ConnectionResourceRequest,
) -> Result<ConnectionResourcePage, ConnectionsError> {
    let base_url = credentials
        .endpoint
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| format!("https://bedrock.{}.amazonaws.com", credentials.aws_region));
    let url = append_path(&base_url, "foundation-models")?;
    let body = signed_aws_json(
        client,
        reqwest::Method::GET,
        url,
        "bedrock",
        credentials,
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
        .filter(|model| {
            let id = model
                .get("modelId")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let name = model
                .get("modelName")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let provider = model
                .get("providerName")
                .and_then(Value::as_str)
                .unwrap_or_default();
            contains_search(request, &[id, name, provider])
        })
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
    credentials: &AwsCredentialsParams,
    request: &ConnectionResourceRequest,
) -> Result<ConnectionResourcePage, ConnectionsError> {
    let endpoint = credentials
        .endpoint
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| format!("https://sqs.{}.amazonaws.com", credentials.aws_region));
    let url = reqwest::Url::parse(&endpoint).map_err(|error| {
        ConnectionsError::Validation(format!("Connection endpoint is invalid: {error}"))
    })?;
    let mut payload = Map::new();
    if let Some(prefix) = request.search() {
        payload.insert("QueueNamePrefix".into(), Value::String(prefix.to_string()));
    }
    if let Some(cursor) = request.cursor.as_deref() {
        payload.insert("NextToken".into(), Value::String(cursor.to_string()));
    }
    payload.insert("MaxResults".into(), json!(request_limit(request)));
    let payload = serde_json::to_vec(&Value::Object(payload)).map_err(|error| {
        ConnectionsError::Internal(format!("Failed to encode SQS request: {error}"))
    })?;
    let body = signed_aws_json(
        client,
        reqwest::Method::POST,
        url,
        "sqs",
        credentials,
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
    use wiremock::matchers::{body_json, header, header_exists, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn request(resource_name: &str) -> ConnectionResourceRequest {
        ConnectionResourceRequest {
            resource_name: resource_name.to_string(),
            search: None,
            cursor: None,
            limit: None,
        }
    }

    #[test]
    fn resources_are_owned_by_connection_extractors() {
        assert_eq!(
            resources_for_integration("openai_api_key"),
            vec![ConnectionResourceDefinition {
                name: "models".into(),
                description: "Available OpenAI models".into(),
            }]
        );
        assert_eq!(
            resources_for_integration("aws_credentials")
                .into_iter()
                .map(|resource| resource.name)
                .collect::<Vec<_>>(),
            vec!["bedrock.models", "sqs.queues"]
        );
        assert!(resources_for_integration("custom").is_empty());
    }

    #[tokio::test]
    async fn openai_models_are_normalized_and_searched() {
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
        let mut request = request("models");
        request.search = Some("gpt".into());

        let page = resolve_resource(
            &reqwest::Client::new(),
            "openai_api_key",
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
    async fn bedrock_searches_name_id_and_provider_without_public_filters() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/foundation-models"))
            .and(header_exists("authorization"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "modelSummaries": [
                    {
                        "modelId": "anthropic.claude-3-haiku",
                        "modelName": "Claude 3 Haiku",
                        "providerName": "Anthropic"
                    },
                    {
                        "modelId": "amazon.titan-text",
                        "modelName": "Titan Text",
                        "providerName": "Amazon"
                    }
                ]
            })))
            .mount(&server)
            .await;
        let mut request = request("bedrock.models");
        request.search = Some("anthropic".into());

        let page = resolve_resource(
            &reqwest::Client::new(),
            "aws_credentials",
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

        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].value, "anthropic.claude-3-haiku");
        assert_eq!(page.items[0].label, "Claude 3 Haiku");
    }

    #[tokio::test]
    async fn sqs_maps_generic_search_to_prefix_and_preserves_cursor() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/"))
            .and(header("x-amz-target", "AmazonSQS.ListQueues"))
            .and(header_exists("authorization"))
            .and(body_json(json!({
                "QueueNamePrefix": "orders",
                "NextToken": "cursor-1",
                "MaxResults": 20
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "QueueUrls": ["https://sqs.test/123/orders-created"],
                "NextToken": "cursor-2"
            })))
            .mount(&server)
            .await;
        let mut request = request("sqs.queues");
        request.search = Some("orders".into());
        request.cursor = Some("cursor-1".into());
        request.limit = Some(20);

        let page = resolve_resource(
            &reqwest::Client::new(),
            "aws_credentials",
            &json!({
                "access_key_id": "key",
                "secret_access_key": "secret",
                "region": "us-east-1",
                "endpoint": server.uri()
            }),
            &request,
        )
        .await
        .unwrap();

        assert_eq!(page.items[0].label, "orders-created");
        assert_eq!(page.next_cursor.as_deref(), Some("cursor-2"));
    }

    #[tokio::test]
    async fn rejects_resource_not_advertised_by_the_connection() {
        let error = resolve_resource(
            &reqwest::Client::new(),
            "openai_api_key",
            &json!({"api_key": "secret"}),
            &request("sqs.queues"),
        )
        .await
        .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("does not expose resource 'sqs.queues'")
        );
    }
}
