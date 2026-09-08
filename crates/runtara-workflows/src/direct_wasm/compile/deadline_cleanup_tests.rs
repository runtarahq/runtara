//! Real emitted timed workflow with deliberately noncooperative Agent fixtures.
use super::*;

fn agent(cpu_body: bool) -> anyhow::Result<Vec<u8>> {
    let core = if cpu_body {
        r#"(func (export "invoke") (param i32 i32 i32 i32) (result i32)
          (loop $spin (br $spin)) unreachable)"#
    } else {
        r#"(global $set (mut i32) (i32.const 0))
        (func (export "invoke") (param i32 i32 i32 i32) (result i32)
          (global.set $set (call $new))
          (call $join (i32.shr_u (call $sleep (i64.const 300000)) (i32.const 4)) (global.get $set))
          (i32.or (i32.shl (global.get $set) (i32.const 4)) (i32.const 2)))
        (func (export "callback") (param $event i32) (param i32 i32) (result i32)
          (if (i32.ne (local.get $event) (i32.const 6)) (then unreachable))
          (loop $cleanup (br $cleanup)) unreachable)"#
    };
    Ok(wat::parse_str(format!(
        r#"(component
      (import "runtara:host-io/timers@0.1.0" (instance $timers
        (export "sleep" (func async (param "ms" u64)))))
      (core func $sleep (canon lower (func $timers "sleep") async))
      (core func $new (canon waitable-set.new))
      (core func $join (canon waitable.join))
      (core module $code
        (import "h" "sleep" (func $sleep (param i64) (result i32)))
        (import "h" "new" (func $new (result i32)))
        (import "h" "join" (func $join (param i32 i32)))
        (memory (export "memory") 1)
        (func (export "realloc") (param i32 i32 i32 i32) (result i32) i32.const 4096)
        {core})
      (core instance $code (instantiate $code (with "h" (instance
        (export "sleep" (func $sleep)) (export "new" (func $new)) (export "join" (func $join))))))
      (type $error (record (field "code" string) (field "message" string)
        (field "category" string) (field "severity" string) (field "retryable" bool)
        (field "retry-after-ms" (option u64)) (field "attributes" (option string))))
      (func $invoke async (param "capability-id" string) (param "input" (list u8))
        (result (result (list u8) (error $error)))
        (canon lift (core func $code "invoke") (memory $code "memory")
          (realloc (func $code "realloc")) {}))
      (instance $agent (export "error-info" (type $error)) (export "invoke" (func $invoke)))
      (export "runtara:agent-http/capabilities@0.4.0" (instance $agent)))"#,
        if cpu_body {
            ""
        } else {
            "async (callback (func $code \"callback\"))"
        }
    ))?)
}

pub(super) fn fixture_components(dir: &Path, cpu_body: bool) -> anyhow::Result<PathBuf> {
    let fixtures = dir.join("components");
    fs::create_dir(&fixtures)?;
    // Stage independent fixture paths. Shared build outputs remain untouched.
    for entry in fs::read_dir(std::env::var("RUNTARA_AGENT_COMPONENTS_DIR")?)? {
        let entry = entry?;
        if entry.file_type()?.is_file() && entry.file_name() != "runtara_agent_http.wasm" {
            fs::hard_link(entry.path(), fixtures.join(entry.file_name()))?;
        }
    }
    fs::write(fixtures.join("runtara_agent_http.wasm"), agent(cpu_body)?)?;
    Ok(fixtures)
}

async fn aborts(cpu_body: bool) -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut compiled = compile(dir.path(), "http://unused.test", 100, false, 0, 0, true)?;
    let fixtures = fixture_components(dir.path(), cpu_body)?;
    compose_direct_workflow(&mut compiled, &fixtures)?;
    let executor = executor();
    let pre = executor.load_instance_pre(&compiled.wasm_path).await?;
    let host = Arc::new(Host::new());
    let started = Instant::now();
    let result = tokio::time::timeout(
        Duration::from_secs(12),
        executor.execute_invoke(
            &pre,
            WorkflowRunSpec {
                env: HashMap::new(),
                stderr: None,
                timeout: Duration::from_secs(10),
                cancel: None,
                limits: Default::default(),
                runtime: Some(host.clone()),
            },
            b"{}".to_vec(),
        ),
    )
    .await?;
    assert!(
        matches!(result.exit, InvokeExit::CleanupAborted),
        "{result:?}"
    );
    assert!(started.elapsed() >= Duration::from_millis(cooperative_wait::TIMEOUT_CLEANUP_GRACE_MS));
    assert!(
        started.elapsed() < Duration::from_secs(9),
        "ordinary run timeout cannot make this pass"
    );
    assert!(!host.acknowledged.load(Ordering::SeqCst));
    assert!(!host.recovery_observed.load(Ordering::SeqCst));
    assert!(
        !host
            .checkpoints
            .lock()
            .unwrap()
            .keys()
            .any(|key| key.ends_with("::result"))
    );
    Ok(())
}

#[tokio::test]
async fn emitted_deadline_prearms_abort_before_cpu_bound_agent_entry() -> anyhow::Result<()> {
    aborts(true).await
}

#[tokio::test]
async fn emitted_deadline_keeps_abort_armed_through_cpu_bound_cancellation() -> anyhow::Result<()> {
    aborts(false).await
}

#[tokio::test]
async fn emitted_deadline_success_disarms_alarm_before_long_continuation() -> anyhow::Result<()> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let dir = tempfile::tempdir()?;
    let compiled = compile_shaped(
        dir.path(),
        &format!("http://{}", listener.local_addr()?),
        500,
        false,
        0,
        0,
        true,
        Shape::LongContinuation,
    )?;
    let executor = executor();
    let pre = executor.load_instance_pre(&compiled.wasm_path).await?;
    let server = async move {
        for second in [false, true] {
            let (mut socket, _) = listener.accept().await?;
            let mut headers = Vec::new();
            while !headers.ends_with(b"\r\n\r\n") {
                headers.push(socket.read_u8().await?);
                anyhow::ensure!(headers.len() < 8192, "oversized fixture request");
            }
            if second {
                assert!(headers.starts_with(b"GET /after "));
                tokio::time::sleep(Duration::from_secs(6)).await;
            }
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n42")
                .await?;
        }
        Ok::<_, anyhow::Error>(())
    };
    let run = executor.execute_invoke(
        &pre,
        WorkflowRunSpec {
            env: HashMap::new(),
            stderr: None,
            timeout: Duration::from_secs(10),
            cancel: None,
            limits: Default::default(),
            runtime: Some(Arc::new(Host::new())),
        },
        b"{}".to_vec(),
    );
    let started = Instant::now();
    let (result, server) =
        tokio::time::timeout(Duration::from_secs(12), async { tokio::join!(run, server) }).await?;
    server?;
    assert!(
        matches!(result.exit, InvokeExit::Completed(_)),
        "{result:?}"
    );
    assert!(
        started.elapsed() >= Duration::from_secs(6),
        "must continue past the disposed alarm's deadline"
    );
    Ok(())
}
