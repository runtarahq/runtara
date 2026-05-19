//! Wasmtime-generated host bindings for the `runtara:agent` world.
//!
//! WASI imports are remapped to wasmtime-wasi's pre-generated bindings so the
//! host doesn't have to re-implement them — `Linker::add_to_linker_async`
//! from `wasmtime_wasi` and `wasmtime_wasi_http` satisfy them at link time.

wasmtime::component::bindgen!({
    path: "../runtara-agent-wit/wit",
    world: "agent",
    imports: { default: async | trappable },
    exports: { default: async },
});

#[cfg(test)]
mod tests {
    // Touch the generated symbols so an API rename or path shift fails to
    // compile here rather than silently downstream.
    #[allow(unused_imports)]
    use super::Agent;
    #[allow(unused_imports)]
    use super::exports::runtara::agent::capabilities::{ConnectionInfo, ErrorInfo};
}
