use runtara_agent_macro::{CapabilityInput, capability};
use serde::Deserialize;

#[derive(Deserialize, CapabilityInput)]
struct Input {
    id: String,
}

// A suspending capability must return `Result<Suspendable<O>, E>`.
#[capability(id = "plain-result", suspends = true)]
fn plain_result(input: Input, _context: &runtara_agent_suspension::SuspendContext) -> Result<String, String> {
    Ok(input.id)
}

// And must take the context.
#[capability(id = "no-context", suspends = true)]
fn no_context(
    input: Input,
) -> Result<runtara_agent_suspension::Suspendable<String>, String> {
    Ok(runtara_agent_suspension::Suspendable::Completed(input.id))
}

fn main() {}
