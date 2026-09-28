use runtara_agent_macro::{CapabilityInput, capability};
use runtara_agent_suspension::{SuspendContext, Suspendable};
use serde::Deserialize;

#[derive(Deserialize, CapabilityInput)]
struct Input {
    id: String,
}

#[capability(id = "wait", side_effects = false, suspends = true)]
async fn wait(input: Input, _context: &SuspendContext) -> Result<Suspendable<String>, String> {
    Ok(Suspendable::Completed(input.id))
}

runtara_agent_macro::agent_component!(
    agent = "control",
    control_executor = true,
    capabilities = [wait],
);

fn main() {}
