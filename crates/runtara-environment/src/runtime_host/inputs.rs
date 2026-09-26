//! Managed-input operations bound to trusted root or child execution authority.
use super::*;
use runtara_component_host::runtime_host::RuntimeInputState;
use runtara_core::persistence::inputs::{
    InputAuthority, InputClosure, InputRequestSpec, InputRequests, InputState, request_id,
};
use runtara_core::persistence::invocations::InvocationLease;

pub(super) fn state(value: InputState) -> RuntimeInputState {
    match value {
        InputState::Open => RuntimeInputState::Open,
        InputState::Accepted { receipt } => RuntimeInputState::Accepted(receipt.payload),
        InputState::Closed { reason, .. } => RuntimeInputState::Closed(reason.as_str().into()),
    }
}

impl PersistenceRuntimeHost {
    /// Bind the root's input IO to the same lease as isolated invocation IO.
    /// A running host cannot be rebound to a different execution owner.
    pub fn bind_input_lease(&self, lease: InvocationLease) -> Result<(), String> {
        if lease.instance_id != self.instance_id {
            return Err("input lease belongs to another instance".into());
        }
        match self.input_lease.set(lease) {
            Ok(()) => Ok(()),
            Err(lease) if self.input_lease.get() == Some(&lease) => Ok(()),
            Err(_) => Err("runtime input lease is already bound".into()),
        }
    }

    pub(super) fn inputs(&self) -> Result<&dyn InputRequests, String> {
        self.state
            .persistence
            .input_requests()
            .ok_or_else(|| "persistence does not support managed inputs".into())
    }

    /// Rebase a guest deadline (minted from this host's `now-ms`) onto the
    /// persistence clock that decides expiry.
    pub(super) async fn persistence_deadline(
        &self,
        deadline: Option<u64>,
    ) -> Result<Option<u64>, String> {
        let host_now = runtara_component_host::runtime_host::RuntimeHost::now_ms(self)?;
        runtara_core::persistence::inputs::persistence_deadline_ms(
            self.inputs()?,
            deadline,
            host_now,
        )
        .await
        .map_err(Self::err)
    }

    async fn input_authority(&self) -> Result<InputAuthority, String> {
        if let Some(lease) = self.input_lease.get() {
            return Ok(InputAuthority::LeasedRoot(lease.clone()));
        }
        let root = self
            .state
            .persistence
            .get_instance_meta(&self.instance_id)
            .await
            .map_err(Self::err)?
            .ok_or_else(|| "input instance not found".to_owned())?;
        Ok(InputAuthority::Root {
            tenant_id: root.tenant_id,
            instance_id: self.instance_id.clone(),
        })
    }

    pub(super) async fn register_managed_input(
        &self,
        descriptor: Vec<u8>,
        deadline: Option<u64>,
    ) -> Result<(), String> {
        let deadline = self.persistence_deadline(deadline).await?;
        let spec = InputRequestSpec::from_descriptor(&descriptor, deadline).map_err(Self::err)?;
        let owner = self.input_authority().await?;
        self.inputs()?
            .register_input(&owner, &spec)
            .await
            .map_err(Self::err)?;
        Ok(())
    }

    pub(super) async fn poll_managed_input(
        &self,
        signal: String,
    ) -> Result<RuntimeInputState, String> {
        let owner = self.input_authority().await?;
        let request = self
            .inputs()?
            .poll_input(&owner, &request_id(&signal))
            .await
            .map_err(Self::err)?;
        if request.spec.signal_id != signal {
            return Err("input signal identity mismatch".into());
        }
        Ok(state(request.state))
    }

    pub(super) async fn close_managed_input(
        &self,
        signal: String,
    ) -> Result<RuntimeInputState, String> {
        let owner = self.input_authority().await?;
        let request = self
            .inputs()?
            .close_input(&owner, &request_id(&signal), InputClosure::Abandoned)
            .await
            .map_err(Self::err)?;
        if request.spec.signal_id != signal {
            return Err("input signal identity mismatch".into());
        }
        Ok(state(request.state))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtara_core::domain::InstanceStatus;
    use runtara_core::persistence::inputs::submit_input;
    use runtara_core::persistence::memory::InMemoryPersistence;
    use serde_json::json;

    async fn fixture() -> (Arc<InMemoryPersistence>, PersistenceRuntimeHost) {
        let p = Arc::new(InMemoryPersistence::new());
        p.register_instance("input-root", "input-tenant")
            .await
            .unwrap();
        p.update_instance_status("input-root", InstanceStatus::Running, None)
            .await
            .unwrap();
        let host = PersistenceRuntimeHost::from_persistence(p.clone(), "input-root".into(), false);
        (p, host)
    }

    fn descriptor() -> Vec<u8> {
        serde_json::to_vec(&json!({
            "signal_id": "wait-one",
            "step_id": "ask",
            "response_schema": { "answer": { "type": "string", "required": true } }
        }))
        .unwrap()
    }

    #[tokio::test]
    async fn managed_input_survives_without_debug_events() {
        let (p, host) = fixture().await;
        host.register_input(descriptor(), None).await.unwrap();
        assert_eq!(
            host.poll_input("wait-one".into()).await.unwrap(),
            RuntimeInputState::Open
        );
        assert!(
            submit_input(
                p.input_requests().unwrap(),
                "input-tenant",
                "input-root",
                &request_id("wait-one"),
                "reply",
                &json!({"answer":false})
            )
            .await
            .is_err()
        );
        let receipt = submit_input(
            p.input_requests().unwrap(),
            "input-tenant",
            "input-root",
            &request_id("wait-one"),
            "reply",
            &json!({"answer":"yes"}),
        )
        .await
        .unwrap();
        host.register_input(descriptor(), None).await.unwrap();
        assert_eq!(
            host.poll_input("wait-one".into()).await.unwrap(),
            RuntimeInputState::Accepted(receipt.payload.clone())
        );
        assert_eq!(
            host.close_input("wait-one".into()).await.unwrap(),
            RuntimeInputState::Accepted(receipt.payload.clone())
        );
        let replayed = PersistenceRuntimeHost::from_persistence(p, "input-root".into(), false);
        assert_eq!(
            replayed.poll_input("wait-one".into()).await.unwrap(),
            RuntimeInputState::Accepted(receipt.payload)
        );
    }

    #[tokio::test]
    async fn persistence_deadline_closes_wait_while_root_continues() {
        let (p, host) = fixture().await;
        host.register_input(descriptor(), Some(1)).await.unwrap();
        assert_eq!(
            host.poll_input("wait-one".into()).await.unwrap(),
            RuntimeInputState::Closed("expired".into())
        );
        assert_eq!(
            p.get_instance("input-root").await.unwrap().unwrap().status,
            InstanceStatus::Running
        );
        assert!(
            submit_input(
                p.input_requests().unwrap(),
                "input-tenant",
                "input-root",
                &request_id("wait-one"),
                "late",
                &json!({"answer":"yes"})
            )
            .await
            .is_err()
        );
    }
}
