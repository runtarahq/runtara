//! Dump the runtime OpenAPI doc to stdout.
//!
//! Used by Phase 2 of the reports refactor to regenerate
//! `frontend/src/generated/RuntaraRuntimeApi.ts` without standing up a full
//! server (the usual `npm run generate-api-runtime-local` script needs the
//! HTTP endpoint).
//!
//! Usage:
//!   cargo run --bin dump_openapi -p runtara-server > openapi.json
//!   npx swagger-typescript-api generate --axios -p openapi.json \
//!     -o crates/runtara-server/frontend/src/generated -n RuntaraRuntimeApi.ts

use runtara_server::server::ApiDoc;
use utoipa::OpenApi;

fn main() {
    // utoipa's schema build recurses deeply through nested DSL types and
    // blows the default 8 MB main-thread stack on debug builds. Run it on
    // a worker thread with a much larger stack so the offline regen
    // pipeline works without a running server.
    let json = std::thread::Builder::new()
        .stack_size(512 * 1024 * 1024)
        .name("openapi-dump".into())
        .spawn(|| {
            let doc = ApiDoc::openapi();
            serde_json::to_string_pretty(&doc).expect("serialize OpenAPI doc")
        })
        .expect("spawn openapi-dump thread")
        .join()
        .expect("openapi-dump thread panicked");
    println!("{json}");
}
