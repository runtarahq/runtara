//! Run state shares the store lock with every lifecycle write, which is what
//! makes the running fence, the write log and the merge one atomic step.
use super::*;
use crate::persistence::run_state::*;

impl Store {
    fn tenant_instance(
        &self,
        tenant_id: &str,
        instance_id: &str,
    ) -> Result<&InstanceRecord, CoreError> {
        self.instances
            .get(instance_id)
            .filter(|instance| instance.tenant_id == tenant_id)
            .ok_or_else(|| CoreError::InstanceNotFound {
                instance_id: instance_id.to_string(),
            })
    }
}

#[async_trait]
impl RunState for InMemoryPersistence {
    async fn apply_state(
        &self,
        tenant_id: &str,
        instance_id: &str,
        operation_id: &str,
        patch: &StatePatch,
    ) -> Result<StateWrite, CoreError> {
        validate_state_write(tenant_id, instance_id, operation_id)?;
        let mut store = self.store.lock().unwrap();
        let status = store.tenant_instance(tenant_id, instance_id)?.status;
        let key = (instance_id.to_owned(), operation_id.to_owned());
        if store.run_state_writes.contains(&key) {
            return Ok(StateWrite::Replayed);
        }
        if status != CoreInstanceStatus::Running {
            return Err(crate::persistence::continuations::not_running(
                instance_id,
                status,
            ));
        }
        let mut state = store
            .run_state
            .get(instance_id)
            .map(|record| record.state.clone())
            .unwrap_or_default();
        patch.apply_to(&mut state);
        if state_size(&state) > MAX_STATE_BYTES {
            return Err(state_too_large());
        }
        store.run_state.insert(
            instance_id.to_owned(),
            RunStateRecord {
                state,
                updated_at: Utc::now(),
            },
        );
        store.run_state_writes.insert(key);
        Ok(StateWrite::Applied)
    }

    async fn get_state(
        &self,
        tenant_id: &str,
        instance_id: &str,
    ) -> Result<Option<RunStateRecord>, CoreError> {
        let store = self.store.lock().unwrap();
        store.tenant_instance(tenant_id, instance_id)?;
        Ok(store.run_state.get(instance_id).cloned())
    }
}
