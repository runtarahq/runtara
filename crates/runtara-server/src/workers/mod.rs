//! Background workers for scenario execution

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

pub mod compilation_worker;
pub mod cron_scheduler;
pub mod execution_engine;
pub mod runtara_dto;
pub mod trigger_worker;

/// Cancellation handle for running executions
pub struct CancellationHandle {
    pub task_handle: tokio::task::JoinHandle<()>,
    pub cancel_flag: Arc<AtomicBool>,
}
