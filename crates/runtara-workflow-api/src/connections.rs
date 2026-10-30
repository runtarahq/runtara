//! Wire shapes of the connection family (`/v1/connections/*` and `/v1/http`),
//! shared by the host and the guest.

use serde::{Deserialize, Serialize};

/// The `Runtara-Outbound` metadata of `POST /v1/http`; the request body is the
/// raw HTTP body.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OutboundMeta {
    pub method: String,
    pub destination: OutboundDestination,
    #[serde(default)]
    pub headers: Vec<(String, String)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_response_bytes: Option<u64>,
    /// The request carries a body, possibly empty. HTTP alone cannot tell an
    /// empty body from none; providers can (a signed upload of zero bytes).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub has_body: bool,
}

/// Where an outbound call goes: through a connection's credentials, or to a
/// public URL through the outbound guard.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub enum OutboundDestination {
    Connection(ConnectionTarget),
    Public(String),
}

/// A connection-backed destination. The connection is an opaque id: the host
/// resolves its credentials by (id, tenant), so nothing secret crosses.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConnectionTarget {
    pub connection_id: String,
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ai_provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aws_service: Option<String>,
}

/// The `Runtara-Error` of a failed outbound call (`502`), whose body is
/// whatever upstream body was received.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutboundFailure {
    pub code: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u64>,
}

/// A SQL call of `POST /v1/connections/{id}/sql/{operation}`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SqlOperation {
    Query,
    Execute,
    Batch,
}

impl SqlOperation {
    pub fn parse(segment: &str) -> Option<Self> {
        match segment {
            "query" => Some(Self::Query),
            "execute" => Some(Self::Execute),
            "batch" => Some(Self::Batch),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn outbound_metadata_wire_form() {
        let meta = OutboundMeta {
            method: "POST".into(),
            destination: OutboundDestination::Connection(ConnectionTarget {
                connection_id: "conn-1".into(),
                url: "https://api.example.com/v1/items".into(),
                endpoint: None,
                endpoint_ref: None,
                ai_provider: None,
                aws_service: None,
            }),
            headers: vec![("content-type".into(), "application/json".into())],
            timeout_ms: Some(1000),
            max_response_bytes: None,
            has_body: false,
        };
        assert_eq!(
            serde_json::to_value(&meta).unwrap(),
            json!({
                "method": "POST",
                "destination": {"connection": {
                    "connectionId": "conn-1",
                    "url": "https://api.example.com/v1/items"
                }},
                "headers": [["content-type", "application/json"]],
                "timeoutMs": 1000
            })
        );
        let public: OutboundMeta = serde_json::from_value(
            json!({"method": "GET", "destination": {"public": "https://example.com"}}),
        )
        .unwrap();
        assert_eq!(
            public.destination,
            OutboundDestination::Public("https://example.com".into())
        );
    }
}
