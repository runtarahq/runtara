use redis::{Client, RedisError, aio::ConnectionManager};

use super::ValkeyConfig;

/// Valkey client wrapper with connection management
pub struct ValkeyClient {
    config: ValkeyConfig,
    manager: ConnectionManager,
}

impl ValkeyClient {
    /// Create a new Valkey client and establish connection
    pub async fn new(config: ValkeyConfig) -> Result<Self, RedisError> {
        let url = config.connection_url();

        println!("Connecting to Valkey at {}:{}", config.host, config.port);

        let client = Client::open(url)?;
        let manager = ConnectionManager::new(client).await?;

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
