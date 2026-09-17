//! Validate execution entry types from prepared metadata, before any Store or
//! initializer exists. Keep field/case ordering aligned with the canonical WIT
//! and host mirrors in `lifecycle`; Wasmtime still type-checks the actual call.
use anyhow::{Result, ensure};
use wasmtime::component::types::{ComponentFunc, Type};

type Check = fn(&Type) -> bool;

pub(super) fn validate(invoke: &ComponentFunc, interface: &str) -> Result<()> {
    let lifecycle = matches!(
        interface,
        runtara_workflow_wit::LIFECYCLE_INTERFACE_NAME
            | runtara_workflow_wit::LIFECYCLE_INTERFACE_NAME_V1
    );
    let mut params = invoke.params();
    if !lifecycle {
        ensure!(
            params.next().is_some_and(|(_, ty)| string(&ty)),
            "isolated capability invoke requires a string capability argument"
        );
    }
    ensure!(
        params.next().is_some_and(|(_, ty)| bytes(&ty)) && params.next().is_none(),
        "isolated invoke requires exactly one input byte-list argument after its capability, if any"
    );
    let mut results = invoke.results();
    let Some(Type::Result(result)) = results.next() else {
        anyhow::bail!("isolated invoke must return a result");
    };
    ensure!(
        results.next().is_none(),
        "isolated invoke must return exactly one result"
    );
    let success: Check = if lifecycle { outcome } else { bytes };
    ensure!(
        result.ok().as_ref().is_some_and(success),
        "incompatible isolated invoke success ABI"
    );
    ensure!(
        result.err().as_ref().is_some_and(error_info),
        "incompatible isolated invoke error ABI"
    );
    // Both sync and async component functions are supported by call_async.
    // In particular the lifecycle 0.1.0 binding remains sync-typed.
    Ok(())
}

fn string(ty: &Type) -> bool {
    *ty == Type::String
}
fn boolean(ty: &Type) -> bool {
    *ty == Type::Bool
}
fn u64_(ty: &Type) -> bool {
    *ty == Type::U64
}
fn bytes(ty: &Type) -> bool {
    matches!(ty, Type::List(list) if list.ty() == Type::U8)
}
fn optional_u64(ty: &Type) -> bool {
    matches!(ty, Type::Option(option) if u64_(&option.ty()))
}
fn optional_string(ty: &Type) -> bool {
    matches!(ty, Type::Option(option) if string(&option.ty()))
}

fn fields(ty: &Type, expected: &[(&str, Check)]) -> bool {
    let Type::Record(record) = ty else {
        return false;
    };
    record.fields().len() == expected.len()
        && record
            .fields()
            .zip(expected)
            .all(|(field, (name, check))| field.name == *name && check(&field.ty))
}
fn cases(ty: &Type, expected: &[(&str, Option<Check>)]) -> bool {
    let Type::Variant(variant) = ty else {
        return false;
    };
    variant.cases().len() == expected.len()
        && variant.cases().zip(expected).all(|(case, (name, check))| {
            case.name == *name
                && match (&case.ty, check) {
                    (Some(ty), Some(check)) => check(ty),
                    (None, None) => true,
                    _ => false,
                }
        })
}
fn error_info(ty: &Type) -> bool {
    fields(
        ty,
        &[
            ("code", string),
            ("message", string),
            ("category", string),
            ("severity", string),
            ("retryable", boolean),
            ("retry-after-ms", optional_u64),
            ("attributes", optional_string),
        ],
    )
}
fn signal_wait(ty: &Type) -> bool {
    fields(
        ty,
        &[("checkpoint-id", string), ("deadline-ms", optional_u64)],
    )
}
fn wake(ty: &Type) -> bool {
    cases(
        ty,
        &[
            ("at", Some(u64_)),
            ("on-signal", Some(signal_wait)),
            ("on-resume", None),
        ],
    )
}
fn wakes(ty: &Type) -> bool {
    matches!(ty, Type::List(list) if wake(&list.ty()))
}
fn outcome(ty: &Type) -> bool {
    cases(
        ty,
        &[("completed", Some(bytes)), ("suspended", Some(wakes))],
    )
}
