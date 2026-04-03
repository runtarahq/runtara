//! MCP (Model Context Protocol) server for Runtara Runtime.
//!
//! Exposes scenario management, execution monitoring, object model,
//! and agent discovery capabilities via Streamable HTTP transport.

mod server;
mod tools;

pub use server::create_mcp_router;
