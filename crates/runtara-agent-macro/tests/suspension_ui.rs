//! Suspension and control-executor declarations are checked at compile time:
//! the `suspending` list must match `#[capability(suspends = true)]`, and a
//! suspending capability must take `&SuspendContext` and return
//! `Result<Suspendable<O>, E>`.

#[test]
fn suspension_declarations_are_checked_at_compile_time() {
    let cases = trybuild::TestCases::new();
    cases.pass("tests/ui/pass/*.rs");
    cases.compile_fail("tests/ui/fail/*.rs");
}
