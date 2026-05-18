# runtara-report-dsl WASM bundle

Vendored output of `wasm-pack build --target web --out-dir pkg --features wasm --no-default-features`
on the `runtara-report-dsl` crate. Imported by `src/features/reports/utils.ts`
to share one minijinja template engine + one `ConditionExpression`
evaluator between the server and the report viewer/builder.

## Regenerating

```sh
cd ../../../../runtara-report-dsl
wasm-pack build --target web --out-dir pkg --features wasm --no-default-features
cp pkg/runtara_report_dsl_bg.wasm pkg/runtara_report_dsl.js \
   pkg/runtara_report_dsl.d.ts   pkg/runtara_report_dsl_bg.wasm.d.ts \
   ../runtara-server/frontend/src/wasm/runtara-report-dsl/
```

## Size

~950 KB raw, ~339 KB gzipped. Above the Phase 2 plan target of <250 KB
but acceptable since the report-builder route is lazy-loaded
(`ReportPage` defers `ReportBuilderWizard` import until edit mode).

The runtara-dsl `spec` and `step_registration` modules plus the
`SchemaGeneratorFn` family in `agent_meta` are now `#[cfg(feature =
"json-schema")]` and this crate consumes runtara-dsl with
`default-features = false`, so schemars + the schema-generation surface
stay out of the WASM tree. Remaining size comes from minijinja
(~150 KB) and the canonical condition / format machinery.
