use redis::{RedisError, aio::ConnectionManager};

use super::{ValkeyConfig, dedicated_manager_for_blocking_consumer};

/// Valkey client wrapper for the trigger-stream consumer.
///
/// The trigger worker issues `XREADGROUP ... BLOCK`, which parks the
/// underlying connection — so this type intentionally builds its own
/// dedicated `ConnectionManager` via
/// [`dedicated_manager_for_blocking_consumer`] rather than cloning the
/// process-wide `SHARED_MANAGER`. See the rule on
/// [`super::SHARED_MANAGER`].
pub struct ValkeyClient {
    config: ValkeyConfig,
    manager: ConnectionManager,
}

impl ValkeyClient {
    /// Create a new Valkey client and establish a dedicated connection
    /// suitable for blocking commands (`XREADGROUP ... BLOCK`).
    pub async fn new(config: ValkeyConfig) -> Result<Self, RedisError> {
        let url = config.connection_url();

        println!("Connecting to Valkey at {}:{}", config.host, config.port);

        let manager =
            dedicated_manager_for_blocking_consumer(&url, "valkey-trigger-stream").await?;

        println!("✓ Valkey connected successfully");

        Ok(ValkeyClient { config, manager })
    }

    /// Get a connection from the connection manager
    pub fn get_connection(&self) -> ConnectionManager {
        self.manager.clone()
    }

    /// Get the configuration
    pub fn config(&self) -> &ValkeyConfig {
        &self.config
    }

    /// Test the connection with a PING command
    pub async fn ping(&mut self) -> Result<(), RedisError> {
        let _: String = redis::cmd("PING").query_async(&mut self.manager).await?;
        Ok(())
    }
}
