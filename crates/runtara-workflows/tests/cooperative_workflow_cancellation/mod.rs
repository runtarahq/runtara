//! Normal composed DSL execution: lifecycle notification selects cancellation
//! in emitted WASM. There is no isolation catalog, child Store or task factory.
use super::*;
use runtara_component_host::runtime_host::{
    RuntimeCheckpointResult, RuntimeHost, RuntimeSignalInfo,
};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::Notify;

struct Host {
    inner: PersistingRuntimeHost,
    requested: AtomicBool,
    requests: AtomicUsize,
    closed: Notify,
    closed_count: AtomicUsize,
    acknowledged: AtomicBool,
    fail_signal_read: bool,
    scenario: Scenario,
    observed: AtomicUsize,
    events: Mutex<Vec<(String, Vec<u8>)>>,
}

impl Host {
    async fn wait_closed(&self) {
        loop {
            let notified = self.closed.notified();
            if self.closed_count.load(Ordering::SeqCst) == self.requests.load(Ordering::SeqCst) {
                return;
            }
            notified.await;
        }
    }
}

#[async_trait::async_trait]
impl RuntimeHost for Host {
    async fn load_input(&self) -> Result<Option<Vec<u8>>, String> {
        self.inner.load_input().await
    }
    fn instance_id(&self) -> Result<String, String> {
        self.inner.instance_id()
    }
    async fn complete(&self, output: Vec<u8>) -> Result<(), String> {
        self.inner.complete(output).await
    }
    async fn fail(&self, error: Vec<u8>) -> Result<(), String> {
        self.inner.fail(error).await
    }
    async fn custom_event(&self, kind: String, payload: Vec<u8>) -> Result<(), String> {
        self.events
            .lock()
            .unwrap()
            .push((kind.clone(), payload.clone()));
        self.inner.custom_event(kind, payload).await
    }
    fn debug_mode_enabled(&self) -> Result<bool, String> {
        Ok(false)
    }
    async fn breakpoint_pause(&self) -> Result<(), String> {
        self.inner.breakpoint_pause().await
    }
    async fn heartbeat(&self) -> Result<(), String> {
        Ok(())
    }
    async fn poll_signal(&self) -> Result<Option<RuntimeSignalInfo>, String> {
        if self.fail_signal_read && self.requested.load(Ordering::SeqCst) {
            return Err("signal delivery failed".into());
        }

        if !self.requested.load(Ordering::SeqCst) || self.acknowledged.load(Ordering::SeqCst) {
            return Ok(None);
        }
        let observation = self.observed.fetch_add(1, Ordering::SeqCst);
        let kind = if self.scenario == Scenario::CheckpointCancelBranches {
            return Ok(None);
        } else if self.scenario == Scenario::PauseThenCancelBranches && observation == 0 {
            "pause"
        } else if self.scenario.drains_normally() {
            if observation > 0 {
                return Ok(None);
            } // rate-limited observation must remain visible
            self.scenario.signal_kind()
        } else {
            "cancel"
        };
        Ok(Some(signal(kind)))
    }
    async fn is_cancelled(&self) -> Result<bool, String> {
        assert!(
            !(self.scenario == Scenario::WhileParallel && self.requested.load(Ordering::SeqCst)),
            "While consumed cancellation before shared sibling cleanup"
        );
        if self.scenario == Scenario::WhileLegacyCancelError {
            return Err("legacy is-cancelled failed".into());
        }
        Ok(false)
    }
    async fn check_signals(&self) -> Result<bool, String> {
        assert!(
            !(self.scenario == Scenario::WhileParallel && self.requested.load(Ordering::SeqCst)),
            "While consumed cancellation before shared sibling cleanup"
        );
        if self.scenario == Scenario::WhileLegacyCheckError {
            return Err("legacy check-signals failed".into());
        }
        Ok(false)
    }
    async fn poll_custom_signal(&self, key: String) -> Result<Option<Vec<u8>>, String> {
        self.inner.poll_custom_signal(key).await
    }
    async fn get_checkpoint(&self, key: String) -> Result<Option<Vec<u8>>, String> {
        self.inner.get_checkpoint(key).await
    }
    async fn checkpoint(
        &self,
        key: String,
        state: Vec<u8>,
    ) -> Result<RuntimeCheckpointResult, String> {
        let mut result = self.inner.checkpoint(key.clone(), state).await?;
        if self.scenario == Scenario::CheckpointCancelBranches
            && key != "start"
            && self.requests.load(Ordering::SeqCst) == 2
        {
            result.pending_signal = Some(signal("cancel"));
        }
        Ok(result)
    }
    async fn handle_checkpoint_signal(&self, kind: String, id: String) -> Result<bool, String> {
        assert_eq!(
            (kind.as_str(), id.as_str()),
            (
                self.scenario.signal_kind(),
                format!("{}-current-run", self.scenario.signal_kind()).as_str()
            )
        );
        if self.requests.load(Ordering::SeqCst) != 0 {
            // The endpoint never supplies a complete response. EOF therefore
            // demonstrates local cancellation, not a response releasing the wait.
            tokio::time::timeout(Duration::from_secs(2), self.wait_closed())
                .await
                .map_err(|_| "acknowledgement preceded HTTP cleanup")?;
        }
        if self.scenario.drains_normally() {
            let checkpoints = self.inner.checkpoints.lock().unwrap();
            assert!(
                ["b", "c"].iter().all(|step| checkpoints
                    .keys()
                    .any(|key| key.ends_with(&format!("\"{step}\"]]")))),
                "pause/shutdown preceded sibling checkpoints: {:?}",
                checkpoints.keys()
            );
        }
        assert!(!self.acknowledged.swap(true, Ordering::SeqCst));
        Ok(true)
    }
    async fn record_retry_attempt(
        &self,
        _key: String,
        _attempt: u32,
        _error: Option<String>,
    ) -> Result<(), String> {
        panic!("root cancellation must not start a retry")
    }
    async fn durable_sleep_checkpoint(
        &self,
        key: String,
        state: Vec<u8>,
        ms: u64,
    ) -> Result<(), String> {
        self.inner.durable_sleep_checkpoint(key, state, ms).await
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Scenario {
    BeforeLaunch,
    WhileBody,
    WhileParallel,
    EmbedBody,
    EmbedPartialBody,
    EmbedWhileBody,
    EmbedParallel,
    WhileLegacyCancelError,
    WhileLegacyCheckError,
    HostlessLoop,
    Headers,
    SlackHeaders,
    Mailgun,
    TeamsChunks,
    McpInitialize,
    McpTool,
    AiSingle,
    AiTurn,
    AiSummary,
    AiMemoryLoad,
    AiMemorySave,
    ObjectQuery,
    ObjectExecute,
    StorageDownload(&'static str, bool),
    StoragePresign(&'static str),
    Sftp,
    SharepointDownload,
    SharepointDownloadBody,
    SharepointContentAfterMetadata,
    ShopifyImages,
    ShopifyImagesBody,
    ShopifyDeleteAfterRead,
    HubspotRead,
    HubspotReadBody,
    HubspotUpdateAfterRead,
    QuickbooksRead,
    QuickbooksReadBody,
    QuickbooksUpdateAfterRead,
    StripeCreate,
    StripeCreateBody,
    StripeFinalizeAfterCreate,
    SqsReceive,
    SqsReceiveBody,
    SqsDeleteAfterReceive,
    PartialBody,
    NestedAgent,
    NestedAgentBody,
    DeepNestedAgent,
    NestedParallelBranches,
    NestedParallelSplit,
    SignalReadFailure,
    ParallelSplit,
    ParallelBranches,
    WavefrontBranches,
    ParallelSignalReadFailure,
    PauseBranches,
    ShutdownBranches,
    PauseThenCancelBranches,
    CheckpointCancelBranches,
}

impl Scenario {
    fn nested_depth(self) -> usize {
        match self {
            Self::NestedAgent
            | Self::NestedAgentBody
            | Self::NestedParallelBranches
            | Self::NestedParallelSplit => 1,
            Self::DeepNestedAgent => 2,
            _ => 0,
        }
    }
    fn is_sharepoint(self) -> bool {
        matches!(
            self,
            Self::SharepointDownload
                | Self::SharepointDownloadBody
                | Self::SharepointContentAfterMetadata
        )
    }
    fn is_shopify(self) -> bool {
        matches!(
            self,
            Self::ShopifyImages | Self::ShopifyImagesBody | Self::ShopifyDeleteAfterRead
        )
    }
    fn is_hubspot(self) -> bool {
        matches!(
            self,
            Self::HubspotRead | Self::HubspotReadBody | Self::HubspotUpdateAfterRead
        )
    }

    fn is_quickbooks(self) -> bool {
        matches!(
            self,
            Self::QuickbooksRead | Self::QuickbooksReadBody | Self::QuickbooksUpdateAfterRead
        )
    }
    fn is_stripe(self) -> bool {
        matches!(
            self,
            Self::StripeCreate | Self::StripeCreateBody | Self::StripeFinalizeAfterCreate
        )
    }
    fn is_sqs(self) -> bool {
        matches!(
            self,
            Self::SqsReceive | Self::SqsReceiveBody | Self::SqsDeleteAfterReceive
        )
    }
    fn is_storage(self) -> bool {
        matches!(
            self,
            Self::StorageDownload(..) | Self::StoragePresign(_) | Self::Sftp
        )
    }
    fn is_object(self) -> bool {
        matches!(self, Self::ObjectQuery | Self::ObjectExecute)
    }
    fn is_ai(self) -> bool {
        matches!(
            self,
            Self::AiSingle
                | Self::AiTurn
                | Self::AiSummary
                | Self::AiMemoryLoad
                | Self::AiMemorySave
        )
    }
    fn is_mcp(self) -> bool {
        matches!(self, Self::McpInitialize | Self::McpTool)
    }
    fn uses_proxy(self) -> bool {
        matches!(self, Self::SlackHeaders | Self::Mailgun | Self::TeamsChunks)
            || self.is_ai()
            || self.is_mcp()
            || matches!(self, Self::StorageDownload(..))
            || self.is_sqs()
            || self.is_stripe()
            || self.is_quickbooks()
            || self.is_hubspot()
            || self.is_sharepoint()
            || self.is_shopify()
    }
    fn drains_normally(self) -> bool {
        matches!(self, Self::PauseBranches | Self::ShutdownBranches)
    }
    fn signal_kind(self) -> &'static str {
        match self {
            Self::PauseBranches => "pause",
            Self::ShutdownBranches => "shutdown",
            _ => "cancel",
        }
    }
}

fn signal(kind: &str) -> RuntimeSignalInfo {
    RuntimeSignalInfo {
        signal_type: kind.into(),
        command_id: format!("{kind}-current-run"),
        payload: vec![],
        checkpoint_id: None,
    }
}

fn compile_nested_agents(
    graph: ExecutionGraph,
    depth: usize,
    dir: &std::path::Path,
) -> anyhow::Result<runtara_workflows::direct_wasm::DirectCompilationResult> {
    compile_nested_agents_with_children(graph, vec![], depth, dir)
}

fn compile_nested_agents_with_children(
    mut graph: ExecutionGraph,
    mut children: Vec<runtara_workflows::ChildWorkflowInput>,
    depth: usize,
    dir: &std::path::Path,
) -> anyhow::Result<runtara_workflows::direct_wasm::DirectCompilationResult> {
    use runtara_workflows::direct_wasm::{
        compile_direct_workflow_with_abi, compose_direct_workflow_with_extra_dirs,
    };
    let components = direct_e2e_components_dir();
    let staging = dir.join("published");
    fs::create_dir(&staging)?;
    let mut agents = Vec::new();
    for level in 0..depth {
        let safety = runtara_workflows::direct_wasm::support::analyze_workflow_agent_safety(
            &graph, &children,
        );
        anyhow::ensure!(
            !safety.may_suspend_or_sleep,
            "fixture violates publish contract: {safety:?}"
        );
        let slug = format!("nested-http-{}", char::from(b'a' + u8::try_from(level)?));
        let mut child = compile_direct_workflow_with_abi(
            DirectCompilationInput {
                workflow_id: slug.clone(),
                version: 1,
                source_checksum: None,
                execution_graph: graph.clone(),
                child_workflows: std::mem::take(&mut children),
                output_dir: dir.join(&slug),
                track_events: false,
                agent_catalog: (!agents.is_empty()).then(|| {
                    Arc::new(runtara_dsl::agent_meta::AgentCatalog::from_agents(
                        agents.clone(),
                    ))
                }),
                agent_slug: Some(slug.clone()),
            },
            WorkflowAbi::AgentCapabilities,
            false,
        )?;
        anyhow::ensure!(
            child.omit_runtime,
            "published child must not observe or acknowledge the root signal"
        );
        if graph.steps.values().any(|step| {
            matches!(step,
            runtara_dsl::Step::Split(split) if split.config.as_ref()
                .and_then(|config| config.max_retries).unwrap_or(0) > 0)
        }) {
            anyhow::ensure!(
                child.parallel_pools.is_empty(),
                "Split-level retries must retain their existing sequential fallback"
            );
        }
        compose_direct_workflow_with_extra_dirs(
            &mut child,
            &components,
            std::slice::from_ref(&staging),
        )?;
        anyhow::ensure!(child.scoped_agents.is_empty() && child.invocation_manifest.is_none());
        let info = certified_workflow_agent_info(
            &slug,
            &slug,
            "",
            &graph.input_schema,
            &graph.output_schema,
        );
        fs::copy(
            child.wasm_path,
            staging.join(format!("runtara_agent_{}.wasm", slug.replace('-', "_"))),
        )?;
        fs::write(
            staging.join(format!(
                "runtara_agent_{}.meta.json",
                slug.replace('-', "_")
            )),
            serde_json::to_vec(&info)?,
        )?;
        agents.push(info);
        graph = serde_json::from_value(serde_json::json!({
            "durable":false, "entryPoint":"call", "steps":{
                "call":{"id":"call","stepType":"Agent","agentId":slug,"capabilityId":"run", "maxRetries":3,"retryDelay":0,
                    "inputMapping":{"items":{"valueType":"reference","value":"data.items"}}},
                "finish":{"id":"finish","stepType":"Finish","inputMapping":{"result":{"valueType":"reference","value":"steps.call.outputs"}}},
                "handled":{"id":"handled","stepType":"Finish","inputMapping":{
                    "unexpected_parent_error":{"valueType":"reference","value":"steps.__error"}}}
            }, "executionPlan":[{"fromStep":"call","toStep":"finish"},{"fromStep":"call","toStep":"handled","label":"onError"}]
        }))?;
    }
    let mut parent = compile_direct_workflow_with_abi(
        DirectCompilationInput {
            workflow_id: "parent-of-nested-http".into(),
            version: 1,
            source_checksum: None,
            execution_graph: graph,
            child_workflows: vec![],
            output_dir: dir.join("parent"),
            track_events: false,
            agent_catalog: Some(Arc::new(
                runtara_dsl::agent_meta::AgentCatalog::from_agents(agents),
            )),
            agent_slug: None,
        },
        WorkflowAbi::InvokeHostImports,
        false,
    )?;
    compose_direct_workflow_with_extra_dirs(&mut parent, &components, &[staging])?;
    Ok(parent)
}

async fn run(scenario: Scenario) -> anyhow::Result<()> {
    let pre_cancel = scenario == Scenario::BeforeLaunch;
    let partial_body = matches!(
        scenario,
        Scenario::PartialBody
            | Scenario::EmbedPartialBody
            | Scenario::NestedAgentBody
            | Scenario::SqsReceiveBody
            | Scenario::StripeCreateBody
            | Scenario::QuickbooksReadBody
            | Scenario::HubspotReadBody
            | Scenario::SharepointDownloadBody
            | Scenario::ShopifyImagesBody
    );
    let fail_signal_read = matches!(
        scenario,
        Scenario::SignalReadFailure | Scenario::ParallelSignalReadFailure
    );
    let parallel = matches!(
        scenario,
        Scenario::ParallelSplit
            | Scenario::EmbedParallel
            | Scenario::NestedParallelBranches
            | Scenario::NestedParallelSplit
            | Scenario::ParallelBranches
            | Scenario::WavefrontBranches
            | Scenario::ParallelSignalReadFailure
            | Scenario::PauseBranches
            | Scenario::ShutdownBranches
            | Scenario::PauseThenCancelBranches
            | Scenario::CheckpointCancelBranches
    );
    let expected_requests = if pre_cancel {
        0
    } else if scenario == Scenario::McpTool {
        3
    } else if parallel
        || matches!(
            scenario,
            Scenario::AiSummary
                | Scenario::AiMemorySave
                | Scenario::TeamsChunks
                | Scenario::StorageDownload(_, true)
                | Scenario::SqsDeleteAfterReceive
                | Scenario::StripeFinalizeAfterCreate
                | Scenario::QuickbooksUpdateAfterRead
                | Scenario::HubspotUpdateAfterRead
                | Scenario::SharepointContentAfterMetadata
                | Scenario::ShopifyDeleteAfterRead
        )
    {
        2
    } else {
        1
    };
    let host = Arc::new(Host {
        inner: PersistingRuntimeHost::new(b"{}"),
        requested: AtomicBool::new(pre_cancel),
        requests: AtomicUsize::new(0),
        closed: Notify::new(),
        closed_count: AtomicUsize::new(0),
        acknowledged: AtomicBool::new(false),
        fail_signal_read,
        scenario,
        observed: AtomicUsize::new(0),
        events: Mutex::new(Vec::new()),
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let url = format!("http://{}", listener.local_addr()?);
    let dir = tempfile::tempdir()?;
    let immediate = |value: Value| serde_json::json!({"valueType":"immediate", "value":value});
    let mut graph: Value = serde_json::json!({
        "durable":false, "entryPoint":"fetch", "steps": {
            "fetch":{"id":"fetch","stepType":"Agent","agentId":"http","capabilityId":"http-request",
                "maxRetries":3,"retryDelay":0,
                "inputMapping":{"url":immediate(url.clone().into()),"method":immediate("GET".into()),"timeout_ms":immediate(300_000.into())}},
            "finish":{"id":"finish","stepType":"Finish","inputMapping":{"unexpected":immediate(true.into())}},
            "handled":{"id":"handled","stepType":"Finish","inputMapping":{"recovered":immediate(true.into())}}
        }, "executionPlan":[{"fromStep":"fetch","toStep":"finish"},{"fromStep":"fetch","toStep":"handled","label":"onError"}]
    });
    if scenario == Scenario::SlackHeaders {
        graph["steps"]["fetch"]["agentId"] = "slack".into();
        graph["steps"]["fetch"]["capabilityId"] = "send-message".into();
        graph["steps"]["fetch"]["inputMapping"] = serde_json::json!({
            "channel":immediate("C-fixture".into()),
            "text":immediate("local cancellation fixture".into()),
            "_connection":immediate(serde_json::json!({"connection_id":"fixture-connection","integration_id":"slack_bot","parameters":{}}))
        });
    }
    if matches!(scenario, Scenario::Mailgun | Scenario::TeamsChunks) || scenario.is_mcp() {
        let (agent, capability, input) = match scenario {
            Scenario::Mailgun => (
                "mailgun",
                "send-email",
                serde_json::json!({"to":"fixture@example.invalid","subject":"fixture","text":"fixture","_connection":{"connection_id":"fixture-connection","integration_id":"mailgun","parameters":{"domain":"example.invalid"}}}),
            ),
            Scenario::TeamsChunks => (
                "teams",
                "send-message",
                serde_json::json!({"target":"fixture-ref","conversation_id":"fixture","text":"x".repeat(4001),"_connection":{"connection_id":"fixture-connection","integration_id":"teams_bot","parameters":{}}}),
            ),
            Scenario::McpInitialize | Scenario::McpTool => (
                "mcp",
                "mcp-tool-invoke",
                serde_json::json!({"tool_name":"echo","args":{"value":"fixture"},"_connection":{"connection_id":"fixture-connection","integration_id":"mcp","parameters":{"url":"https://mcp.invalid/rpc","tool_scope":["echo"]}}}),
            ),
            _ => unreachable!(),
        };
        graph["steps"]["fetch"]["agentId"] = agent.into();
        graph["steps"]["fetch"]["capabilityId"] = capability.into();
        graph["steps"]["fetch"]["inputMapping"] = input
            .as_object()
            .unwrap()
            .iter()
            .map(|(key, value)| (key.clone(), immediate(value.clone())))
            .collect::<serde_json::Map<String, Value>>()
            .into();
    }
    if scenario.is_sharepoint() || scenario.is_shopify() {
        let (agent, capability, input, integration) = if scenario.is_sharepoint() {
            (
                "sharepoint",
                "sharepoint-download-file",
                serde_json::json!({"drive_id":"drive","item_id":"42","as_text":true}),
                "microsoft_entra_client_credentials",
            )
        } else {
            (
                "shopify",
                "replace-product-images",
                serde_json::json!({"product_id":"42","images":[{"url":"https://image.invalid/new.png"}]}),
                "shopify",
            )
        };
        graph["steps"]["fetch"]["agentId"] = agent.into();
        graph["steps"]["fetch"]["capabilityId"] = capability.into();
        graph["steps"]["fetch"]["inputMapping"] = input
            .as_object()
            .unwrap()
            .iter()
            .map(|(key, value)| (key.clone(), immediate(value.clone())))
            .collect::<serde_json::Map<String, Value>>()
            .into();
        graph["steps"]["fetch"]["inputMapping"]["_connection"] = immediate(
            serde_json::json!({"connection_id":"fixture-connection","integration_id":integration,"parameters":{}}),
        );
    }
    if scenario.is_hubspot() {
        let connection = immediate(
            serde_json::json!({"connection_id":"fixture-connection","integration_id":"hubspot_private_app","parameters":{}}),
        );
        graph["steps"]["fetch"]["agentId"] = "hubspot".into();
        graph["steps"]["fetch"]["capabilityId"] = "get-contact".into();
        graph["steps"]["fetch"]["inputMapping"] =
            serde_json::json!({"contact_id":immediate("42".into()),"_connection":connection});
        graph["steps"]["update"] = serde_json::json!({"id":"update","stepType":"Agent","agentId":"hubspot","capabilityId":"update-contact","maxRetries":3,"retryDelay":0,
            "inputMapping":{"_connection":connection,"properties":immediate(serde_json::json!({"name":"updated fixture"})),"contact_id":{"valueType":"reference","value":"steps.fetch.outputs.contact.id"}}});
        graph["executionPlan"] = serde_json::json!([
            {"fromStep":"fetch","toStep":"update"}, {"fromStep":"fetch","toStep":"handled","label":"onError"},
            {"fromStep":"update","toStep":"finish"}, {"fromStep":"update","toStep":"handled","label":"onError"}
        ]);
    }
    if scenario.is_quickbooks() {
        let connection = immediate(
            serde_json::json!({"connection_id":"fixture-connection","integration_id":"quickbooks_online","parameters":{}}),
        );
        graph["steps"]["fetch"]["agentId"] = "quickbooks".into();
        graph["steps"]["fetch"]["capabilityId"] = "read".into();
        graph["steps"]["fetch"]["inputMapping"] = serde_json::json!({"entity":immediate("Customer".into()),"id":immediate("42".into()),"_connection":connection});
        graph["steps"]["update"] = serde_json::json!({"id":"update","stepType":"Agent","agentId":"quickbooks","capabilityId":"update","maxRetries":3,"retryDelay":0,
            "inputMapping":{"_connection":connection,"entity":immediate("Customer".into()),"body":immediate(serde_json::json!({"DisplayName":"updated fixture"})),"id":{"valueType":"reference","value":"steps.fetch.outputs.id"},"sync_token":{"valueType":"reference","value":"steps.fetch.outputs.sync_token"}}});
        graph["executionPlan"] = serde_json::json!([
            {"fromStep":"fetch","toStep":"update"}, {"fromStep":"fetch","toStep":"handled","label":"onError"},
            {"fromStep":"update","toStep":"finish"}, {"fromStep":"update","toStep":"handled","label":"onError"}
        ]);
    }
    if scenario.is_stripe() {
        let connection = immediate(
            serde_json::json!({"connection_id":"fixture-connection","integration_id":"stripe_api_key","parameters":{}}),
        );
        graph["steps"]["fetch"]["agentId"] = "stripe".into();
        graph["steps"]["fetch"]["capabilityId"] = "create-invoice".into();
        graph["steps"]["fetch"]["inputMapping"] = serde_json::json!({"customer":immediate("cus_fixture".into()),"_connection":connection});
        graph["steps"]["finalize"] = serde_json::json!({"id":"finalize","stepType":"Agent","agentId":"stripe","capabilityId":"finalize-invoice","maxRetries":3,"retryDelay":0,
            "inputMapping":{"_connection":connection,"invoice_id":{"valueType":"reference","value":"steps.fetch.outputs.invoice.id"}}});
        graph["executionPlan"] = serde_json::json!([
            {"fromStep":"fetch","toStep":"finalize"}, {"fromStep":"fetch","toStep":"handled","label":"onError"},
            {"fromStep":"finalize","toStep":"finish"}, {"fromStep":"finalize","toStep":"handled","label":"onError"}
        ]);
    }
    if scenario.is_sqs() {
        let queue = immediate("https://sqs.invalid/fixture/queue".into());
        let connection = immediate(
            serde_json::json!({"connection_id":"fixture-connection","integration_id":"aws_credentials","parameters":{}}),
        );
        graph["steps"]["fetch"]["agentId"] = "sqs".into();
        graph["steps"]["fetch"]["capabilityId"] = "queue-receive-messages".into();
        graph["steps"]["fetch"]["inputMapping"] = serde_json::json!({"queue_url":queue,"wait_time_seconds":immediate(20.into()),"_connection":connection});
        graph["steps"]["delete"] = serde_json::json!({"id":"delete","stepType":"Agent","agentId":"sqs","capabilityId":"queue-delete-message","maxRetries":3,"retryDelay":0,
            "inputMapping":{"queue_url":queue,"_connection":connection,"receipt_handle":{"valueType":"reference","value":"steps.fetch.outputs.messages.0.receipt_handle"}}});
        graph["executionPlan"] = serde_json::json!([
            {"fromStep":"fetch","toStep":"delete"}, {"fromStep":"fetch","toStep":"handled","label":"onError"},
            {"fromStep":"delete","toStep":"finish"}, {"fromStep":"delete","toStep":"handled","label":"onError"}
        ]);
    }
    if scenario.is_storage() {
        let (agent, capability) = match scenario {
            Scenario::StorageDownload(agent, _) => (agent, "storage-download-file"),
            Scenario::StoragePresign(agent) => (agent, "storage-generate-presigned-url"),
            Scenario::Sftp => ("sftp", "sftp-download-file"),
            _ => unreachable!(),
        };
        let integration = match agent {
            "s3-storage" => "s3_compatible",
            "azure-blob-storage" => "azure_blob_storage",
            _ => "sftp",
        };
        graph["steps"]["fetch"]["agentId"] = agent.into();
        graph["steps"]["fetch"]["capabilityId"] = capability.into();
        graph["steps"]["fetch"]["inputMapping"] = serde_json::json!({
            "bucket":immediate("bucket".into()), "key":immediate("file.txt".into()),
            "path":immediate("/file.txt".into()), "operation":immediate("download".into()),
            "_connection":immediate(serde_json::json!({"connection_id":"fixture-connection","integration_id":integration,"parameters":{}}))
        });
    }
    if scenario.is_object() {
        graph["steps"]["fetch"]["agentId"] = "object-model".into();
        graph["steps"]["fetch"]["capabilityId"] = if scenario == Scenario::ObjectQuery {
            "query-sql"
        } else {
            "execute-sql"
        }
        .into();
        graph["steps"]["fetch"]["inputMapping"] = serde_json::json!({
            "sql":immediate("SELECT 1".into()), "params":immediate(serde_json::json!([])),
            "_connection":immediate(serde_json::json!({"connection_id":"fixture-connection","integration_id":"object_model","parameters":{}}))
        });
    }
    if scenario.is_ai() {
        graph = serde_json::from_str(&match scenario {
            Scenario::AiSingle => single_shot_ai_agent_graph_json(""),
            Scenario::AiTurn => ai_agent_tool_loop_durable_graph_json(false),
            Scenario::AiSummary | Scenario::AiMemoryLoad | Scenario::AiMemorySave => {
                ai_agent_memory_graph_json()
            }
            _ => unreachable!(),
        })?;
        graph["durable"] = false.into();
        graph["steps"]["handled"] = serde_json::json!({"id":"handled","stepType":"Finish","inputMapping":{"recovered":immediate(true.into())}});
        graph["executionPlan"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({"fromStep":"ai","toStep":"handled","label":"onError"}));
        if scenario == Scenario::AiSummary {
            graph["steps"]["ai"]["config"]["memory"]["compaction"]["strategy"] = "summarize".into();
        }
    }
    if parallel {
        graph = if matches!(
            scenario,
            Scenario::ParallelSplit
                | Scenario::ParallelSignalReadFailure
                | Scenario::NestedParallelSplit
        ) {
            serde_json::from_str(&parallel_http_split_graph(&url, 2))?
        } else {
            serde_json::from_str(&parallel_http_branches_graph(&url, true))?
        };
        if scenario == Scenario::WavefrontBranches {
            for branch in ["b", "c"] {
                let wait = format!("wait-{branch}");
                graph["steps"][&wait] =
                    serde_json::json!({"id":wait,"stepType":"WaitForSignal","pollIntervalMs":0});
                let edges = graph["executionPlan"].as_array_mut().unwrap();
                for edge in edges.iter_mut() {
                    if edge["fromStep"] == branch {
                        edge["toStep"] = wait.clone().into();
                    }
                }
                edges.push(serde_json::json!({"fromStep":wait,"toStep":"finish"}));
            }
        }
    }
    if scenario.nested_depth() > 0 {
        // Match the production publish contract: no durable suspension or
        // shared root runtime import inside a published workflow-agent.
        graph["durable"] = false.into();
    }
    if matches!(scenario, Scenario::WhileBody | Scenario::EmbedWhileBody) {
        let mut wrapper = loop_boundaries::loop_graph(false, 3);
        wrapper["steps"]["loop"]["subgraph"] = graph;
        graph = wrapper;
    }
    if scenario == Scenario::WhileParallel {
        graph = loop_boundaries::parallel_graph(graph["steps"]["fetch"].clone());
    }
    let mut children = vec![];
    if matches!(
        scenario,
        Scenario::EmbedBody
            | Scenario::EmbedPartialBody
            | Scenario::EmbedWhileBody
            | Scenario::EmbedParallel
    ) {
        for id in ["inner-embed", "outer-embed"] {
            children.push(runtara_workflows::compile::ChildWorkflowInput {
                step_id: id.into(),
                workflow_id: id.into(),
                version_requested: "latest".into(),
                version_resolved: 1,
                execution_graph: serde_json::from_value(graph)?,
            });
            graph = serde_json::json!({"durable":true,"entryPoint":id,"steps":{
                id:{"id":id,"stepType":"EmbedWorkflow","childWorkflowId":id,"childVersion":"latest","maxRetries":3},
                "finish":{"id":"finish","stepType":"Finish"},
                "handled":{"id":"handled","stepType":"Finish"}
            },"executionPlan":[{"fromStep":id,"toStep":"finish"},{"fromStep":id,"toStep":"handled","label":"onError"}]});
        }
    }
    let graph = serde_json::from_value(graph)?;
    let compiled = if scenario.nested_depth() > 0 {
        compile_nested_agents(graph, scenario.nested_depth(), dir.path())?
    } else {
        compile_direct_workflow_composed_configured(
            DirectCompilationInput {
                workflow_id: "cooperative-http".into(),
                version: 1,
                source_checksum: None,
                execution_graph: graph,
                child_workflows: children,
                output_dir: dir.path().into(),
                track_events: false,
                agent_catalog: None,
                agent_slug: None,
            },
            direct_e2e_components_dir(),
            RuntimeBinding::HostImport,
            WorkflowAbi::InvokeHostImports,
            false,
        )?
    };
    anyhow::ensure!(
        compiled.scoped_agents.is_empty(),
        "fixture selected isolated Agent adapters"
    );
    anyhow::ensure!(
        compiled.invocation_manifest.is_none(),
        "fixture emitted an isolation inventory"
    );
    let bytes = fs::read(&compiled.wasm_path)?;
    anyhow::ensure!(
        !bytes
            .windows(b"runtara:workflow-execution/tasks".len())
            .any(|bytes| bytes == b"runtara:workflow-execution/tasks"),
        "fixture contains the superseded task interface"
    );
    let server_host = host.clone();
    let server = tokio::spawn(async move {
        let mut connections = tokio::task::JoinSet::new();
        loop {
            tokio::select! {
                accepted = listener.accept() => {
                    let (mut stream, _) = accepted?;
                    let server_host = server_host.clone();
                    connections.spawn(async move {
                        let mut request = vec![];
                        let mut buffer = [0; 1024];
                        while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                            let n = stream.read(&mut buffer).await?;
                            anyhow::ensure!(n != 0, "request closed before headers");
                            request.extend_from_slice(&buffer[..n]);
                        }
                        // Read the entire internal request before issuing a response or
                        // cancellation. Memory-save payloads can span multiple packets.
                        if scenario.is_ai() || scenario.is_object() || scenario.is_storage() {
                            let end = request.windows(4).position(|bytes| bytes == b"\r\n\r\n").unwrap() + 4;
                            let headers = std::str::from_utf8(&request[..end])?;
                            let length = headers.lines().filter_map(|line| line.split_once(':'))
                                .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                                .map(|(_, length)| length.trim().parse::<usize>()).transpose()?.unwrap_or(0);
                            anyhow::ensure!(length < 16_384, "unexpected internal request size");
                            while request.len() < end + length {
                                let n = stream.read(&mut buffer).await?;
                                anyhow::ensure!(n > 0, "internal request body closed early");
                                request.extend_from_slice(&buffer[..n]);
                            }
                            if scenario.is_object() {
                                let path = if scenario == Scenario::ObjectQuery {"query"} else {"execute"};
                                assert!(request.starts_with(format!("POST /sql/{path}?connectionId=fixture-connection ").as_bytes()));
                                let body: Value = serde_json::from_slice(&request[end..end + length])?;
                                assert_eq!(body["connectionId"], "fixture-connection");
                                assert_eq!(body["sql"], "SELECT 1");
                            }
                            if matches!(scenario, Scenario::StoragePresign(_) | Scenario::Sftp) {
                                let body: Value = serde_json::from_slice(&request[end..end + length])?;
                                if scenario == Scenario::Sftp {
                                    assert!(request.starts_with(b"POST /agent/sftp/sftp-download-file "));
                                    assert_eq!(body["_connection"]["connection_id"], "fixture-connection");
                                    assert_eq!(body["path"], "/file.txt");
                                } else {
                                    assert!(request.starts_with(b"POST /presign "));
                                    assert_eq!(body["connection_id"], "fixture-connection");
                                    assert_eq!(body["method"], "GET");
                                    assert_eq!(body["path"], "/bucket/file.txt");
                                }
                            }
                        }
                        if scenario.is_ai() {
                            let end = request.windows(4).position(|bytes| bytes == b"\r\n\r\n").unwrap() + 4;
                            let headers = std::str::from_utf8(&request[..end])?;
                            let line = headers.lines().next().unwrap();
                            if line.starts_with("GET /fixture-tenant/conn-1/metadata ") {
                                let bytes = serde_json::to_vec(&serde_json::json!({"connectionId":"conn-1","integrationId":"openai_api_key","status":"ACTIVE","resources":[],"metadata":null}))?;
                                stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", bytes.len()).as_bytes()).await?;
                                stream.write_all(&bytes).await?;
                                return anyhow::Ok(());
                            }
                            if line.contains(" /object-model/") {
                                // The summary fixture permits only memory loading. The
                                // load/save fixtures cancel at their selected request.
                                assert!(matches!(scenario, Scenario::AiSummary | Scenario::AiMemoryLoad | Scenario::AiMemorySave));
                                let cancel_load = scenario == Scenario::AiMemoryLoad && line.starts_with("POST /object-model/instances/query?");
                                let cancel_save = scenario == Scenario::AiMemorySave && line.starts_with("POST /object-model/instances?connectionId=");
                                if cancel_load || cancel_save {
                                    let started = server_host.requests.fetch_add(1, Ordering::SeqCst) + 1;
                                    assert_eq!(started, expected_requests);
                                    server_host.requested.store(true, Ordering::SeqCst);
                                    loop {
                                        match stream.read(&mut buffer).await {
                                            Ok(0) => break,
                                            Ok(_) => {},
                                            Err(e) if matches!(e.kind(), std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::BrokenPipe) => break,
                                            Err(e) => return Err(e.into()),
                                        }
                                    }
                                    server_host.closed_count.fetch_add(1, Ordering::SeqCst);
                                    server_host.closed.notify_one();
                                    return anyhow::Ok(());
                                }
                                let reply = if line.starts_with("GET /object-model/schemas/ai_conversation_memory?connectionId=conn-1 ") {
                                    serde_json::json!({"success":true,"schema":{}})
                                } else {
                                    assert!(line.starts_with("POST /object-model/instances/query?connectionId=conn-1 "), "memory was mutated after cancellation: {line}");
                                    serde_json::json!({"success":true,"instances":[]})
                                };
                                let bytes = serde_json::to_vec(&reply)?;
                                stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", bytes.len()).as_bytes()).await?;
                                stream.write_all(&bytes).await?;
                                return anyhow::Ok(());
                            }
                        }
                        if scenario.uses_proxy() {
                            let end = request.windows(4).position(|bytes| bytes == b"\r\n\r\n").unwrap() + 4;
                            let headers = std::str::from_utf8(&request[..end])?;
                            anyhow::ensure!(headers.starts_with("POST / "), "Agent request bypassed local proxy");
                            let length: usize = headers.lines().filter_map(|line| line.split_once(':'))
                                .find(|(name, _)| name.eq_ignore_ascii_case("content-length")).unwrap().1.trim().parse()?;
                            anyhow::ensure!(length < 16_384, "unexpected proxy request size");
                            while request.len() < end + length {
                                let n = stream.read(&mut buffer).await?;
                                anyhow::ensure!(n > 0, "proxy body closed early");
                                request.extend_from_slice(&buffer[..n]);
                            }
                            let body: Value = serde_json::from_slice(&request[end..end + length])?;
                            if scenario.is_ai() {
                                assert_eq!(body["url"], "/v1/chat/completions");
                                assert_eq!(body["connection_id"], "conn-1");
                                assert_eq!(body["ai_provider"], "openai");
                                if scenario == Scenario::AiSummary && server_host.requests.load(Ordering::SeqCst) == 1 {
                                    assert!(body["body"]["messages"][0]["content"].as_str().unwrap().contains("conversation summarizer"));
                                }
                            } else {
                                assert_eq!(body["connection_id"], "fixture-connection");
                                match scenario {
                                    Scenario::SharepointDownload | Scenario::SharepointDownloadBody | Scenario::SharepointContentAfterMetadata => {
                                        assert_eq!(body["method"],"GET");
                                        if server_host.requests.load(Ordering::SeqCst) == 0 {
                                            assert_eq!(body["url"],"https://graph.microsoft.com/v1.0/drives/drive/items/42");
                                            assert_eq!(body["timeout_ms"],30_000);
                                        } else {
                                            assert_eq!(body["url"],"https://graph.microsoft.com/v1.0/drives/drive/items/42/content");
                                            assert_eq!(body["timeout_ms"],120_000);
                                        }
                                    },
                                    Scenario::ShopifyImages | Scenario::ShopifyImagesBody | Scenario::ShopifyDeleteAfterRead => {
                                        assert_eq!(body["method"],"POST");
                                        assert_eq!(body["url"],"/admin/api/2025-01/graphql.json");
                                        assert_eq!(body["timeout_ms"],60_000);
                                        let payload = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, body["body_raw"].as_str().unwrap())?;
                                        let payload:Value = serde_json::from_slice(&payload)?;
                                        assert_eq!(payload["variables"], if server_host.requests.load(Ordering::SeqCst)==0 {serde_json::json!({"productId":"42"})} else {serde_json::json!({"fileIds":["old-image"]})});
                                    },
                                    Scenario::HubspotRead | Scenario::HubspotReadBody | Scenario::HubspotUpdateAfterRead => {
                                        assert_eq!(body["timeout_ms"],30_000);
                                        assert_eq!(body["url"],"https://api.hubapi.com/crm/v3/objects/contacts/42");
                                        if server_host.requests.load(Ordering::SeqCst) == 0 {
                                            assert_eq!(body["method"],"GET");
                                        } else {
                                            assert_eq!(body["method"],"PATCH");
                                            let payload = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, body["body_raw"].as_str().unwrap())?;
                                            assert_eq!(serde_json::from_slice::<Value>(&payload)?,serde_json::json!({"properties":{"name":"updated fixture"}}));
                                        }
                                    },
                                    Scenario::QuickbooksRead | Scenario::QuickbooksReadBody | Scenario::QuickbooksUpdateAfterRead => {
                                        assert_eq!(body["timeout_ms"], 30_000);
                                        if server_host.requests.load(Ordering::SeqCst) == 0 {
                                            assert_eq!(body["method"], "GET");
                                            assert_eq!(body["url"], "/customer/42?minorversion=75");
                                        } else {
                                            assert_eq!(body["method"], "POST");
                                            assert_eq!(body["url"], "/customer?minorversion=75");
                                            let payload = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, body["body_raw"].as_str().unwrap())?;
                                            assert_eq!(serde_json::from_slice::<Value>(&payload)?, serde_json::json!({"Id":"42","SyncToken":"3","sparse":true,"DisplayName":"updated fixture"}));
                                        }
                                    },
                                    Scenario::StripeCreate | Scenario::StripeCreateBody | Scenario::StripeFinalizeAfterCreate => {
                                        assert_eq!(body["method"], "POST");
                                        assert_eq!(body["timeout_ms"], 30_000);
                                        assert_eq!(body["headers"]["Content-Type"], "application/x-www-form-urlencoded");
                                        let payload = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, body["body_raw"].as_str().unwrap())?;
                                        if server_host.requests.load(Ordering::SeqCst) == 0 {
                                            assert_eq!(body["url"], "/v1/invoices");
                                            assert_eq!(payload, b"customer=cus_fixture");
                                        } else {
                                            assert_eq!(body["url"], "/v1/invoices/in_fixture/finalize");
                                            assert!(payload.is_empty());
                                        }
                                    },
                                    Scenario::SqsReceive | Scenario::SqsReceiveBody | Scenario::SqsDeleteAfterReceive => {
                                        assert_eq!(body["aws_service"], "sqs");
                                        assert_eq!(body["url"], "/");
                                        let bytes = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, body["body_raw"].as_str().unwrap())?;
                                        let payload: Value = serde_json::from_slice(&bytes)?;
                                        assert_eq!(payload["QueueUrl"], "https://sqs.invalid/fixture/queue");
                                        if server_host.requests.load(Ordering::SeqCst) == 0 {
                                            assert_eq!(body["headers"]["X-Amz-Target"], "AmazonSQS.ReceiveMessage");
                                            assert_eq!(payload["WaitTimeSeconds"], 20);
                                        } else {
                                            assert_eq!(body["headers"]["X-Amz-Target"], "AmazonSQS.DeleteMessage");
                                            assert_eq!(payload["ReceiptHandle"], "fixture-receipt");
                                        }
                                    },
                                    Scenario::StorageDownload(_, _) => {
                                        assert_eq!(body["url"], "/bucket/file.txt");
                                        assert_eq!(body["method"], if server_host.requests.load(Ordering::SeqCst) == 0 {"HEAD"} else {"GET"});
                                    },
                                    Scenario::SlackHeaders => assert_eq!(body["url"], "https://slack.com/api/chat.postMessage"),
                                    Scenario::Mailgun => assert_eq!(body["url"], "/v3/example.invalid/messages"),
                                    Scenario::TeamsChunks => {
                                        assert_eq!(body["url"], "/v3/conversations/fixture/activities");
                                        assert_eq!(body["endpoint_ref"], "fixture-ref");
                                    },
                                    Scenario::McpInitialize | Scenario::McpTool => {
                                        assert_eq!(body["url"], "https://mcp.invalid/rpc");
                                        let stage = server_host.requests.load(Ordering::SeqCst);
                                        assert_eq!(body["body"]["method"], match stage {0=>"initialize",1=>"notifications/initialized",_=>"tools/call"});
                                        if stage > 0 { assert_eq!(body["headers"]["Mcp-Session-Id"], "fixture-session"); }
                                    },
                                    _ => unreachable!(),
                                }
                            }
                        }
                        if partial_body {
                            stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\nConnection: close\r\n\r\nx").await?;
                        }
                        let started = server_host.requests.fetch_add(1, Ordering::SeqCst) + 1;
                        if matches!(scenario, Scenario::SharepointContentAfterMetadata | Scenario::ShopifyDeleteAfterRead) && started == 1 {
                            let body = if scenario.is_sharepoint() { serde_json::json!({"id":"42","name":"fixture.txt"}) } else { serde_json::json!({"data":{"product":{"media":{"edges":[{"node":{"id":"old-image"}}]}}}}) };
                            let bytes=serde_json::to_vec(&serde_json::json!({"status":200,"headers":{},"body":body}))?;
                            stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",bytes.len()).as_bytes()).await?;
                            stream.write_all(&bytes).await?;
                            server_host.closed_count.fetch_add(1,Ordering::SeqCst);
                            server_host.closed.notify_one();
                            return anyhow::Ok(());
                        }
                        if scenario == Scenario::HubspotUpdateAfterRead && started == 1 {
                            let bytes = serde_json::to_vec(&serde_json::json!({"status":200,"headers":{},"body":{"id":"42","properties":{"name":"fixture"}}}))?;
                            stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", bytes.len()).as_bytes()).await?;
                            stream.write_all(&bytes).await?;
                            server_host.closed_count.fetch_add(1, Ordering::SeqCst);
                            server_host.closed.notify_one();
                            return anyhow::Ok(());
                        }
                        if scenario == Scenario::QuickbooksUpdateAfterRead && started == 1 {
                            let bytes = serde_json::to_vec(&serde_json::json!({"status":200,"headers":{},"body":{"Customer":{"Id":"42","SyncToken":"3","DisplayName":"fixture"}}}))?;
                            stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", bytes.len()).as_bytes()).await?;
                            stream.write_all(&bytes).await?;
                            server_host.closed_count.fetch_add(1, Ordering::SeqCst);
                            server_host.closed.notify_one();
                            return anyhow::Ok(());
                        }
                        if scenario == Scenario::StripeFinalizeAfterCreate && started == 1 {
                            let bytes = serde_json::to_vec(&serde_json::json!({"status":200,"headers":{},"body":{"id":"in_fixture","status":"draft"}}))?;
                            stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", bytes.len()).as_bytes()).await?;
                            stream.write_all(&bytes).await?;
                            server_host.closed_count.fetch_add(1, Ordering::SeqCst);
                            server_host.closed.notify_one();
                            return anyhow::Ok(());
                        }
                        if scenario == Scenario::SqsDeleteAfterReceive && started == 1 {
                            let bytes = serde_json::to_vec(&serde_json::json!({"status":200,"headers":{},"body":{"Messages":[{"MessageId":"one","ReceiptHandle":"fixture-receipt","Body":"hello"}]}}))?;
                            stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", bytes.len()).as_bytes()).await?;
                            stream.write_all(&bytes).await?;
                            server_host.closed_count.fetch_add(1, Ordering::SeqCst);
                            server_host.closed.notify_one();
                            return anyhow::Ok(());
                        }
                        if matches!(scenario, Scenario::StorageDownload(_, true)) && started == 1 {
                            let bytes = serde_json::to_vec(&serde_json::json!({"status":200,"headers":{"content-type":"text/plain"},"body_raw":""}))?;
                            stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", bytes.len()).as_bytes()).await?;
                            stream.write_all(&bytes).await?;
                            server_host.closed_count.fetch_add(1, Ordering::SeqCst);
                            server_host.closed.notify_one();
                            return anyhow::Ok(());
                        }
                        if (scenario == Scenario::TeamsChunks && started == 1) || (scenario == Scenario::McpTool && started < 3) {
                            let body = if scenario == Scenario::TeamsChunks {serde_json::json!({"id":"first"})} else {serde_json::json!({"jsonrpc":"2.0","id":1,"result":{}})};
                            let bytes = serde_json::to_vec(&serde_json::json!({"status":if scenario.is_mcp() && started==2 {202} else {200},"headers":{"mcp-session-id":"fixture-session"},"body":body}))?;
                            stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",bytes.len()).as_bytes()).await?;
                            stream.write_all(&bytes).await?;
                            server_host.closed_count.fetch_add(1,Ordering::SeqCst);
                            server_host.closed.notify_one();
                            return anyhow::Ok(());
                        }
                        if matches!(scenario, Scenario::AiSummary | Scenario::AiMemorySave) && started == 1 {
                            let bytes = serde_json::to_vec(&llm_ok("completed first turn"))?;
                            stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", bytes.len()).as_bytes()).await?;
                            stream.write_all(&bytes).await?;
                            server_host.closed_count.fetch_add(1, Ordering::SeqCst);
                            server_host.closed.notify_one();
                            return anyhow::Ok(());
                        }
                        if started == expected_requests {
                            server_host.requested.store(true, Ordering::SeqCst);
                        }
                        let respond = scenario.drains_normally() || (scenario == Scenario::CheckpointCancelBranches && started == 1);
                        if respond {
                            while server_host.observed.load(Ordering::SeqCst) == 0 {
                                tokio::time::sleep(Duration::from_millis(10)).await;
                            }
                            stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}").await?;
                        }
                        if !respond { loop {
                            match stream.read(&mut buffer).await {
                                Ok(0) => break,
                                Ok(_) => {},
                                Err(e) if matches!(e.kind(), std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::BrokenPipe) => break,
                                Err(e) => return Err(e.into()),
                            }
                        }
                        }
                        server_host.closed_count.fetch_add(1, Ordering::SeqCst);
                        server_host.closed.notify_one();
                        anyhow::Ok(())
                    });
                },
                finished = connections.join_next(), if !connections.is_empty() => { finished.unwrap()??; }
            }
        }
        #[allow(unreachable_code)]
        Ok::<_, anyhow::Error>(())
    });
    let result = async {
        let executor = embedded_executor();
        let pre = executor.load_instance_pre(&compiled.wasm_path).await?;
        let run = executor
            .execute_invoke(
                &pre,
                runtara_component_host::WorkflowRunSpec {
                    env: if scenario.uses_proxy() || scenario.is_object() || scenario.is_storage() {
                        HashMap::from([("RUNTARA_HTTP_PROXY_URL".into(), url.clone()), ("RUNTARA_TENANT_ID".into(), "fixture-tenant".into()), ("CONNECTION_SERVICE_URL".into(), url.clone()), ("RUNTARA_AGENT_SERVICE_URL".into(), format!("{url}/agent")), ("RUNTARA_OBJECT_MODEL_URL".into(), if scenario.is_object() {url.clone()} else {format!("{url}/object-model")})])
                    } else { HashMap::new() },
                    stderr: None,
                    timeout: Duration::from_secs(10),
                    cancel: None,
                    limits: Default::default(),
                    runtime: Some(host.clone()),
                },
                br#"{"data":{"items":[1,2]}}"#.to_vec(),
            )
            .await;
        if fail_signal_read {
            anyhow::ensure!(matches!(&run.exit, runtara_component_host::InvokeExit::Failed(error) if error.message == "signal delivery failed"), "signal error lost: {:?}", run.exit);
            tokio::time::timeout(Duration::from_secs(2), host.wait_closed()).await?;
            anyhow::ensure!(!host.acknowledged.load(Ordering::SeqCst), "failed signal read was acknowledged");
        } else {
            anyhow::ensure!(matches!(run.exit, runtara_component_host::InvokeExit::Suspended(_)), "expected lifecycle stop, got {:?}; requests={}; error={:?}", run.exit, host.requests.load(Ordering::SeqCst), host.inner.failed.lock().unwrap());
            anyhow::ensure!(host.acknowledged.load(Ordering::SeqCst), "WASM did not acknowledge the command");
        }
        anyhow::ensure!(
            host.requests.load(Ordering::SeqCst) == expected_requests,
            "unexpected HTTP retry or pre-cancel invocation"
        );
        anyhow::ensure!(
            host.inner.completed.lock().unwrap().is_none(),
            "normal/onError path ran after root cancellation"
        );
        if fail_signal_read {
            anyhow::ensure!(host.inner.failed.lock().unwrap().as_deref() == Some(b"signal delivery failed"), "signal transport error publication changed");
        } else {
            anyhow::ensure!(host.inner.failed.lock().unwrap().is_none(), "root cancellation became an ordinary step failure");
        }
        if scenario == Scenario::CheckpointCancelBranches {
            let checkpoints = host.inner.checkpoints.lock().unwrap();
            let completed = ["b", "c"].iter().filter(|step| checkpoints.keys().any(|key| key.ends_with(&format!("\"{step}\"]]")))).count();
            anyhow::ensure!(completed == 1, "cancellation lost the completed sibling checkpoint or checkpointed cancelled work");
        }
        if scenario.drains_normally() {
            let resumed = executor.execute_invoke(&pre, runtara_component_host::WorkflowRunSpec {
                env: HashMap::new(), stderr: None, timeout: Duration::from_secs(10), cancel: None,
                limits: Default::default(), runtime: Some(host.clone()),
            }, b"{}".to_vec()).await;
            anyhow::ensure!(matches!(resumed.exit, runtara_component_host::InvokeExit::Completed(_)), "resume failed: {:?}", resumed.exit);
            anyhow::ensure!(host.requests.load(Ordering::SeqCst) == expected_requests, "resume re-fired a checkpointed sibling");
        }
        anyhow::Ok(())
    }
    .await;
    server.abort();
    let _ = server.await;
    result
}

#[tokio::test]
async fn emitted_cancel_before_agent_launch_does_not_send_http() -> anyhow::Result<()> {
    run(Scenario::BeforeLaunch).await
}
#[tokio::test]
async fn emitted_cancel_interrupts_pending_headers_without_retry_or_recovery() -> anyhow::Result<()>
{
    run(Scenario::Headers).await
}
#[tokio::test]
async fn emitted_cancel_interrupts_partial_body_without_retry_or_recovery() -> anyhow::Result<()> {
    run(Scenario::PartialBody).await
}

#[tokio::test]
async fn emitted_signal_poll_failure_cleans_up_http_before_failing() -> anyhow::Result<()> {
    run(Scenario::SignalReadFailure).await
}

#[tokio::test]
async fn emitted_cancel_cleans_every_parallel_split_call() -> anyhow::Result<()> {
    run(Scenario::ParallelSplit).await
}

#[tokio::test]
async fn emitted_cancel_cleans_every_scheduled_branch() -> anyhow::Result<()> {
    run(Scenario::ParallelBranches).await
}

#[tokio::test]
async fn emitted_cancel_cleans_every_wavefront_branch() -> anyhow::Result<()> {
    run(Scenario::WavefrontBranches).await
}

#[tokio::test]
async fn emitted_signal_poll_failure_cleans_every_parallel_call() -> anyhow::Result<()> {
    run(Scenario::ParallelSignalReadFailure).await
}

#[tokio::test]
async fn emitted_pause_observed_once_checkpoints_every_sibling_before_ack() -> anyhow::Result<()> {
    run(Scenario::PauseBranches).await
}
#[tokio::test]
async fn emitted_shutdown_observed_once_checkpoints_every_sibling_before_ack() -> anyhow::Result<()>
{
    run(Scenario::ShutdownBranches).await
}
#[tokio::test]
async fn emitted_cancel_supersedes_pause_while_parallel_calls_hang() -> anyhow::Result<()> {
    run(Scenario::PauseThenCancelBranches).await
}
#[tokio::test]
async fn emitted_checkpoint_cancel_cleans_pending_sibling_before_ack() -> anyhow::Result<()> {
    run(Scenario::CheckpointCancelBranches).await
}

#[tokio::test]
async fn emitted_cancel_interrupts_slack_without_retry_or_recovery() -> anyhow::Result<()> {
    run(Scenario::SlackHeaders).await
}

#[tokio::test]
async fn emitted_ai_single_shot_cancels_without_on_error_recovery() -> anyhow::Result<()> {
    run(Scenario::AiSingle).await
}
#[tokio::test]
async fn emitted_ai_turn_cancels_without_dispatching_tools_or_recovery() -> anyhow::Result<()> {
    run(Scenario::AiTurn).await
}
#[tokio::test]
async fn emitted_ai_summary_cancels_without_saving_fallback_memory() -> anyhow::Result<()> {
    run(Scenario::AiSummary).await
}

#[tokio::test]
async fn emitted_ai_memory_load_cancels_before_calling_the_model() -> anyhow::Result<()> {
    run(Scenario::AiMemoryLoad).await
}
#[tokio::test]
async fn emitted_ai_memory_save_cancels_without_completing_workflow() -> anyhow::Result<()> {
    run(Scenario::AiMemorySave).await
}
#[tokio::test]
async fn emitted_sql_query_cancel_bypasses_retries_and_recovery() -> anyhow::Result<()> {
    run(Scenario::ObjectQuery).await
}
#[tokio::test]
async fn emitted_sql_execute_cancel_bypasses_retries_and_recovery() -> anyhow::Result<()> {
    run(Scenario::ObjectExecute).await
}

#[tokio::test]
async fn emitted_mailgun_cancel_bypasses_retries_and_recovery() -> anyhow::Result<()> {
    run(Scenario::Mailgun).await
}
#[tokio::test]
async fn emitted_teams_cancel_interrupts_second_chunk_without_recovery() -> anyhow::Result<()> {
    run(Scenario::TeamsChunks).await
}
#[tokio::test]
async fn emitted_mcp_cancel_interrupts_initialization_without_recovery() -> anyhow::Result<()> {
    run(Scenario::McpInitialize).await
}
#[tokio::test]
async fn emitted_mcp_cancel_interrupts_tool_after_handshake_without_recovery() -> anyhow::Result<()>
{
    run(Scenario::McpTool).await
}

#[tokio::test]
async fn emitted_storage_download_cancel_stops_head_and_get_without_recovery() -> anyhow::Result<()>
{
    for agent in ["s3-storage", "azure-blob-storage"] {
        for after_head in [false, true] {
            run(Scenario::StorageDownload(agent, after_head)).await?;
        }
    }
    Ok(())
}

#[tokio::test]
async fn emitted_storage_presign_cancel_bypasses_soft_failure_and_recovery() -> anyhow::Result<()> {
    for agent in ["s3-storage", "azure-blob-storage"] {
        run(Scenario::StoragePresign(agent)).await?;
    }
    Ok(())
}

#[tokio::test]
async fn emitted_sftp_cancel_stops_native_service_wait_without_recovery() -> anyhow::Result<()> {
    run(Scenario::Sftp).await
}

#[tokio::test]
async fn emitted_sqs_cancel_stops_long_poll_without_deleting_or_retrying() -> anyhow::Result<()> {
    run(Scenario::SqsReceive).await
}

#[tokio::test]
async fn emitted_sqs_cancel_stops_partial_receive_without_deleting_or_retrying()
-> anyhow::Result<()> {
    run(Scenario::SqsReceiveBody).await
}

#[tokio::test]
async fn emitted_sqs_cancel_stops_delete_after_received_message_without_recovery()
-> anyhow::Result<()> {
    run(Scenario::SqsDeleteAfterReceive).await
}

#[tokio::test]
async fn emitted_stripe_cancel_stops_pending_invoice_without_finalize_or_retry()
-> anyhow::Result<()> {
    run(Scenario::StripeCreate).await
}

#[tokio::test]
async fn emitted_stripe_cancel_stops_partial_invoice_without_finalize_or_retry()
-> anyhow::Result<()> {
    run(Scenario::StripeCreateBody).await
}

#[tokio::test]
async fn emitted_stripe_cancel_stops_finalize_after_create_without_recovery() -> anyhow::Result<()>
{
    run(Scenario::StripeFinalizeAfterCreate).await
}

#[tokio::test]
async fn emitted_quickbooks_cancel_stops_read_without_update_or_retry() -> anyhow::Result<()> {
    run(Scenario::QuickbooksRead).await
}

#[tokio::test]
async fn emitted_quickbooks_cancel_stops_partial_read_without_update_or_retry() -> anyhow::Result<()>
{
    run(Scenario::QuickbooksReadBody).await
}

#[tokio::test]
async fn emitted_quickbooks_cancel_stops_update_after_read_without_recovery() -> anyhow::Result<()>
{
    run(Scenario::QuickbooksUpdateAfterRead).await
}

#[tokio::test]
async fn emitted_hubspot_cancel_stops_read_without_update_or_retry() -> anyhow::Result<()> {
    run(Scenario::HubspotRead).await
}
#[tokio::test]
async fn emitted_hubspot_cancel_stops_partial_read_without_update_or_retry() -> anyhow::Result<()> {
    run(Scenario::HubspotReadBody).await
}
#[tokio::test]
async fn emitted_hubspot_cancel_stops_update_after_read_without_recovery() -> anyhow::Result<()> {
    run(Scenario::HubspotUpdateAfterRead).await
}

#[tokio::test]
async fn emitted_workflow_cancel_sharepoint_metadata() -> anyhow::Result<()> {
    run(Scenario::SharepointDownload).await
}

#[tokio::test]
async fn emitted_workflow_cancel_sharepoint_metadata_body() -> anyhow::Result<()> {
    run(Scenario::SharepointDownloadBody).await
}

#[tokio::test]
async fn emitted_workflow_cancel_sharepoint_content_after_metadata() -> anyhow::Result<()> {
    run(Scenario::SharepointContentAfterMetadata).await
}

#[tokio::test]
async fn emitted_workflow_cancel_shopify_media_read() -> anyhow::Result<()> {
    run(Scenario::ShopifyImages).await
}

#[tokio::test]
async fn emitted_workflow_cancel_shopify_media_body() -> anyhow::Result<()> {
    run(Scenario::ShopifyImagesBody).await
}

#[tokio::test]
async fn emitted_workflow_cancel_shopify_delete_after_read() -> anyhow::Result<()> {
    run(Scenario::ShopifyDeleteAfterRead).await
}

#[tokio::test]
async fn emitted_nested_agent_cancel_cleans_http_before_root_ack() -> anyhow::Result<()> {
    run(Scenario::NestedAgent).await
}

#[tokio::test]
async fn emitted_nested_agent_cancel_cleans_partial_body_before_root_ack() -> anyhow::Result<()> {
    run(Scenario::NestedAgentBody).await
}

#[tokio::test]
async fn emitted_nested_agent_cancel_unwinds_two_published_levels() -> anyhow::Result<()> {
    run(Scenario::DeepNestedAgent).await
}

#[tokio::test]
async fn emitted_nested_agent_cancel_drains_parallel_branches() -> anyhow::Result<()> {
    run(Scenario::NestedParallelBranches).await
}

#[tokio::test]
async fn emitted_nested_agent_cancel_drains_parallel_split() -> anyhow::Result<()> {
    run(Scenario::NestedParallelSplit).await
}

#[path = "nested_retry.rs"]
mod nested_retry;

#[path = "loop_boundaries.rs"]
mod loop_boundaries;

#[tokio::test]
async fn emitted_while_body_cancel_bypasses_recovery() -> anyhow::Result<()> {
    run(Scenario::WhileBody).await
}
#[tokio::test]
async fn emitted_while_boundary_cancel_cleans_pending_sibling_before_ack() -> anyhow::Result<()> {
    run(Scenario::WhileParallel).await
}
#[tokio::test]
async fn emitted_embed_cancel_unwinds_two_inline_scopes() -> anyhow::Result<()> {
    run(Scenario::EmbedBody).await
}
#[tokio::test]
async fn emitted_embed_cancel_cleans_partial_http_body() -> anyhow::Result<()> {
    run(Scenario::EmbedPartialBody).await
}
#[tokio::test]
async fn emitted_embed_cancel_unwinds_while_body_without_retry() -> anyhow::Result<()> {
    run(Scenario::EmbedWhileBody).await
}
#[tokio::test]
async fn emitted_embed_cancel_cleans_parallel_child_branches() -> anyhow::Result<()> {
    run(Scenario::EmbedParallel).await
}

#[path = "composite_retry.rs"]
mod composite_retry;

mod pure_retry;
