pub mod local;
pub mod oidc;
pub mod trust_proxy;

pub use local::LocalProvider;
pub use oidc::OidcProvider;
pub use trust_proxy::TrustProxyProvider;
