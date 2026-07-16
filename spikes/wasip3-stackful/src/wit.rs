//! WIT contracts for the spike components.
//!
//! Every function that may block is `async func` — under the ratified
//! component-model-async rules (wasm-tools >= 1.249, enforced by wasmtime 46)
//! blocking legality is keyed to the function TYPE, and async canon options
//! are only legal on async-typed functions.
//!
//! Layout note: wit-parser 0.247 requires a top-level `package …;` header in
//! every file group (a file of only nested packages is rejected), so each
//! component's text is: its own package header + world at top level, with
//! dependency packages as nested `package … { }` blocks.

/// Plugin A: sync-LIFTED (but async-TYPED) export; sync-LOWERED async host
/// sleep. This is exactly the shape a sync-implemented agent will have under
/// ABI v2: it may block (type is async), holding only its own instance lock.
pub const PLUGIN_A_WIT: &str = r#"
package demo:plugin-a@0.1.0;

package demo:host@0.1.0 {
    interface env {
        sleep: async func(ms: u64);
    }
}
package demo:plugins@0.1.0 {
    interface alpha {
        run: async func(ms: u64) -> u64;
    }
}

world plugin-a {
    import demo:host/env@0.1.0;
    export demo:plugins/alpha@0.1.0;
}
"#;

pub const PLUGIN_B_WIT: &str = r#"
package demo:plugin-b@0.1.0;

package demo:host@0.1.0 {
    interface env {
        sleep: async func(ms: u64);
    }
}
package demo:plugins@0.1.0 {
    interface beta {
        run: async func(ms: u64) -> u64;
    }
}

world plugin-b {
    import demo:host/env@0.1.0;
    export demo:plugins/beta@0.1.0;
}
"#;

/// Orchestrator: stackful-async-lifted exports, async-lowered plugin imports.
pub const ORCHESTRATOR_WIT: &str = r#"
package demo:orchestrator@0.1.0;

package demo:plugins@0.1.0 {
    interface alpha {
        run: async func(ms: u64) -> u64;
    }
    interface beta {
        run: async func(ms: u64) -> u64;
    }
}
package demo:app@0.1.0 {
    interface runner {
        run-both: async func(ms: u64) -> u64;
        run-seq: async func(ms: u64) -> u64;
    }
}

world orchestrator {
    import demo:plugins/alpha@0.1.0;
    import demo:plugins/beta@0.1.0;
    export demo:app/runner@0.1.0;
}
"#;

/// Composition script — same textual wac pipeline production uses
/// (wac_parser::Document + FileSystemPackageResolver + wac-graph). The
/// trailing `...` lets `demo:host/env` bubble up to the composed component's
/// imports, where the wasmtime host satisfies it.
pub const COMPOSE_WAC: &str = r#"// Spike S1 composition.
package demo:composed;

let plugin-a = new demo:plugin-a { ... };
let plugin-b = new demo:plugin-b { ... };
let orch = new demo:orchestrator { ...plugin-a, ...plugin-b, ... };

export orch...;
"#;
