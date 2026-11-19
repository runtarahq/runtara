//! How a connection type's webhooks are signed, declared as data.
//!
//! A platform that calls a trigger's webhook proves the call is its own with
//! a secret the connection holds: a token it sends as it is, or a signature
//! over the request. Each such connection type declares here which of its
//! parameters holds the secret and where in a request to find what to check.
//! The server checks every webhook from the declaration alone, asking seal,
//! and holds no code of any platform's (`docs/native-chat-phase-5.md`).
//!
//! The secret's seal policy only verifies (`sealed::policy`): it is never
//! sent, and nothing reads it. seal answers yes or no.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// How one connection type's webhooks are checked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebhookCheck {
    /// The connection's parameter that holds the secret.
    pub secret_field: String,
    #[serde(flatten)]
    pub kind: CheckKind,
}

/// What a webhook carries to prove it is the platform's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum CheckKind {
    /// The secret itself, in a header.
    #[serde(rename_all = "camelCase")]
    Token {
        /// The header it comes in.
        header: String,
    },
    /// An HMAC-SHA256 under the secret of a message made from the request.
    #[serde(rename_all = "camelCase")]
    HmacSha256 {
        /// What is signed: a [template](Template).
        message: String,
        /// Where the signature is: a template.
        signature: String,
        /// Text before the signature, removed before it is decoded (`v0=`).
        #[serde(default, skip_serializing_if = "String::is_empty")]
        signature_prefix: String,
        /// How the signature is written.
        encoding: Encoding,
        /// Where the time it was signed is, in seconds since the epoch: a
        /// template. With `max_age_secs`, a webhook signed longer ago than
        /// that, or that far ahead, is refused.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timestamp: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_age_secs: Option<u64>,
    },
}

/// How a signature is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Encoding {
    Hex,
    Base64,
}

/// What a webhook presents to be checked, read from it by its declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Presented {
    /// What seal is to MAC. Empty for a token, which covers no message.
    pub message: Vec<u8>,
    /// The signature, decoded, or the token as it was sent.
    pub mac: Vec<u8>,
}

/// The checks of the connection types whose webhooks are signed.
pub fn declared(integration_id: &str) -> Option<WebhookCheck> {
    let hmac = |secret_field: &str, message: &str, signature: &str| WebhookCheck {
        secret_field: secret_field.to_owned(),
        kind: CheckKind::HmacSha256 {
            message: message.to_owned(),
            signature: signature.to_owned(),
            signature_prefix: String::new(),
            encoding: Encoding::Hex,
            timestamp: None,
            max_age_secs: None,
        },
    };
    match integration_id {
        // HMAC-SHA256(key, timestamp + token), all three in the body: under
        // `signature`, or at the top level as older webhooks have them.
        "mailgun" => Some(hmac(
            "webhook_signing_key",
            "{field:/signature/timestamp|/timestamp}{field:/signature/token|/token}",
            "{field:/signature/signature|/signature}",
        )),
        // `v0=` + hex HMAC-SHA256(secret, "v0:" + timestamp + ":" + body),
        // refused after five minutes as Slack asks.
        "slack_bot" => {
            let mut check = hmac(
                "signing_secret",
                "v0:{header:x-slack-request-timestamp}:{body}",
                "{header:x-slack-signature}",
            );
            if let CheckKind::HmacSha256 {
                signature_prefix,
                timestamp,
                max_age_secs,
                ..
            } = &mut check.kind
            {
                *signature_prefix = "v0=".to_owned();
                *timestamp = Some("{header:x-slack-request-timestamp}".to_owned());
                *max_age_secs = Some(300);
            }
            Some(check)
        }
        // The secret token given to `setWebhook`, sent back with each call.
        "telegram_bot" => Some(WebhookCheck {
            secret_field: crate::sealed::policy::WEBHOOK_SECRET_FIELD.to_owned(),
            kind: CheckKind::Token {
                header: "x-telegram-bot-api-secret-token".to_owned(),
            },
        }),
        _ => None,
    }
}

/// The connection types with a declared check, for the tests that hold the
/// declarations to their seal policies.
pub const DECLARING_TYPES: &[&str] = &["mailgun", "slack_bot", "telegram_bot"];

impl WebhookCheck {
    /// Headers a check found a secret in: not handed to the run.
    pub fn withheld_headers(&self) -> Vec<String> {
        match &self.kind {
            CheckKind::Token { header } => vec![header.to_ascii_lowercase()],
            CheckKind::HmacSha256 { .. } => Vec::new(),
        }
    }

    /// What `request` presents to be checked, at `now` (seconds since the
    /// epoch). `Err` refuses the webhook without asking seal: it lacks what
    /// the declaration names, its signature is not written as declared, or
    /// it was signed too long ago.
    pub fn presented(&self, request: &Request<'_>, now: i64) -> Result<Presented, String> {
        match &self.kind {
            CheckKind::Token { header } => {
                let token = request
                    .header(header)
                    .filter(|token| !token.is_empty())
                    .ok_or("the webhook carries no secret token")?;
                Ok(Presented {
                    message: Vec::new(),
                    mac: token.as_bytes().to_vec(),
                })
            }
            CheckKind::HmacSha256 {
                message,
                signature,
                signature_prefix,
                encoding,
                timestamp,
                max_age_secs,
            } => {
                if let (Some(timestamp), Some(max_age)) = (timestamp, max_age_secs) {
                    let at = Template(timestamp).render(request)?;
                    let at: i64 = std::str::from_utf8(&at)
                        .ok()
                        .and_then(|at| at.trim().parse().ok())
                        .ok_or("the webhook's timestamp is not a time")?;
                    if now.abs_diff(at) > *max_age {
                        return Err("the webhook was signed too long ago".to_owned());
                    }
                }
                let message = Template(message).render(request)?;
                let written = Template(signature).render(request)?;
                let written = std::str::from_utf8(&written)
                    .map_err(|_| "the webhook's signature is not text")?
                    .trim();
                let written = written
                    .strip_prefix(signature_prefix.as_str())
                    .ok_or("the webhook's signature is not written as its platform writes it")?;
                let mac = match encoding {
                    Encoding::Hex => hex::decode(written).ok(),
                    Encoding::Base64 => {
                        use base64::Engine as _;
                        base64::engine::general_purpose::STANDARD
                            .decode(written)
                            .ok()
                    }
                }
                .filter(|mac| !mac.is_empty())
                .ok_or("the webhook's signature is not written as its platform writes it")?;
                Ok(Presented { message, mac })
            }
        }
    }
}

/// A webhook's request, as a check reads it.
pub struct Request<'a> {
    headers: Vec<(String, &'a str)>,
    body: &'a [u8],
    json: std::cell::OnceCell<Option<Value>>,
}

impl<'a> Request<'a> {
    /// `headers` by name, as received.
    pub fn new(headers: impl IntoIterator<Item = (&'a str, &'a str)>, body: &'a [u8]) -> Self {
        Self {
            headers: headers
                .into_iter()
                .map(|(name, value)| (name.to_ascii_lowercase(), value))
                .collect(),
            body,
            json: std::cell::OnceCell::new(),
        }
    }

    /// A request whose fields were read from a form (`multipart/form-data`)
    /// rather than a JSON body: `{field:…}` reads `fields`.
    pub fn with_fields(mut self, fields: Value) -> Self {
        self.json = std::cell::OnceCell::from(Some(fields));
        self
    }

    fn header(&self, name: &str) -> Option<&'a str> {
        let name = name.to_ascii_lowercase();
        self.headers
            .iter()
            .find(|(candidate, _)| *candidate == name)
            .map(|(_, value)| *value)
    }

    fn field(&self, pointer: &str) -> Option<String> {
        let json = self
            .json
            .get_or_init(|| serde_json::from_slice(self.body).ok());
        match json.as_ref()?.pointer(pointer)? {
            Value::String(text) => Some(text.clone()),
            Value::Number(number) => Some(number.to_string()),
            _ => None,
        }
    }
}

/// Text with parts of the request in it: `{body}`, the body as it was sent;
/// `{header:NAME}`; `{field:/json/pointer}`, a string or number of a JSON
/// body, with alternatives tried in order (`{field:/a|/b}`). A part the
/// request does not have refuses the webhook.
struct Template<'t>(&'t str);

impl Template<'_> {
    fn render(&self, request: &Request<'_>) -> Result<Vec<u8>, String> {
        let mut out = Vec::new();
        let mut rest = self.0;
        while let Some(open) = rest.find('{') {
            out.extend_from_slice(&rest.as_bytes()[..open]);
            let close = rest[open..]
                .find('}')
                .map(|close| open + close)
                .ok_or("a webhook check's template is not closed")?;
            let part = &rest[open + 1..close];
            if part == "body" {
                out.extend_from_slice(request.body);
            } else if let Some(name) = part.strip_prefix("header:") {
                let value = request
                    .header(name)
                    .ok_or_else(|| format!("the webhook carries no {name} header"))?;
                out.extend_from_slice(value.as_bytes());
            } else if let Some(pointers) = part.strip_prefix("field:") {
                let value = pointers
                    .split('|')
                    .find_map(|pointer| request.field(pointer))
                    .ok_or_else(|| format!("the webhook's body has no {pointers}"))?;
                out.extend_from_slice(value.as_bytes());
            } else {
                return Err(format!("a webhook check's template names {part}"));
            }
            rest = &rest[close + 1..];
        }
        out.extend_from_slice(rest.as_bytes());
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hmac::{Hmac, Mac};

    fn mac(secret: &str, message: &[u8]) -> Vec<u8> {
        let mut mac = Hmac::<sha2::Sha256>::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(message);
        mac.finalize().into_bytes().to_vec()
    }

    const NOW: i64 = 1_700_000_000;

    #[test]
    fn slack_signs_its_timestamp_and_body() {
        let check = declared("slack_bot").unwrap();
        let body = br#"{"type":"event_callback","event":{"text":"hi"}}"#;
        let signed = mac(
            "s3cret",
            format!("v0:{NOW}:{}", std::str::from_utf8(body).unwrap()).as_bytes(),
        );
        let signature = format!("v0={}", hex::encode(&signed));
        let timestamp = NOW.to_string();
        let request = Request::new(
            [
                ("X-Slack-Request-Timestamp", timestamp.as_str()),
                ("X-Slack-Signature", signature.as_str()),
            ],
            body,
        );
        let presented = check.presented(&request, NOW + 10).unwrap();
        assert_eq!(presented.mac, signed);
        assert_eq!(presented.message, mac_message(NOW, body));
        assert!(check.withheld_headers().is_empty());

        // Signed six minutes ago, or six minutes ahead.
        assert!(check.presented(&request, NOW + 360).is_err());
        assert!(check.presented(&request, NOW - 360).is_err());
        // No signature, or one without its prefix, or not hex.
        let unsigned = Request::new([("x-slack-request-timestamp", timestamp.as_str())], body);
        assert!(check.presented(&unsigned, NOW).is_err());
        let bare = hex::encode(&signed);
        let unprefixed = Request::new(
            [
                ("x-slack-request-timestamp", timestamp.as_str()),
                ("x-slack-signature", bare.as_str()),
            ],
            body,
        );
        assert!(check.presented(&unprefixed, NOW).is_err());
        let garbled = Request::new(
            [
                ("x-slack-request-timestamp", timestamp.as_str()),
                ("x-slack-signature", "v0=zz"),
            ],
            body,
        );
        assert!(check.presented(&garbled, NOW).is_err());
    }

    fn mac_message(at: i64, body: &[u8]) -> Vec<u8> {
        let mut message = format!("v0:{at}:").into_bytes();
        message.extend_from_slice(body);
        message
    }

    #[test]
    fn mailgun_signs_what_its_body_carries_in_either_place() {
        let check = declared("mailgun").unwrap();
        let nested = br#"{"signature":{"timestamp":"1700000000","token":"tok","signature":"abcd"},"event-data":{}}"#;
        let presented = check.presented(&Request::new([], nested), NOW).unwrap();
        assert_eq!(presented.message, b"1700000000tok");
        assert_eq!(presented.mac, [0xab, 0xcd]);

        let top = br#"{"timestamp":1700000000,"token":"tok","signature":"abcd"}"#;
        let presented = check.presented(&Request::new([], top), NOW).unwrap();
        assert_eq!(presented.message, b"1700000000tok");

        assert!(check.presented(&Request::new([], b"{}"), NOW).is_err());
        assert!(
            check
                .presented(&Request::new([], b"not json"), NOW)
                .is_err()
        );

        // A form's fields, as a route forwards an inbound message.
        let form = Request::new([], b"").with_fields(
            serde_json::json!({"timestamp": "1700000000", "token": "tok", "signature": "abcd"}),
        );
        assert_eq!(
            check.presented(&form, NOW).unwrap().message,
            b"1700000000tok"
        );
    }

    #[test]
    fn telegram_sends_its_token_in_a_header_the_run_is_not_handed() {
        let check = declared("telegram_bot").unwrap();
        let request = Request::new([("X-Telegram-Bot-Api-Secret-Token", "tok-1")], b"{}");
        let presented = check.presented(&request, NOW).unwrap();
        assert_eq!(presented.message, b"");
        assert_eq!(presented.mac, b"tok-1");
        assert_eq!(
            check.withheld_headers(),
            ["x-telegram-bot-api-secret-token"]
        );
        assert!(check.presented(&Request::new([], b"{}"), NOW).is_err());
        let empty = Request::new([("x-telegram-bot-api-secret-token", "")], b"{}");
        assert!(check.presented(&empty, NOW).is_err());
    }

    #[test]
    fn a_declaration_is_read_back_as_it_was_written() {
        for integration in DECLARING_TYPES {
            let check = declared(integration).unwrap();
            let written = serde_json::to_value(&check).unwrap();
            assert_eq!(
                serde_json::from_value::<WebhookCheck>(written).unwrap(),
                check
            );
        }
        assert_eq!(
            serde_json::to_value(declared("telegram_bot").unwrap()).unwrap(),
            serde_json::json!({
                "secretField": "webhook_secret",
                "kind": "token",
                "header": "x-telegram-bot-api-secret-token"
            })
        );
        assert!(declared("http_bearer").is_none());
    }

    /// A declared secret is one seal only verifies with, the way its check
    /// asks: a token compared, a signature an HMAC.
    #[test]
    fn each_declared_secret_is_sealed_only_to_be_checked() {
        let allowances = crate::sealed::policy::Allowances::default();
        for integration in DECLARING_TYPES {
            let check = declared(integration).unwrap();
            let policies =
                crate::sealed::policy::policies(integration, &serde_json::json!({}), &allowances)
                    .unwrap_or_else(|| panic!("{integration} is sealed"));
            let policy = serde_json::to_value(&policies[&check.secret_field]).unwrap();
            assert!(
                policy
                    .get("rules")
                    .is_none_or(|rules| rules == &serde_json::json!([])),
                "{integration} sends its secret"
            );
            let expected = match check.kind {
                CheckKind::Token { .. } => serde_json::json!("equals"),
                CheckKind::HmacSha256 { .. } => serde_json::json!({"hmac": {"hash": "sha256"}}),
            };
            assert_eq!(policy["verify"], expected, "{integration}");
            assert!(
                !crate::sealed::policy::request_fields(integration, &serde_json::json!({}))
                    .contains(&check.secret_field),
                "{integration} presents its webhook secret with requests"
            );
        }
    }

    #[test]
    fn a_template_names_only_what_it_knows() {
        let request = Request::new([("a", "1")], b"B");
        assert_eq!(
            Template("x{header:A}y{body}").render(&request).unwrap(),
            b"x1yB"
        );
        assert!(Template("{query:a}").render(&request).is_err());
        assert!(Template("{body").render(&request).is_err());
    }
}
