//! An ordinary agent with a suspending capability:
//! the glue answers it with the `suspended` outcome and reads the continuation
//! from the host's `runtara:agent/continuation`.
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

#[capability(id = "plain", side_effects = false)]
fn plain(input: Input) -> Result<Output, String> {
    Ok(Output { id: input.id })
}

#[capability(id = "pause", side_effects = false, suspends = true)]
async fn pause(input: Input, context: &SuspendContext) -> Result<Suspendable<Output>, String> {
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
    agent = "suspend-probe",
    capabilities = [plain, pause],
    suspending = [pause],
);

fn main() {
    assert!(__CAPABILITY_SUSPENDS_PAUSE);
    assert!(!__CAPABILITY_SUSPENDS_PLAIN);
    assert!(__CAPABILITY_META_PAUSE.suspends);
    assert_eq!(__CAPABILITY_META_PAUSE.output_type, "Output");
    // Natively the plain invoke adapter of a suspending capability refuses it.
    let waker = std::task::Waker::noop();
    let mut cx = std::task::Context::from_waker(waker);
    let mut refused = std::pin::pin!(__invoke_pause(serde_json::json!({"id": "x"})));
    let std::task::Poll::Ready(result) = refused.as_mut().poll(&mut cx) else {
        panic!("the refusal is immediate");
    };
    assert!(result.unwrap_err().contains(runtara_agent_suspension::SUSPENSION_UNSUPPORTED));
    let context = SuspendContext::default();
    let mut suspended = std::pin::pin!(__suspend_pause(serde_json::json!({"id": "x"}), &context));
    let std::task::Poll::Ready(Ok(Suspendable::Suspended { state, .. })) =
        suspended.as_mut().poll(&mut cx)
    else {
        panic!("the first invocation suspends");
    };
    assert_eq!(state, b"x");
}
