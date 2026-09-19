//! A completed external operation and its durable record may precede Cancel.
//! The guest must retain that record but stop continuation and clean up peers
//! before acknowledging. These tests use public compilation and real HTTP.
use super::*;

const RESULT: &str = "runtara:v2:[\"agent\",";
const BUDGET: &str = "runtara:v2:[\"agent-deadline\",";

#[tokio::test]
async fn checkpoint_pause_after_failed_attempt_retains_original_backoff_on_replay()
-> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut graph = agent_graph(2);
    graph["steps"]["fetch"]["timeout"] = 60_000.into();
    graph["steps"]["fetch"]["retryDelay"] = 3_000.into();
    let compiled = compile_graph(dir.path(), graph)?;
    let host = Arc::new(Host::new());
    host.clock_override.store(1_000, Ordering::SeqCst);
    *host.checkpoint_signal.lock().unwrap() = Some(RESULT.into());
    host.checkpoint_signal_remaining.store(1, Ordering::SeqCst);
    let mut server = Server::start(host.clone(), vec![Child::Retryable, Child::Success], 0).await?;
    let exit = invoke_with_outbound(&compiled, host.clone(), server.outbound()).await?;
    server.check().await?;
    assert!(matches!(exit, InvokeExit::Suspended(_)), "{exit:?}");
    assert!(host.acknowledged.swap(false, Ordering::SeqCst));
    let saved = host.checkpoints.lock().unwrap().clone();
    let retry = saved
        .iter()
        .find(|(key, _)| key.contains("::retry_sleep::"))
        .expect("Pause must persist backoff before acknowledging");
    assert_eq!(u64::from_le_bytes(retry.1.as_slice().try_into()?), 4_000);
    assert_eq!(server.children.load(Ordering::SeqCst), 1);

    *host.checkpoint_signal.lock().unwrap() = None;
    host.clock_override.store(2_000, Ordering::SeqCst);
    let exit = invoke_with_outbound(&compiled, host.clone(), server.outbound()).await?;
    server.check().await?;
    assert!(matches!(exit, InvokeExit::Suspended(_)), "{exit:?}");
    assert_eq!(
        server.children.load(Ordering::SeqCst),
        1,
        "early resume must not retry"
    );
    assert_eq!(
        *host.checkpoints.lock().unwrap(),
        saved,
        "early resume cannot reset the clock"
    );
    assert!(!host.acknowledged.load(Ordering::SeqCst));

    host.clock_override.store(4_000, Ordering::SeqCst);
    let exit = invoke_with_outbound(&compiled, host.clone(), server.outbound()).await?;
    server.check().await?;
    let InvokeExit::Completed(output) = exit else {
        anyhow::bail!("{exit:?}")
    };
    assert_eq!(
        serde_json::from_slice::<Value>(&output)?,
        json!({"ok":true})
    );
    assert_eq!(
        server.children.load(Ordering::SeqCst),
        2,
        "replay must skip the completed failed attempt and execute one retry"
    );
    assert!(!host.acknowledged.load(Ordering::SeqCst));
    Ok(())
}

#[tokio::test]
async fn checkpoint_completed_result_replays_after_pause_even_when_budget_has_elapsed()
-> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut graph = agent_graph(0);
    graph["steps"]["fetch"]["timeout"] = 60_000.into();
    let compiled = compile_graph(dir.path(), graph)?;
    let host = Arc::new(Host::new());
    host.clock_override.store(1_000, Ordering::SeqCst);
    *host.checkpoint_signal.lock().unwrap() = Some(RESULT.into());
    host.checkpoint_signal_remaining.store(1, Ordering::SeqCst);
    let mut server = Server::start(host.clone(), vec![Child::Success], 0).await?;
    let exit = invoke_with_outbound(&compiled, host.clone(), server.outbound()).await?;
    server.check().await?;
    assert!(matches!(exit, InvokeExit::Suspended(_)), "{exit:?}");
    assert!(host.acknowledged.swap(false, Ordering::SeqCst));
    let saved = host.checkpoints.lock().unwrap().clone();
    *host.checkpoint_signal.lock().unwrap() = None;
    host.clock_override.store(70_000, Ordering::SeqCst);
    let exit = invoke_with_outbound(&compiled, host.clone(), server.outbound()).await?;
    server.check().await?;
    let InvokeExit::Completed(output) = exit else {
        anyhow::bail!("{exit:?}")
    };
    assert_eq!(
        serde_json::from_slice::<Value>(&output)?,
        json!({"ok":true})
    );
    assert_eq!(server.children.load(Ordering::SeqCst), 1);
    assert_eq!(*host.checkpoints.lock().unwrap(), saved);
    assert!(!host.acknowledged.load(Ordering::SeqCst));
    Ok(())
}

fn cancel_on_checkpoint(host: &Host, prefix: &str) {
    *host.checkpoint_signal.lock().unwrap() = Some(prefix.into());
    host.checkpoint_cancel.store(true, Ordering::SeqCst);
    host.checkpoint_signal_remaining.store(1, Ordering::SeqCst);
}

fn assert_cancelled_at_checkpoint(exit: InvokeExit, host: &Host) {
    // Lifecycle Cancel is acknowledged by RuntimeHost; the guest exits with
    // Suspended, not a fabricated normal result or a local onError outcome.
    assert!(matches!(exit, InvokeExit::Suspended(_)), "{exit:?}");
    assert!(host.acknowledged.load(Ordering::SeqCst));
    assert!(
        !host.cancel.load(Ordering::SeqCst),
        "only checkpoint delivery is enabled"
    );
}

fn result_records(host: &Host) -> Vec<(String, Vec<u8>)> {
    host.checkpoints
        .lock()
        .unwrap()
        .iter()
        .filter(|(key, _)| key.starts_with(RESULT))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

#[tokio::test]
async fn checkpoint_cancel_after_failed_attempt_prevents_retry_and_terminal_failure()
-> anyhow::Result<()> {
    for (response, retryable) in [(Child::Permanent, false), (Child::Retryable, true)] {
        for retries in [1, 2] {
            let dir = tempfile::tempdir()?;
            let mut graph = agent_graph(retries);
            graph["steps"]["fetch"]["timeout"] = 60_000.into();
            graph["steps"]["fetch"]["retryDelay"] = 0.into();
            let compiled = compile_graph(dir.path(), graph)?;
            let host = Arc::new(Host::new());
            cancel_on_checkpoint(&host, RESULT);
            let mut server = Server::start(host.clone(), vec![response], 0).await?;
            let exit = invoke_with_outbound(&compiled, host.clone(), server.outbound()).await?;
            server.check().await?;
            assert_cancelled_at_checkpoint(exit, &host);
            assert_eq!(server.children.load(Ordering::SeqCst), 1);
            let saved = result_records(&host);
            assert_eq!(saved.len(), 1);
            assert!(saved[0].0.ends_with("::attempt::1"), "{}", saved[0].0);
            assert_eq!(saved[0].1[0], 1, "preserve the failed attempt envelope");
            assert_eq!(
                saved[0].1[1],
                u8::from(retryable),
                "preserve retryability for explicit replay"
            );
        }
    }
    Ok(())
}

#[tokio::test]
async fn checkpoint_cancel_retains_completed_agent_result_without_continuation()
-> anyhow::Result<()> {
    for retries in [0, 2] {
        let dir = tempfile::tempdir()?;
        let mut graph = agent_graph(retries);
        graph["steps"]["fetch"]["timeout"] = 60_000.into();
        for id in ["after", "recovery"] {
            graph["steps"][id] = graph["steps"]["fetch"].clone();
            graph["steps"][id]["id"] = id.into();
            graph["executionPlan"]
                .as_array_mut()
                .unwrap()
                .push(json!({"fromStep":id,"toStep":"finish"}));
        }
        graph["executionPlan"][0]["toStep"] = "after".into();
        graph["executionPlan"]
            .as_array_mut()
            .unwrap()
            .push(json!({"fromStep":"fetch","toStep":"recovery","label":"onError"}));
        let compiled = compile_graph(dir.path(), graph)?;
        let host = Arc::new(Host::new());
        cancel_on_checkpoint(&host, RESULT);
        let mut server = Server::start(host.clone(), vec![Child::Success], 0).await?;
        let exit = invoke_with_outbound(&compiled, host.clone(), server.outbound()).await?;
        server.check().await?;
        assert_cancelled_at_checkpoint(exit, &host);
        assert_eq!(
            server.children.load(Ordering::SeqCst),
            1,
            "Cancel must not dispatch normal continuation, retry or onError"
        );
        let saved = result_records(&host);
        assert_eq!(saved.len(), 1);
        assert!(
            !saved[0].1.is_empty(),
            "the completed result must survive Cancel"
        );
        let result: Value = serde_json::from_slice(&saved[0].1)?;
        assert_eq!(result["status_code"], 200, "{result}");
        assert_eq!(
            host.checkpoint_calls
                .lock()
                .unwrap()
                .iter()
                .filter(|(key, write)| *write && key.starts_with(RESULT))
                .count(),
            1
        );
    }
    Ok(())
}

#[tokio::test]
async fn checkpoint_cancel_at_budget_creation_prevents_agent_dispatch() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut graph = agent_graph(2);
    graph["steps"]["fetch"]["timeout"] = 60_000.into();
    let compiled = compile_graph(dir.path(), graph)?;
    let host = Arc::new(Host::new());
    cancel_on_checkpoint(&host, BUDGET);
    let mut server = Server::start(host.clone(), vec![], 0).await?;
    let exit = invoke_with_outbound(&compiled, host.clone(), server.outbound()).await?;
    server.check().await?;
    assert_cancelled_at_checkpoint(exit, &host);
    assert_eq!(server.children.load(Ordering::SeqCst), 0);
    assert!(result_records(&host).is_empty());
    assert_eq!(
        host.checkpoints
            .lock()
            .unwrap()
            .keys()
            .filter(|key| key.starts_with(BUDGET))
            .count(),
        1
    );
    Ok(())
}

#[tokio::test]
async fn checkpoint_cancel_after_parallel_result_closes_pending_peer_before_ack()
-> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut graph = branch_graph();
    graph["steps"]["a"]["inputMapping"]["url"]["value"] = "http://fixture.test/fast".into();
    graph["steps"]["b"]["inputMapping"]["url"]["value"] = "http://fixture.test/slow".into();
    for id in ["a", "b"] {
        graph["steps"][id]["timeout"] = 60_000.into();
    }
    let compiled = compile_graph(dir.path(), graph)?;
    for body in [false, true] {
        let host = Arc::new(Host::new());
        cancel_on_checkpoint(&host, RESULT);
        *host.cancel_cleanup.lock().unwrap() = Some(Arc::new(tokio::sync::Notify::new()));
        // The fast response is released only after the sibling request starts.
        // The runtime acknowledgement waits for the server to observe EOF;
        // dropping the Store after invoke cannot satisfy this ordering check.
        let mut server = live_peer_server(host.clone(), body, false).await?;
        let exit = invoke_with_outbound(&compiled, host.clone(), server.outbound()).await?;
        server.check().await?;
        assert_cancelled_at_checkpoint(exit, &host);
        assert_eq!(server.children.load(Ordering::SeqCst), 2);
        assert_eq!(server.closed.load(Ordering::SeqCst), 1);
        let saved = result_records(&host);
        assert_eq!(
            saved.len(),
            1,
            "pending peer must not acquire a completed result"
        );
        let result: Value = serde_json::from_slice(&saved[0].1)?;
        assert_eq!(result["status_code"], 200, "{result}");
    }
    Ok(())
}
