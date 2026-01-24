// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Compensation framework for saga pattern support.
//!
//! Provides tooling for managing distributed transaction rollback when
//! downstream steps fail. Works with the existing checkpoints table
//! (extended with compensation fields) to track and execute rollback actions.

use crate::error::{Result, StructuredError};
use crate::persistence::{CheckpointRecord, Persistence};
use std::sync::Arc;
use tracing::{debug, info};

/// Compensation state for a checkpoint.
///
/// Tracks the lifecycle of compensation for a single checkpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompensationState {
    /// No compensation defined for this checkpoint.
    None,
    /// Compensation may be needed (step completed successfully).
    Pending,
    /// Compensation is currently in progress.
    Triggered,
    /// Compensation completed successfully.
    Completed,
    /// Compensation failed.
    Failed,
}

impl CompensationState {
    /// Returns the string representation of the state.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Pending => "pending",
            Self::Triggered => "triggered",
            Self::Completed => "completed",
            Self::Failed => "failed",
        }
    }

    /// Parse a state from a string.
    pub fn parse(s: &str) -> Self {
        match s {
            "pending" => Self::Pending,
            "triggered" => Self::Triggered,
            "completed" => Self::Completed,
            "failed" => Self::Failed,
            _ => Self::None,
        }
    }
}

/// Compensation manager - orchestrates saga rollback operations.
///
/// Uses the persistence layer to track and execute compensations.
/// Compensations are executed in reverse order of their registration
/// (last step is compensated first).
pub struct CompensationManager<P: Persistence + ?Sized> {
    persistence: Arc<P>,
}

impl<P: Persistence + ?Sized> CompensationManager<P> {
    /// Create a new compensation manager.
    pub fn new(persistence: Arc<P>) -> Self {
        Self { persistence }
    }

    /// Get all compensatable checkpoints for an instance in reverse execution order.
    ///
    /// Returns checkpoints ordered by compensation_order DESC (highest first).
    pub async fn get_pending_compensations(
        &self,
        instance_id: &str,
    ) -> Result<Vec<CheckpointRecord>> {
        self.persistence
            .get_compensatable_checkpoints(instance_id)
            .await
    }

    /// Trigger compensation for an instance.
    ///
    /// Marks all pending compensations as triggered and returns them
    /// in the order they should be executed (reverse of execution order).
    pub async fn trigger_compensation(
        &self,
        instance_id: &str,
        reason: &str,
    ) -> Result<Vec<CheckpointRecord>> {
        info!(instance_id, reason, "Triggering compensation");

        // Get checkpoints in reverse compensation_order
        let checkpoints = self.get_pending_compensations(instance_id).await?;

        if checkpoints.is_empty() {
            debug!(instance_id, "No pending compensations to trigger");
            return Ok(vec![]);
        }

        // Mark instance as compensating
        self.persistence
            .set_instance_compensation_state(instance_id, "triggered", Some(reason))
            .await?;

        // Mark each checkpoint as triggered
        for cp in &checkpoints {
            self.persistence
                .set_checkpoint_compensation_state(
                    instance_id,
                    &cp.checkpoint_id,
                    CompensationState::Triggered.as_str(),
                )
                .await?;
        }

        info!(
            instance_id,
            count = checkpoints.len(),
            "Compensation triggered for checkpoints"
        );

        Ok(checkpoints)
    }

    /// Mark a single checkpoint's compensation as complete.
    ///
    /// Returns true if all compensations are now done (either success or failure).
    pub async fn complete_checkpoint_compensation(
        &self,
        instance_id: &str,
        checkpoint_id: &str,
        success: bool,
        error: Option<&StructuredError>,
    ) -> Result<bool> {
        let state = if success {
            CompensationState::Completed
        } else {
            CompensationState::Failed
        };

        debug!(
            instance_id,
            checkpoint_id,
            success,
            state = state.as_str(),
            "Completing checkpoint compensation"
        );

        // Update checkpoint state
        self.persistence
            .set_checkpoint_compensation_state(instance_id, checkpoint_id, state.as_str())
            .await?;

        // Log the compensation attempt
        let compensation_step_id = "unknown"; // Would need to fetch from checkpoint
        let error_message = error.map(|e| e.message.as_str());

        self.persistence
            .log_compensation_attempt(
                instance_id,
                checkpoint_id,
                compensation_step_id,
                success,
                error_message,
                None, // error_id would require recording the error first
            )
            .await?;

        // Check if all compensations are done
        let remaining = self
            .persistence
            .count_pending_compensations(instance_id)
            .await?;

        if remaining == 0 {
            // All done - update instance state
            let all_succeeded = self
                .persistence
                .all_compensations_succeeded(instance_id)
                .await?;

            let final_state = if all_succeeded { "completed" } else { "failed" };

            self.persistence
                .set_instance_compensation_state(instance_id, final_state, None)
                .await?;

            info!(instance_id, all_succeeded, "All compensations completed");

            return Ok(all_succeeded);
        }

        debug!(instance_id, remaining, "Compensations remaining");
        Ok(true) // More compensations pending
    }

    /// Get the current compensation status for an instance.
    pub async fn get_compensation_status(&self, instance_id: &str) -> Result<CompensationStatus> {
        let pending = self.get_pending_compensations(instance_id).await?;

        // Count states
        let mut total = 0;
        let mut triggered = 0;
        let mut completed = 0;
        let mut failed = 0;

        for cp in &pending {
            if let Some(state) = &cp.compensation_state {
                total += 1;
                match CompensationState::parse(state) {
                    CompensationState::Triggered => triggered += 1,
                    CompensationState::Completed => completed += 1,
                    CompensationState::Failed => failed += 1,
                    _ => {}
                }
            }
        }

        let overall_state = if total == 0 {
            CompensationState::None
        } else if completed == total {
            CompensationState::Completed
        } else if failed > 0 {
            CompensationState::Failed
        } else if triggered > 0 {
            CompensationState::Triggered
        } else {
            CompensationState::Pending
        };

        Ok(CompensationStatus {
            state: overall_state,
            total_steps: total,
            completed_steps: completed,
            failed_steps: failed,
            pending_checkpoints: pending,
        })
    }
}

/// Summary of compensation status for an instance.
#[derive(Debug, Clone)]
pub struct CompensationStatus {
    /// Overall compensation state.
    pub state: CompensationState,
    /// Total number of compensatable steps.
    pub total_steps: usize,
    /// Number of completed compensations.
    pub completed_steps: usize,
    /// Number of failed compensations.
    pub failed_steps: usize,
    /// List of checkpoints still needing compensation.
    pub pending_checkpoints: Vec<CheckpointRecord>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compensation_state_roundtrip() {
        for state in [
            CompensationState::None,
            CompensationState::Pending,
            CompensationState::Triggered,
            CompensationState::Completed,
            CompensationState::Failed,
        ] {
            let s = state.as_str();
            let parsed = CompensationState::parse(s);
            assert_eq!(state, parsed);
        }
    }

    #[test]
    fn test_compensation_state_unknown_defaults_to_none() {
        assert_eq!(CompensationState::parse("invalid"), CompensationState::None);
        assert_eq!(CompensationState::parse(""), CompensationState::None);
    }
}
