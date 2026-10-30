use serde::{Deserialize, Serialize};

/// Why a workflow API call failed. Generic codes; endpoint families add their
/// own domain codes (see [`ErrorCode::Domain`]) with the HTTP status of the
/// closest generic code.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ErrorCode {
    Invalid,
    Denied,
    NotFound,
    ReplayConflict,
    Fenced,
    Finished,
    TooLarge,
    Capacity,
    Unavailable,
    Timeout,
    Unsupported,
    /// A family-specific code such as `not-child` or `requires-operation`,
    /// sent with the HTTP status of `like`.
    Domain {
        code: String,
        like: Box<ErrorCode>,
    },
}

impl ErrorCode {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Invalid => "invalid",
            Self::Denied => "denied",
            Self::NotFound => "not-found",
            Self::ReplayConflict => "replay-conflict",
            Self::Fenced => "fenced",
            Self::Finished => "finished",
            Self::TooLarge => "too-large",
            Self::Capacity => "capacity",
            Self::Unavailable => "unavailable",
            Self::Timeout => "timeout",
            Self::Unsupported => "unsupported",
            Self::Domain { code, .. } => code,
        }
    }

    /// Parse a wire code. Unknown codes are kept as domain codes that map to
    /// `invalid`'s status, so a newer host never breaks an older reader.
    pub fn parse(code: &str) -> Self {
        match code {
            "invalid" => Self::Invalid,
            "denied" => Self::Denied,
            "not-found" => Self::NotFound,
            "replay-conflict" => Self::ReplayConflict,
            "fenced" => Self::Fenced,
            "finished" => Self::Finished,
            "too-large" => Self::TooLarge,
            "capacity" => Self::Capacity,
            "unavailable" => Self::Unavailable,
            "timeout" => Self::Timeout,
            "unsupported" => Self::Unsupported,
            other => Self::Domain {
                code: other.to_string(),
                like: Box::new(Self::Invalid),
            },
        }
    }

    pub fn http_status(&self) -> u16 {
        match self {
            Self::Invalid => 400,
            Self::Denied => 403,
            Self::NotFound => 404,
            Self::ReplayConflict | Self::Fenced | Self::Finished => 409,
            Self::TooLarge => 413,
            Self::Capacity => 429,
            Self::Unsupported => 501,
            Self::Unavailable => 503,
            Self::Timeout => 504,
            Self::Domain { like, .. } => like.http_status(),
        }
    }

    /// Whether a caller may retry the same request later.
    pub fn retryable(&self) -> bool {
        match self {
            Self::Capacity | Self::Unavailable | Self::Timeout => true,
            Self::Domain { like, .. } => like.retryable(),
            _ => false,
        }
    }
}

impl Serialize for ErrorCode {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ErrorCode {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let code = String::deserialize(deserializer)?;
        Ok(Self::parse(&code))
    }
}

/// The body of every non-2xx response.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiError {
    pub code: ErrorCode,
    /// Human-readable; never carries payloads.
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}

impl ApiError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            retry_after_ms: None,
            details: None,
        }
    }

    pub fn domain(code: &str, like: ErrorCode, message: impl Into<String>) -> Self {
        Self::new(
            ErrorCode::Domain {
                code: code.to_string(),
                like: Box::new(like),
            },
            message,
        )
    }

    pub fn with_retry_after(mut self, ms: u64) -> Self {
        self.retry_after_ms = Some(ms);
        self
    }

    pub fn with_details(mut self, details: serde_json::Value) -> Self {
        self.details = Some(details);
        self
    }

    pub fn http_status(&self) -> u16 {
        self.code.http_status()
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code.as_str(), self.message)
    }
}

impl std::error::Error for ApiError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statuses_match_the_design_table() {
        let cases = [
            (ErrorCode::Invalid, 400),
            (ErrorCode::Denied, 403),
            (ErrorCode::NotFound, 404),
            (ErrorCode::ReplayConflict, 409),
            (ErrorCode::Fenced, 409),
            (ErrorCode::Finished, 409),
            (ErrorCode::TooLarge, 413),
            (ErrorCode::Capacity, 429),
            (ErrorCode::Unavailable, 503),
            (ErrorCode::Timeout, 504),
            (ErrorCode::Unsupported, 501),
        ];
        for (code, status) in cases {
            assert_eq!(code.http_status(), status, "{}", code.as_str());
            assert_eq!(ErrorCode::parse(code.as_str()), code);
        }
    }

    #[test]
    fn domain_codes_round_trip_with_their_status() {
        let error = ApiError::domain("not-child", ErrorCode::Denied, "not a direct child");
        assert_eq!(error.http_status(), 403);
        let json = serde_json::to_value(&error).unwrap();
        assert_eq!(json["code"], "not-child");
        assert!(json.get("retryAfterMs").is_none());
        let back: ApiError = serde_json::from_value(json).unwrap();
        assert_eq!(back.code.as_str(), "not-child");
    }

    #[test]
    fn envelope_shape() {
        let error = ApiError::new(ErrorCode::Capacity, "busy")
            .with_retry_after(3000)
            .with_details(serde_json::json!({"limit": 4}));
        assert_eq!(
            serde_json::to_value(&error).unwrap(),
            serde_json::json!({
                "code": "capacity",
                "message": "busy",
                "retryAfterMs": 3000,
                "details": {"limit": 4}
            })
        );
        assert!(error.code.retryable());
        assert!(!ErrorCode::Fenced.retryable());
    }
}
