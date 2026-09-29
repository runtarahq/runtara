use runtara_agent_macro::{CapabilityInput, capability};
use serde::{Deserialize, Serialize};

#[derive(Deserialize, CapabilityInput)]
struct Input {
    id: String,
}

#[derive(Serialize)]
struct Output {
    id: String,
}

#[capability(module = "control", id = "get", side_effects = false)]
fn get(input: Input) -> Result<Output, String> {
    Ok(Output { id: input.id })
}

#[capability(module = "control", id = "cancel", side_effects = true)]
async fn cancel(input: Input) -> Result<Output, String> {
    Ok(Output { id: input.id })
}

runtara_agent_macro::agent_component!(
    agent = "control",
    control = true,
    capabilities = [get, cancel],
);

fn main() {
    assert!(!__CAPABILITY_SUSPENDS_GET);
    assert!(!__CAPABILITY_SUSPENDS_CANCEL);
    assert!(!__CAPABILITY_META_GET.suspends);
    assert!(__CAPABILITY_META_CANCEL.has_side_effects);
}
