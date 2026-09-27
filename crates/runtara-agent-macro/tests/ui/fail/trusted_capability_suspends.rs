use runtara_agent_macro::{CapabilityInput, capability};
use serde::Deserialize;

#[derive(Deserialize, CapabilityInput)]
struct Input {
    id: String,
}

#[capability(id = "sign", trusted = true, suspends = true)]
fn sign(
    input: Input,
    _context: &runtara_agent_suspension::SuspendContext,
) -> Result<runtara_agent_suspension::Suspendable<String>, String> {
    Ok(runtara_agent_suspension::Suspendable::Completed(input.id))
}

fn main() {}
