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

mod custom {
    use super::*;

    // Another function, in another module, given the same id by hand.
    #[capability(id = "get", side_effects = false)]
    pub fn get_again(input: Input) -> Result<String, String> {
        Ok(input.id)
    }
}

runtara_agent_macro::agent_component!(
    agent = "probe",
    capabilities = [get, custom::get_again],
);

fn main() {}
