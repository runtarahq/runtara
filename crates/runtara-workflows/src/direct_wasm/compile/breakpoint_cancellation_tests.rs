//! Lifecycle receipts returned by breakpoint persistence must be handled before
//! the debug pause or resumed step runs. Use public compilation and real HTTP.
use super::*;

const BREAKPOINT: &str = "runtara:v2:[\"breakpoint\",";

fn graph(durable: bool) -> Value {
    let mut graph = agent_graph(0);
    graph["durable"] = durable.into();
    graph["steps"]["fetch"]["breakpoint"] = true.into();
    graph["steps"]["fetch"]["timeout"] = 60_000.into();
    graph
}

fn debug_host() -> Arc<Host> {
    let host = Arc::new(Host::new());
    host.debug_enabled.store(true, Ordering::SeqCst);
    *host.breakpoint_pause_error.lock().unwrap() = None;
    host
}

#[tokio::test]
async fn breakpoint_cancel_in_embed_parent_or_child_prevents_child_http() -> anyhow::Result<()> {
    for at_child in [false, true] {
        let dir = tempfile::tempdir()?;
        let root = json!({"durable":true,"entryPoint":"call","steps":{
            "call":{"id":"call","stepType":"EmbedWorkflow","childWorkflowId":"child",
                "childVersion":"latest","timeout":60_000,"breakpoint":!at_child},
            "finish":{"id":"finish","stepType":"Finish"},
            "recovery":{"id":"recovery","stepType":"Finish","inputMapping":{
                "unexpected":{"valueType":"immediate","value":true}}}},
            "executionPlan":[{"fromStep":"call","toStep":"finish"},
                {"fromStep":"call","toStep":"recovery","label":"onError"}]});
        let mut child = graph(true);
        child["steps"]["fetch"]["breakpoint"] = at_child.into();
        let compiled = crate::direct_wasm::compile::agent_deadline_tests::embed::compile_composed(
            dir.path(),
            serde_json::from_value(root)?,
            vec![crate::ChildWorkflowInput {
                step_id: "call".into(),
                workflow_id: "child".into(),
                version_requested: "latest".into(),
                version_resolved: 1,
                execution_graph: serde_json::from_value(child)?,
            }],
            false,
            "breakpoint-cancellation",
        )?;
        let host = debug_host();
        deliver_at_breakpoint(&host, true);
        let mut server = Server::start(host.clone(), vec![], 0).await?;
        let exit = invoke_with_env(&compiled, host.clone(), server.env()).await?;
        server.check().await?;
        assert_acknowledged(exit, &host);
        assert_eq!(server.children.load(Ordering::SeqCst), 0);
        assert_eq!(host.breakpoint_pause_calls.load(Ordering::SeqCst), 0);
        assert_eq!(host.breakpoint_hits.load(Ordering::SeqCst), 0);
        assert_eq!(
            host.checkpoints
                .lock()
                .unwrap()
                .keys()
                .filter(|key| key.starts_with(BREAKPOINT))
                .count(),
            1
        );
    }
    Ok(())
}

fn deliver_at_breakpoint(host: &Host, cancel: bool) {
    *host.checkpoint_signal.lock().unwrap() = Some(BREAKPOINT.into());
    host.checkpoint_cancel.store(cancel, Ordering::SeqCst);
    host.checkpoint_signal_remaining.store(1, Ordering::SeqCst);
}

fn assert_acknowledged(exit: InvokeExit, host: &Host) {
    assert!(matches!(exit, InvokeExit::Suspended(_)), "{exit:?}");
    assert!(host.acknowledged.load(Ordering::SeqCst));
    assert_eq!(host.checkpoint_signal_remaining.load(Ordering::SeqCst), 0);
    assert!(
        !host.cancel.load(Ordering::SeqCst),
        "poll_signal never delivers this command"
    );
}

#[tokio::test]
async fn breakpoint_rejected_pause_does_not_erase_the_existing_marker() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let compiled = compile_graph(dir.path(), graph(true))?;
    let host = debug_host();
    let mut server = Server::start(host.clone(), vec![Child::Success], 0).await?;
    let exit = invoke_with_env(&compiled, host.clone(), server.env()).await?;
    assert!(matches!(exit, InvokeExit::Suspended(_)), "{exit:?}");
    assert_eq!(host.breakpoint_pause_calls.load(Ordering::SeqCst), 1);
    deliver_at_breakpoint(&host, false);
    host.reject_checkpoint_signal.store(true, Ordering::SeqCst);
    let exit = invoke_with_env(&compiled, host.clone(), server.env()).await?;
    server.check().await?;
    let InvokeExit::Completed(output) = exit else {
        anyhow::bail!("{exit:?}")
    };
    assert_eq!(
        serde_json::from_slice::<Value>(&output)?,
        json!({"ok":true})
    );
    assert!(!host.reject_checkpoint_signal.load(Ordering::SeqCst));
    assert!(!host.acknowledged.load(Ordering::SeqCst));
    assert_eq!(server.children.load(Ordering::SeqCst), 1);
    assert_eq!(host.breakpoint_pause_calls.load(Ordering::SeqCst), 1);
    assert_eq!(host.breakpoint_hits.load(Ordering::SeqCst), 1);
    Ok(())
}

#[tokio::test]
async fn breakpoint_pause_receipt_is_acknowledged_once_and_resumes_without_a_second_pause()
-> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let compiled = compile_graph(dir.path(), graph(true))?;
    let host = debug_host();
    deliver_at_breakpoint(&host, false);
    let mut server = Server::start(host.clone(), vec![Child::Success], 0).await?;
    let exit = invoke_with_env(&compiled, host.clone(), server.env()).await?;
    server.check().await?;
    assert_acknowledged(exit, &host);
    assert_eq!(server.children.load(Ordering::SeqCst), 0);
    assert_eq!(host.breakpoint_pause_calls.load(Ordering::SeqCst), 0);
    assert_eq!(host.breakpoint_hits.load(Ordering::SeqCst), 0);
    *host.checkpoint_signal.lock().unwrap() = None;
    host.acknowledged.store(false, Ordering::SeqCst);
    let exit = invoke_with_env(&compiled, host.clone(), server.env()).await?;
    server.check().await?;
    assert!(matches!(exit, InvokeExit::Completed(_)), "{exit:?}");
    assert_eq!(server.children.load(Ordering::SeqCst), 1);
    assert_eq!(host.breakpoint_pause_calls.load(Ordering::SeqCst), 0);
    assert!(!host.acknowledged.load(Ordering::SeqCst));
    Ok(())
}

#[tokio::test]
async fn breakpoint_cancel_prevents_terminal_completion_and_error_publication() -> anyhow::Result<()>
{
    for step in [
        json!({"id":"terminal","stepType":"Finish","breakpoint":true}),
        json!({"id":"terminal","stepType":"Error","breakpoint":true,
            "code":"UNEXPECTED","message":"must not execute"}),
    ] {
        let dir = tempfile::tempdir()?;
        let compiled = compile_graph(
            dir.path(),
            json!({"durable":true,"entryPoint":"terminal",
            "steps":{"terminal":step},"executionPlan":[]}),
        )?;
        let host = debug_host();
        deliver_at_breakpoint(&host, true);
        let exit = invoke(&compiled, host.clone()).await?;
        assert_acknowledged(exit, &host);
        assert_eq!(host.breakpoint_pause_calls.load(Ordering::SeqCst), 0);
        assert_eq!(host.breakpoint_hits.load(Ordering::SeqCst), 0);
    }
    Ok(())
}

#[tokio::test]
async fn breakpoint_cancel_on_first_checkpoint_precedes_debug_pause_and_dispatch()
-> anyhow::Result<()> {
    for timed in [false, true] {
        let dir = tempfile::tempdir()?;
        let mut source = graph(true);
        if !timed {
            source["steps"]["fetch"]
                .as_object_mut()
                .unwrap()
                .remove("timeout");
        }
        let compiled = compile_graph(dir.path(), source)?;
        let host = debug_host();
        deliver_at_breakpoint(&host, true);
        let mut server = Server::start(host.clone(), vec![], 0).await?;
        let exit = invoke_with_env(&compiled, host.clone(), server.env()).await?;
        server.check().await?;
        assert_acknowledged(exit, &host);
        assert_eq!(server.children.load(Ordering::SeqCst), 0);
        assert_eq!(host.breakpoint_pause_calls.load(Ordering::SeqCst), 0);
        assert_eq!(host.breakpoint_hits.load(Ordering::SeqCst), 0);
        let saved = host.checkpoints.lock().unwrap();
        assert_eq!(saved.len(), 1);
        assert!(saved.keys().next().unwrap().starts_with(BREAKPOINT));
    }
    Ok(())
}

#[tokio::test]
async fn breakpoint_cancel_on_checkpoint_hit_prevents_resumed_agent_dispatch() -> anyhow::Result<()>
{
    let dir = tempfile::tempdir()?;
    let compiled = compile_graph(dir.path(), graph(true))?;
    let host = debug_host();
    let mut server = Server::start(host.clone(), vec![Child::Success], 0).await?;
    let exit = invoke_with_env(&compiled, host.clone(), server.env()).await?;
    server.check().await?;
    assert!(matches!(exit, InvokeExit::Suspended(_)), "{exit:?}");
    assert_eq!(server.children.load(Ordering::SeqCst), 0);
    assert_eq!(host.breakpoint_pause_calls.load(Ordering::SeqCst), 1);
    assert_eq!(host.breakpoint_hits.load(Ordering::SeqCst), 1);
    let saved = host.checkpoints.lock().unwrap().clone();

    deliver_at_breakpoint(&host, true);
    let exit = invoke_with_env(&compiled, host.clone(), server.env()).await?;
    server.check().await?;
    assert_acknowledged(exit, &host);
    assert_eq!(server.children.load(Ordering::SeqCst), 0);
    assert_eq!(host.breakpoint_pause_calls.load(Ordering::SeqCst), 1);
    assert_eq!(host.breakpoint_hits.load(Ordering::SeqCst), 1);
    assert_eq!(*host.checkpoints.lock().unwrap(), saved);
    Ok(())
}

#[tokio::test]
async fn breakpoint_without_cancel_still_pauses_once_then_resumes() -> anyhow::Result<()> {
    for durable in [false, true] {
        let dir = tempfile::tempdir()?;
        let compiled = compile_graph(dir.path(), graph(durable))?;
        let host = debug_host();
        let mut server = Server::start(host.clone(), vec![Child::Success], 0).await?;
        let exit = invoke_with_env(&compiled, host.clone(), server.env()).await?;
        server.check().await?;
        if !durable {
            assert!(matches!(exit, InvokeExit::Completed(_)), "{exit:?}");
            assert_eq!(server.children.load(Ordering::SeqCst), 1);
            assert_eq!(host.breakpoint_pause_calls.load(Ordering::SeqCst), 0);
            assert_eq!(host.breakpoint_hits.load(Ordering::SeqCst), 0);
            assert!(host.checkpoints.lock().unwrap().is_empty());
            continue;
        }
        assert!(matches!(exit, InvokeExit::Suspended(_)), "{exit:?}");
        assert_eq!(server.children.load(Ordering::SeqCst), 0);
        let exit = invoke_with_env(&compiled, host.clone(), server.env()).await?;
        server.check().await?;
        let InvokeExit::Completed(output) = exit else {
            anyhow::bail!("{exit:?}")
        };
        assert_eq!(
            serde_json::from_slice::<Value>(&output)?,
            json!({"ok":true})
        );
        assert_eq!(server.children.load(Ordering::SeqCst), 1);
        assert_eq!(host.breakpoint_pause_calls.load(Ordering::SeqCst), 1);
        assert_eq!(host.breakpoint_hits.load(Ordering::SeqCst), 1);
        assert!(!host.acknowledged.load(Ordering::SeqCst));
    }
    Ok(())
}
