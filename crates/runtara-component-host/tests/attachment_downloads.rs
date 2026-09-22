//! Real WASM agents against an injected outbound fixture, with no HTTP listener. Ingestion is covered by the
//! local-server E2E; this suite verifies component dispatch and download transport.
mod common;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use runtara_component_host::outbound_http::{
    Destination, OutboundContext, OutboundError, OutboundHttpHost, RequestOptions, Response,
};
use runtara_component_host::{
    ComponentDispatcherService, DispatcherEnv, ResolvedConnection, TestCapabilityRequest,
};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

type Requests = Arc<Mutex<Vec<RequestOptions>>>;
struct DownloadService(Requests);

#[async_trait::async_trait]
impl OutboundHttpHost for DownloadService {
    async fn request(
        &self,
        context: &OutboundContext,
        request: RequestOptions,
    ) -> Result<Response, OutboundError> {
        assert_eq!(context.tenant_id, "attachments-test");
        Ok(download_fixture(&self.0, request))
    }
}
const FILE_URL: &str = "https://files.slack.com/files-pri/T-F/invoice.bin";
const MESSAGE_URL: &str =
    "https://storage-us-west1.api.mailgun.net/v3/domains/inbound.example/messages/key";
const CONTENT: &[u8] = &[0, 255, 1, 128];

fn download_fixture(requests: &Requests, request: RequestOptions) -> Response {
    requests.lock().unwrap().push(request.clone());
    let url = match &request.destination {
        Destination::Connection(c) => &c.url,
        Destination::Public(url) => url,
    };
    let mut status = 200;
    let mut headers = json!({"content-type": "application/octet-stream"});
    let body = if url.ends_with("/files.info") {
        let body: Value = serde_json::from_slice(request.body.as_deref().unwrap()).unwrap();
        let id = body["file"].as_str().unwrap();
        serde_json::to_vec(&json!({"ok": true, "file": {
            "id": id, "name": "invoice.bin", "mimetype": "application/octet-stream",
            "size": if id == "F-large" { 100 } else { CONTENT.len() },
            "url_private_download": FILE_URL
        }}))
        .unwrap()
    } else if url == MESSAGE_URL {
        headers = json!({"content-type": "application/json"});
        serde_json::to_vec(
            &json!({"body-plain": "Email", "body-html": "<p>Email</p>", "attachments": [
                {"name": "invoice.bin", "url": format!("{MESSAGE_URL}/attachments/0")}
            ]}),
        )
        .unwrap()
    } else if url.ends_with("/rate-limited") {
        status = 429;
        headers = json!({"retry-after": "3"});
        vec![]
    } else if url.ends_with("/redirect") {
        status = 302;
        headers = json!({"location": "https://unapproved.example/file"});
        vec![]
    } else if url.ends_with("/oversize") {
        status = 413;
        vec![]
    } else {
        CONTENT.to_vec()
    };
    Response {
        status,
        headers: headers
            .as_object()
            .unwrap()
            .iter()
            .map(|(name, value)| (name.clone(), value.as_str().unwrap().into()))
            .collect(),
        body,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn workflow_agents_download_only_on_explicit_invocation() -> anyhow::Result<()> {
    let requests = Requests::default();
    let bundle = tempfile::tempdir()?;
    for agent in ["slack", "mailgun"] {
        for extension in ["wasm", "meta.json"] {
            let name = format!("runtara_agent_{agent}.{extension}");
            std::fs::copy(common::bundle_dir().join(&name), bundle.path().join(name))?;
        }
    }
    let dispatcher = ComponentDispatcherService::from_dir(
        bundle.path(),
        DispatcherEnv {
            core_http_url: "http://127.0.0.1:1".into(),
        },
    )
    .await?;
    dispatcher.set_outbound_http(Arc::new(DownloadService(requests.clone())))?;
    let call = |agent: &str, capability: &str, input| TestCapabilityRequest {
        tenant_id: "attachments-test".into(),
        agent_id: agent.into(),
        capability_id: capability.into(),
        input,
        connection: Some(ResolvedConnection {
            connection_id: format!("{agent}-connection"),
            integration_id: if agent == "slack" {
                "slack_bot"
            } else {
                "mailgun"
            }
            .into(),
            connection_subtype: None,
            parameters: json!({}),
            rate_limit_config: None,
        }),
    };

    let info = dispatcher
        .test_capability(call("slack", "get-file-info", json!({"file_id": "F-test"})))
        .await?;
    assert!(info.success, "{:?}", info.error);
    assert_eq!(info.output.unwrap()["file"]["name"], "invoice.bin");
    assert_eq!(
        requests.lock().unwrap().len(),
        1,
        "metadata must not fetch bytes"
    );

    let downloaded = dispatcher
        .test_capability(call("slack", "download-file", json!({"file_id": "F-test"})))
        .await?;
    assert!(downloaded.success, "{:?}", downloaded.error);
    let output = downloaded.output.unwrap();
    assert_eq!(
        STANDARD.decode(output["content"].as_str().unwrap())?,
        CONTENT
    );
    assert_eq!(output["size"], CONTENT.len());
    let count = requests.lock().unwrap().len();
    let large = dispatcher
        .test_capability(call(
            "slack",
            "download-file",
            json!({"file_id": "F-large", "max_bytes": 10}),
        ))
        .await?;
    assert_eq!(large.error.unwrap().code, "DOWNLOAD_TOO_LARGE");
    assert_eq!(
        requests.lock().unwrap().len(),
        count + 1,
        "reject large metadata before file GET"
    );

    let count = requests.lock().unwrap().len();
    let message = dispatcher
        .test_capability(call("mailgun", "get-message", json!({"url": MESSAGE_URL})))
        .await?;
    assert!(message.success, "{:?}", message.error);
    assert_eq!(requests.lock().unwrap().len(), count + 1);
    let message = message.output.unwrap();
    assert_eq!(message["message"]["body-html"], "<p>Email</p>");
    let downloaded = dispatcher
        .test_capability(call(
            "mailgun",
            "download-attachment",
            json!({"url": message["attachments"][0]["url"], "filename": "invoice.bin"}),
        ))
        .await?;
    assert!(downloaded.success, "{:?}", downloaded.error);
    assert_eq!(
        STANDARD.decode(downloaded.output.unwrap()["content"].as_str().unwrap())?,
        CONTENT
    );

    for (path, code) in [
        ("rate-limited", "DOWNLOAD_HTTP_ERROR"),
        ("redirect", "DOWNLOAD_HTTP_ERROR"),
        ("oversize", "DOWNLOAD_TOO_LARGE"),
    ] {
        let result = dispatcher
            .test_capability(call(
                "mailgun",
                "download-attachment",
                json!({"url": format!("{MESSAGE_URL}/{path}")}),
            ))
            .await?;
        let error = result.error.unwrap();
        assert_eq!(error.code, code);
        if path == "rate-limited" {
            assert_eq!(error.category, "transient");
        }
    }
    let count = requests.lock().unwrap().len();
    let invalid = dispatcher
        .test_capability(call(
            "mailgun",
            "download-attachment",
            json!({"url": "https://evil.example/file"}),
        ))
        .await?;
    assert_eq!(invalid.error.unwrap().code, "MAILGUN_INVALID_STORAGE_URL");
    assert_eq!(requests.lock().unwrap().len(), count);

    for request in requests.lock().unwrap().iter() {
        assert!(
            !request
                .headers
                .iter()
                .any(|(name, _)| name.eq_ignore_ascii_case("authorization")),
            "guest must not receive provider credentials"
        );
        let Destination::Connection(connection) = &request.destination else {
            panic!("download must name its connection")
        };
        assert!(connection.connection_id.ends_with("-connection"));
        if connection.url == FILE_URL {
            assert_eq!(connection.endpoint.as_deref(), Some("files"));
        }
        if connection.url.starts_with(MESSAGE_URL) {
            assert_eq!(connection.endpoint.as_deref(), Some("storage-us-west1"));
            assert_eq!(request.max_response_bytes, Some(5 * 1024 * 1024));
        }
    }
    Ok(())
}
