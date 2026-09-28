use runtara_agent_macro::{CapabilityInput, capability};
use serde::Deserialize;

#[derive(Deserialize, CapabilityInput)]
struct Input {
    id: String,
}

#[capability(id = "get", side_effects = false)]
fn get(input: Input) -> Result<String, String> {
    Ok(input.id)
}

runtara_agent_macro::agent_component!(
    agent = "probe",
    capabilities = [get],
    suspending = [get],
);

fn main() {}
