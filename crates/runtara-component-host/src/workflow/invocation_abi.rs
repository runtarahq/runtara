//! Validate execution entry types from prepared metadata, before any Store or
//! initializer exists. Keep field/case ordering aligned with the canonical WIT
//! and host mirrors in `lifecycle`; Wasmtime still type-checks the actual call.
use anyhow::{Result, ensure};
use wasmtime::component::types::{ComponentFunc, Type};

type Check = fn(&Type) -> bool;

/// Every entry, workflow or agent, is `invoke(capability-id, input) ->
/// result<outcome, error-info>`.
pub(super) fn validate(invoke: &ComponentFunc) -> Result<()> {
    let mut params = invoke.params();
    ensure!(
        params.next().is_some_and(|(_, ty)| string(&ty)),
        "isolated invoke requires a string capability argument"
    );
    ensure!(
        params.next().is_some_and(|(_, ty)| bytes(&ty)) && params.next().is_none(),
        "isolated invoke requires exactly one input byte-list argument after its capability"
    );
    let mut results = invoke.results();
    let Some(Type::Result(result)) = results.next() else {
        anyhow::bail!("isolated invoke must return a result");
    };
    ensure!(
        results.next().is_none(),
        "isolated invoke must return exactly one result"
    );
    ensure!(
        result.ok().as_ref().is_some_and(outcome),
        "incompatible isolated invoke success ABI"
    );
    ensure!(
        result.err().as_ref().is_some_and(error_info),
        "incompatible isolated invoke error ABI"
    );
    // Both sync and async component functions are supported by call_async.
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
            ("details", optional_string),
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
            ("instances", Some(string)),
        ],
    )
}
fn wakes(ty: &Type) -> bool {
    matches!(ty, Type::List(list) if wake(&list.ty()))
}
fn suspension(ty: &Type) -> bool {
    fields(ty, &[("wakes", wakes), ("state", bytes)])
}
fn outcome(ty: &Type) -> bool {
    cases(
        ty,
        &[("completed", Some(bytes)), ("suspended", Some(suspension))],
    )
}
