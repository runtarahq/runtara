//! A Valkey container for tests that start OAuth flows, which are held there.

use testcontainers::core::{IntoContainerPort, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage};

pub struct ValkeyFixture {
    pub manager: redis::aio::ConnectionManager,
    _container: ContainerAsync<GenericImage>,
}

impl ValkeyFixture {
    pub async fn start() -> Self {
        let container = GenericImage::new("valkey/valkey", "8-alpine")
            .with_exposed_port(6379.tcp())
            .with_wait_for(WaitFor::message_on_stdout("Ready to accept connections"))
            .start()
            .await
            .expect("required Docker Valkey container must start");
        let host = container.get_host().await.expect("Valkey host");
        let port = container
            .get_host_port_ipv4(6379)
            .await
            .expect("Valkey port");
        let client = redis::Client::open(format!("redis://{host}:{port}")).expect("Valkey URL");
        let manager = redis::aio::ConnectionManager::new(client)
            .await
            .expect("Valkey connection");
        Self {
            manager,
            _container: container,
        }
    }
}
