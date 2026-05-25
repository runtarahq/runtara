//! MCP (Model Context Protocol) server for Runtara Runtime.
//!
//! Exposes workflow management, execution monitoring, object model,
//! and agent discovery capabilities via Streamable HTTP transport.

pub mod entitlement;
pub mod server;
mod session_store;
pub mod tools;

pub use server::create_mcp_router;
pub use session_store::ValkeyMcpSessionStore;
