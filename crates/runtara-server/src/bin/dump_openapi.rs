//! Dump the runtime OpenAPI doc to stdout.
//!
//! The release workflow attaches its output to each release as `openapi.json`,
//! which runtara-frontend generates its API client from
//! (`npm run generate-api-runtime-release`).
//!
//! Usage:
//!   cargo run --bin dump_openapi -p runtara-server > openapi.json

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
