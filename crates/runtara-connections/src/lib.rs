pub mod auth;
pub mod config;
pub mod crypto;
pub mod error;
pub mod events;
pub mod facade;
pub mod handler;
pub mod integration_compatibility;
pub mod repository;
pub mod router;
pub mod service;
pub mod tenant;
pub mod types;
pub mod util;

pub use auth::aws_signing::AwsSigningParams;
pub use auth::azure_signing::AzureSigningParams;
pub use auth::provider_auth::{ResolvedConnectionAuth, resolve_connection_auth};
pub use config::{ConnectionsConfig, ConnectionsState};
pub use crypto::{
    CipherError, CredentialCipher, ENCRYPTION_KEY_ENV, ENVELOPE_ALG, ENVELOPE_VERSION,
    cipher_from_env,
};
pub use error::ConnectionsError;
pub use events::{ConnectionEventSink, ConnectionEvents, ConnectionLifecycleEvent};
pub use facade::ConnectionsFacade;
pub use integration_compatibility::{IntegrationCompatibility, OBJECT_STORAGE_DEFAULT_FOR};
pub use repository::connections::{ConnectionWithParameters, ReencryptionStats};
pub use router::{admin_router, connections_router, oauth_callback_router, runtime_router};
pub use tenant::TenantId;
pub use types::{
    ConnectionDto, ConnectionStatus, CreateConnectionRequest, RateLimitConfigDto,
    RateLimitEventType, RuntimeConnectionResponse, UpdateConnectionRequest,
};
