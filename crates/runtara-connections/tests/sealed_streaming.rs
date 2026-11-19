// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! A model provider's answer, streamed through seal: what reaches this process
//! arrives a piece at a time, as the provider sends it, and not when it is
//! done (docs/ai-agent-streaming.md).
//!
//! The provider is a stand-in on this machine that writes server-sent events a
//! while apart. A sealed API key is attached by seal, which this process never
//! reads.

use std::sync::Arc;
use std::time::{Duration, Instant};

use http_body_util::BodyExt;
use runtara_connections::sealed::{Allowances, MemoryKeySource, SealedCredentials};
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const TENANT: &str = "acme";
const CONNECTION: &str = "model";
const KEY: &str = "sk-stand-in-CANARY";
/// How far apart the stand-in sends its pieces.
const APART: Duration = Duration::from_millis(400);
const PIECES: [&str; 4] = [
    "data: {\"delta\":\"Once\"}\n\n",
    "data: {\"delta\":\" upon\"}\n\n",
    "data: {\"delta\":\" a time\"}\n\n",
    "data: [DONE]\n\n",
];

/// A provider that answers one request with [`PIECES`], [`APART`] apart, as
/// HTTP/1.1 chunks, and hands back the request it was sent.
async fn stand_in() -> (String, u16, tokio::task::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let served = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut asked = Vec::new();
        let mut buffer = [0u8; 4096];
        // The head and a small body: read until the body is there.
        loop {
            let read = socket.read(&mut buffer).await.unwrap();
            asked.extend_from_slice(&buffer[..read]);
            let text = String::from_utf8_lossy(&asked);
            if let Some(end) = text.find("\r\n\r\n") {
                let length = text[..end]
                    .lines()
                    .find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .and_then(|value| value.trim().parse::<usize>().ok())
                    })
                    .unwrap_or(0);
                if asked.len() >= end + 4 + length {
                    break;
                }
            }
            if read == 0 {
                break;
            }
        }
        socket
            .write_all(
                b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\
                  transfer-encoding: chunked\r\nconnection: close\r\n\r\n",
            )
            .await
            .unwrap();
        for piece in PIECES {
            let chunk = format!("{:x}\r\n{piece}\r\n", piece.len());
            socket.write_all(chunk.as_bytes()).await.unwrap();
            socket.flush().await.unwrap();
            tokio::time::sleep(APART).await;
        }
        socket.write_all(b"0\r\n\r\n").await.unwrap();
        String::from_utf8_lossy(&asked).into_owned()
    });
    ("127.0.0.1".to_owned(), port, served)
}

/// What this process is allowed to reach: the stand-in, on this machine.
fn allowances(host: &str, port: u16) -> Allowances {
    Allowances {
        private_hosts: vec![format!("{host}:{port}")],
        plain_http_hosts: vec![host.to_owned()],
        ..Default::default()
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_streamed_answer_reaches_this_process_a_piece_at_a_time_through_seal() {
    let (host, port, served) = stand_in().await;
    let sealed = SealedCredentials::in_process(
        Arc::new(MemoryKeySource::default()),
        &[7_u8; 32],
        allowances(&host, port),
    )
    .unwrap();
    measure(&sealed, &host, port, served).await;
}

/// The same, through a seal that is a process of its own (`SEAL_BIN`), over
/// its socket. Passed over where no seal binary is given.
#[tokio::test(flavor = "multi_thread")]
async fn a_streamed_answer_reaches_this_process_a_piece_at_a_time_through_a_seal_of_its_own() {
    let Ok(seal_bin) = std::env::var("SEAL_BIN") else {
        eprintln!("SEAL_BIN is not set: a seal of its own is not tried");
        return;
    };
    let (host, port, served) = stand_in().await;
    // A socket's path is short: a system's temporary directory can be too long.
    let dir = tempfile::Builder::new()
        .prefix("rseal")
        .tempdir_in("/tmp")
        .unwrap();
    let root = dir.path();
    std::fs::create_dir(root.join("keys")).unwrap();
    std::fs::write(root.join("kek"), [9u8; 32]).unwrap();
    // seal reads no key that others may.
    {
        use std::os::unix::fs::PermissionsExt;
        for path in [root.join("kek"), root.join("keys"), root.to_path_buf()] {
            let mode = if path.is_dir() { 0o700 } else { 0o600 };
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
        }
    }
    let socket = root.join("seal.sock");
    let uid = String::from_utf8(
        std::process::Command::new("id")
            .arg("-u")
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    let config = format!(
        "[listen]\nunix = \"{socket}\"\n\n[[callers]]\nname = \"runtara-server\"\nunix_uid = {uid}\n\
         keysets = [\"*\"]\nadmin = true\ndestroy_keysets = true\n\n[keys]\n\
         source = {{ dir = \"{keys}\" }}\nwrap = {{ file = \"{kek}\" }}\nlock_memory = \"off\"\n\n\
         [network]\nallow_private_cidrs = [\"127.0.0.0/8\"]\nprivate_rule_cidrs = [\"127.0.0.0/8\"]\n",
        socket = socket.display(),
        uid = uid.trim(),
        keys = root.join("keys").display(),
        kek = root.join("kek").display(),
    );
    std::fs::write(root.join("seal.toml"), config).unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            root.join("seal.toml"),
            std::fs::Permissions::from_mode(0o600),
        )
        .unwrap();
    }
    let mut seal = std::process::Command::new(seal_bin)
        .args(["serve", "--config"])
        .arg(root.join("seal.toml"))
        .stdout(std::process::Stdio::null())
        .stderr(std::fs::File::create(root.join("seal.log")).unwrap())
        .spawn()
        .expect("seal starts");
    for _ in 0..100 {
        if socket.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let mode = runtara_connections::sealed::mode::SealMode::from_lookup(|name| match name {
        "RUNTARA_SEAL_MODE" => Some("unix".to_owned()),
        "RUNTARA_SEAL_SOCKET" => Some(socket.display().to_string()),
        _ => None,
    })
    .unwrap();
    assert!(
        socket.exists(),
        "seal did not listen: {}",
        std::fs::read_to_string(root.join("seal.log")).unwrap_or_default()
    );
    let client = mode.client().unwrap().expect("a seal of its own");
    let sealed = SealedCredentials::remote(client, allowances(&host, port));
    measure(&sealed, &host, port, served).await;
    let _ = seal.kill();
    let _ = seal.wait();
}

/// Sends a request through `sealed` to the stand-in, and holds the answer to
/// arriving a piece at a time.
async fn measure(
    sealed: &SealedCredentials,
    host: &str,
    port: u16,
    served: tokio::task::JoinHandle<String>,
) {
    let base = format!("http://{host}:{port}");
    let stored = sealed
        .seal_parameters(
            TENANT,
            CONNECTION,
            "openai_api_key",
            &json!({"api_key": KEY, "base_url": base}),
            None,
        )
        .await
        .unwrap()
        .expect("an OpenAI key is sealed");
    assert!(
        !stored.to_string().contains(KEY),
        "the key is not kept readable"
    );

    let request = http::Request::builder()
        .method("POST")
        .uri(format!("{base}/v1/chat/completions"))
        .header("content-type", "application/json")
        .body(Some(br#"{"stream":true}"#.to_vec()))
        .unwrap();
    let asked = Instant::now();
    let response = sealed
        .send(TENANT, CONNECTION, "openai_api_key", &stored, request)
        .await
        .expect("seal sends it");
    assert_eq!(response.status(), 200);

    // Each piece of the body, with when it arrived.
    let mut body = response.into_body();
    let mut arrived: Vec<(Duration, String)> = Vec::new();
    while let Some(frame) = body.frame().await {
        if let Ok(data) = frame.expect("a body frame").into_data() {
            arrived.push((asked.elapsed(), String::from_utf8_lossy(&data).into_owned()));
        }
    }
    let whole: String = arrived.iter().map(|(_, text)| text.as_str()).collect();
    assert_eq!(
        whole,
        PIECES.concat(),
        "the answer arrives whole and in order"
    );
    let times: Vec<u128> = arrived.iter().map(|(at, _)| at.as_millis()).collect();
    eprintln!("pieces arrived at {times:?} ms");

    // The first piece is here long before the provider has finished, and the
    // pieces come as they are sent, not together at the end.
    let last_sent = APART * (PIECES.len() as u32 - 1);
    assert!(arrived.len() >= 3, "{} frames: {times:?}", arrived.len());
    assert!(
        arrived[0].0 < last_sent / 2,
        "the first piece arrived at {:?}, the provider finished at {last_sent:?}",
        arrived[0].0
    );
    assert!(
        arrived.last().unwrap().0 - arrived[0].0 >= APART * 2,
        "the pieces arrived together: {times:?}"
    );

    // seal attached the key; this process never held it.
    let asked = served.await.unwrap();
    assert!(
        asked.to_ascii_lowercase().contains(&format!(
            "authorization: bearer {}",
            KEY.to_ascii_lowercase()
        )),
        "{asked}"
    );
}
