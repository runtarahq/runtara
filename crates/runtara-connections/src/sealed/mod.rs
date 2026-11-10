//! Sealed credentials: secrets held by seal, each with the rules for using it.
//!
//! seal runs in this process (library mode). The server can still open every
//! credential; what changes is that a secret is stored with the one place it
//! may go, and seal attaches it and sends the request.

pub mod keys;
pub mod policy;
