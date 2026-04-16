pub mod auth;
pub mod config;
pub mod error;
pub mod facade;
pub mod handler;
pub mod repository;
pub mod router;
pub mod service;
pub mod tenant;
pub mod types;
pub mod util;

pub use auth::aws_signing::AwsSigningParams;
pub use auth::provider_auth::{ResolvedConnectionAuth, resolve_connection_auth};
pub use config::{ConnectionsConfig, ConnectionsState};
pub use error::ConnectionsError;
pub use facade::ConnectionsFacade;
pub use repository::connections::ConnectionWithParameters;
pub use router::{connections_router, oauth_callback_router, runtime_router};
pub use tenant::TenantId;
pub use types::{
    ConnectionDto, ConnectionStatus, CreateConnectionRequest, RateLimitConfigDto,
    RateLimitEventType, RuntimeConnectionResponse, UpdateConnectionRequest,
};
