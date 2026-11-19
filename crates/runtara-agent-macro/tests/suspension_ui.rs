//! Suspension declarations are checked at compile time:
//! the `suspending` list must match `#[capability(suspends = true)]`, and a
//! suspending capability must take `&SuspendContext` and return
//! `Result<Suspendable<O>, E>`. So is the capability list itself: no two of an
//! agent's capabilities may have the same id, wherever they are declared.

#[test]
fn suspension_declarations_are_checked_at_compile_time() {
    let cases = trybuild::TestCases::new();
    cases.pass("tests/ui/pass/*.rs");
    cases.compile_fail("tests/ui/fail/*.rs");
}
