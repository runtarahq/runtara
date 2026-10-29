//! Whose work this process claims.
//!
//! A process serving some tenants of a store that other processes share (a
//! shared service while another instance hands over to it, say) must not wake
//! or launch another tenant's runs: it has neither that tenant's credential
//! keys nor the fleet's routing for them. By default every tenant's work is
//! claimed, as by a process that has its store to itself.

static TENANTS: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();

/// Claim only `tenants`' wakes and launches from now on. Set once, at boot.
pub fn set_claim_tenants(tenants: Vec<String>) {
    let _ = TENANTS.set(tenants);
}

/// The tenants whose work this process claims; `None` for every tenant.
pub fn claim_tenants() -> Option<&'static [String]> {
    TENANTS.get().map(Vec::as_slice)
}
