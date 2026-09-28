//! Control receipts share the store lock with every lifecycle write.
use super::*;
use crate::persistence::control_receipts::*;

fn missing(caller: &str, operation: &str) -> CoreError {
    CoreError::PersistenceError {
        operation: "complete_control_receipt".into(),
        details: format!("no control receipt for {caller}/{operation}"),
    }
}

#[async_trait]
impl ControlReceipts for InMemoryPersistence {
    async fn receipt_by_operation(
        &self,
        caller: &str,
        operation: &str,
    ) -> Result<Option<ControlReceipt>, CoreError> {
        validate_receipt_key(caller, operation)?;
        let store = self.store.lock().unwrap();
        Ok(store
            .control_receipts
            .get(&(caller.to_owned(), operation.to_owned()))
            .cloned())
    }

    async fn begin(
        &self,
        caller: &str,
        operation: &str,
        intent: &ControlIntent,
    ) -> Result<BeginReceipt, CoreError> {
        validate_receipt_key(caller, operation)?;
        let mut store = self.store.lock().unwrap();
        store.instance_mut(caller)?;
        let key = (caller.to_owned(), operation.to_owned());
        if let Some(existing) = store.control_receipts.get(&key) {
            return Ok(BeginReceipt::Existing(existing.clone()));
        }
        let receipt = ControlReceipt {
            caller_instance_id: caller.to_owned(),
            operation_id: operation.to_owned(),
            intent: intent.clone(),
            state: ControlReceiptState::Pending,
            result: None,
            created_at: Utc::now(),
            completed_at: None,
        };
        store.control_receipts.insert(key, receipt.clone());
        Ok(BeginReceipt::Started(receipt))
    }

    async fn complete(
        &self,
        caller: &str,
        operation: &str,
        result: &serde_json::Value,
    ) -> Result<ControlReceipt, CoreError> {
        validate_receipt_key(caller, operation)?;
        let mut store = self.store.lock().unwrap();
        let receipt = store
            .control_receipts
            .get_mut(&(caller.to_owned(), operation.to_owned()))
            .ok_or_else(|| missing(caller, operation))?;
        if receipt.state == ControlReceiptState::Pending {
            receipt.state = ControlReceiptState::Completed;
            receipt.result = Some(result.clone());
            receipt.completed_at = Some(Utc::now());
        }
        Ok(receipt.clone())
    }

    async fn discard(&self, caller: &str, operation: &str) -> Result<bool, CoreError> {
        validate_receipt_key(caller, operation)?;
        let mut store = self.store.lock().unwrap();
        let key = (caller.to_owned(), operation.to_owned());
        if store
            .control_receipts
            .get(&key)
            .is_some_and(|receipt| receipt.state == ControlReceiptState::Pending)
        {
            store.control_receipts.remove(&key);
            return Ok(true);
        }
        Ok(false)
    }
}
