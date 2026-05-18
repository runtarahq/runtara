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
    let doc = ApiDoc::openapi();
    let json = serde_json::to_string_pretty(&doc).expect("serialize OpenAPI doc");
    println!("{json}");
}
