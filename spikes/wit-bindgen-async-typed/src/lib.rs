// Sync impl of an async-TYPED export: legal per Concurrency.md (an async func
// may be lifted with the sync ABI; the callee blocks holding its instance
// lock). The question is whether wit-bindgen 0.58 can express it.
mod bindings {
    wit_bindgen::generate!({
        path: "wit",
        world: "probe",
        // Force the sync ABI even for async-typed functions (sync lift).
        async: false,
    });
}

struct Component;

impl bindings::exports::demo::probe::work::Guest for Component {
    fn run(ms: u64) -> u64 {
        ms
    }
}

bindings::export!(Component with_types_in bindings);
