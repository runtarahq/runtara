//! Agent continuations share the store lock with every lifecycle write, which
//! is what makes the running fence atomic with the write.
use super::*;
use crate::persistence::continuations::*;

#[async_trait]
impl AgentContinuations for InMemoryPersistence {
    async fn get(
        &self,
        instance_id: &str,
        op_hash: &str,
        attempt: u32,
    ) -> Result<Option<Vec<u8>>, CoreError> {
        validate_continuation(instance_id, op_hash, attempt, None)?;
        let store = self.store.lock().unwrap();
        Ok(store
            .agent_continuations
            .get(&(instance_id.to_owned(), op_hash.to_owned()))
            .filter(|(stored, _)| *stored == attempt)
            .map(|(_, state)| state.clone()))
    }

    async fn put(
        &self,
        instance_id: &str,
        op_hash: &str,
        attempt: u32,
        state: &[u8],
        owner: crate::persistence::ExecutionWriter<'_>,
    ) -> Result<(), CoreError> {
        validate_continuation(instance_id, op_hash, attempt, Some(state))?;
        let mut store = self.store.lock().unwrap();
        let status = store.instance_mut(instance_id)?.status;
        if status != CoreInstanceStatus::Running {
            return Err(not_running(instance_id, status));
        }
        store.admit_writer(instance_id, owner)?;
        store.agent_continuations.insert(
            (instance_id.to_owned(), op_hash.to_owned()),
            (attempt, state.to_vec()),
        );
        Ok(())
    }

    async fn delete(&self, instance_id: &str, op_hash: &str) -> Result<bool, CoreError> {
        validate_continuation(instance_id, op_hash, 1, None)?;
        let mut store = self.store.lock().unwrap();
        Ok(store
            .agent_continuations
            .remove(&(instance_id.to_owned(), op_hash.to_owned()))
            .is_some())
    }
}
