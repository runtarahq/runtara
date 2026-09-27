use runtara_agent_macro::{CapabilityInput, capability};
use runtara_agent_suspension::{SuspendContext, Suspendable, Wake};
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

#[capability(module = "control", id = "wait", side_effects = false, suspends = true)]
async fn wait(input: Input, context: &SuspendContext) -> Result<Suspendable<Output>, String> {
    Ok(match context.continuation() {
        None => Suspendable::Suspended {
            wakes: vec![Wake::At(1)],
            state: input.id.into_bytes(),
        },
        Some(state) => Suspendable::Completed(Output {
            id: String::from_utf8_lossy(state).into_owned(),
        }),
    })
}

runtara_agent_macro::agent_component!(
    agent = "control",
    control_executor = true,
    capabilities = [get, wait],
    suspending = [wait],
);

fn main() {
    assert!(__CAPABILITY_SUSPENDS_WAIT);
    assert!(!__CAPABILITY_SUSPENDS_GET);
    assert!(__CAPABILITY_META_WAIT.suspends);
    assert!(!__CAPABILITY_META_GET.suspends);
    // The declared output of a suspending capability is the `O` it completes with.
    assert_eq!(__CAPABILITY_META_WAIT.output_type, "Output");
}
