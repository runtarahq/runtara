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

~900 KB raw, ~320 KB gzipped. Above the Phase 2 plan target of <250 KB.
Further slimming requires cfg-gating `runtara-dsl::step_registration` and
the `SchemaGeneratorFn` family in `agent_meta` so the WASM tree can
drop schemars 0.8 entirely. Tracked in
`docs/reports-refactoring-plan.md` as Phase 2 follow-up; mitigation is
lazy-loading on the report-builder route.
