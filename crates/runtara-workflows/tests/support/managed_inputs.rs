//! Real managed persistence behind the compiled workflow test host. Scripted
//! replies are submitted only after registration, never inserted as raw signals.
use std::collections::{BTreeSet, HashMap};
use std::sync::Mutex;

use runtara_component_host::runtime_host::RuntimeInputState;
use runtara_core::domain::InstanceStatus;
use runtara_core::persistence::{
    Persistence,
    inputs::{
        InputAuthority, InputClosure, InputRequest, InputRequestSpec, InputState,
        persistence_deadline_ms, request_id, submit_input,
    },
    memory::InMemoryPersistence,
};

const TENANT: &str = "compiled-input-test";

pub(super) struct ManagedInputs {
    instance: String,
    persistence: tokio::sync::OnceCell<InMemoryPersistence>,
    replies: Mutex<HashMap<String, serde_json::Value>>,
    registered: Mutex<BTreeSet<String>>,
}

impl Default for ManagedInputs {
    fn default() -> Self {
        Self::new("checkpoint-ns-e2e")
    }
}

impl ManagedInputs {
    pub(super) fn new(instance: impl Into<String>) -> Self {
        Self {
            instance: instance.into(),
            persistence: Default::default(),
            replies: Default::default(),
            registered: Default::default(),
        }
    }

    async fn persistence(&self) -> &InMemoryPersistence {
        self.persistence
            .get_or_init(|| async {
                let p = InMemoryPersistence::new();
                p.register_instance(&self.instance, TENANT).await.unwrap();
                p.update_instance_status(&self.instance, InstanceStatus::Running, None)
                    .await
                    .unwrap();
                p
            })
            .await
    }

    fn authority(&self) -> InputAuthority {
        InputAuthority::Root {
            tenant_id: TENANT.into(),
            instance_id: self.instance.clone(),
        }
    }

    pub(super) fn respond_when_registered(&self, signal: &str, payload: &[u8]) {
        let value = serde_json::from_slice(payload).expect("response JSON");
        assert!(
            self.replies
                .lock()
                .unwrap()
                .insert(signal.into(), value)
                .is_none()
        );
    }

    /// Register like a production host: a guest deadline minted against
    /// `host_now_ms` is rebased onto persistence time. `None` keeps it absolute.
    pub(super) async fn register(
        &self,
        descriptor: Vec<u8>,
        deadline: Option<u64>,
        host_now_ms: Option<u64>,
    ) -> Result<(), String> {
        let inputs = self.persistence().await.input_requests().unwrap();
        let deadline = match host_now_ms {
            Some(now) => persistence_deadline_ms(inputs, deadline, now)
                .await
                .map_err(|e| e.to_string())?,
            None => deadline,
        };
        let spec =
            InputRequestSpec::from_descriptor(&descriptor, deadline).map_err(|e| e.to_string())?;
        inputs
            .register_input(&self.authority(), &spec)
            .await
            .map_err(|e| e.to_string())?;
        self.registered.lock().unwrap().insert(spec.signal_id);
        Ok(())
    }

    pub(super) async fn poll(&self, signal: &str) -> Result<RuntimeInputState, String> {
        let inputs = self.persistence().await.input_requests().unwrap();
        let payload = self.replies.lock().unwrap().remove(signal);
        if let Some(payload) = payload {
            self.respond(signal, &payload).await?;
        }
        inputs
            .poll_input(&self.authority(), &request_id(signal))
            .await
            .map(|request| state(request.state))
            .map_err(|e| e.to_string())
    }

    pub(super) async fn respond(
        &self,
        signal: &str,
        payload: &serde_json::Value,
    ) -> Result<(), String> {
        let request = request_id(signal);
        submit_input(
            self.persistence().await.input_requests().unwrap(),
            TENANT,
            &self.instance,
            &request,
            &format!("reply-{request}"),
            payload,
        )
        .await
        .map(|_| ())
        .map_err(|e| e.to_string())
    }

    pub(super) async fn close(&self, signal: &str) -> Result<RuntimeInputState, String> {
        self.persistence()
            .await
            .input_requests()
            .unwrap()
            .close_input(
                &self.authority(),
                &request_id(signal),
                InputClosure::Abandoned,
            )
            .await
            .map(|request| state(request.state))
            .map_err(|e| e.to_string())
    }

    pub(super) async fn request(&self, signal: &str) -> InputRequest {
        self.persistence()
            .await
            .input_requests()
            .unwrap()
            .get_input(TENANT, &self.instance, &request_id(signal))
            .await
            .unwrap()
    }

    pub(super) async fn requests(&self) -> Vec<InputRequest> {
        let signals = self.registered.lock().unwrap().clone();
        let mut requests = Vec::new();
        for signal in signals {
            requests.push(self.request(&signal).await);
        }
        requests
    }
}

fn state(state: InputState) -> RuntimeInputState {
    match state {
        InputState::Open => RuntimeInputState::Open,
        InputState::Accepted { receipt } => RuntimeInputState::Accepted(receipt.payload),
        InputState::Closed { reason, .. } => RuntimeInputState::Closed(reason.as_str().into()),
    }
}

impl std::fmt::Debug for ManagedInputs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ManagedInputs")
            .field("instance", &self.instance)
            .finish_non_exhaustive()
    }
}
