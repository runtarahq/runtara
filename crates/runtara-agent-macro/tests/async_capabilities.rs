//! The normal capability macro preserves coercion/errors and future ownership.
use runtara_agent_macro::{CapabilityInput, capability};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    future::Future,
    sync::atomic::{AtomicUsize, Ordering},
    task::{Context, Poll, Waker},
};

#[derive(CapabilityInput, Deserialize)]
struct Input {
    count: i64,
    #[serde(default)]
    mode: String,
}

#[capability(module = "test", id = "async-value", side_effects = false)]
async fn value(input: Input) -> Result<Value, String> {
    match input.mode.as_str() {
        "plain" => Err("plain failure".into()),
        "structured" => Err(
            json!({"code":"RETRY_LATER","category":"transient","retry_after_ms":23}).to_string(),
        ),
        _ => Ok(json!({"count": input.count})),
    }
}

#[capability(module = "test", id = "sync-value", side_effects = false)]
fn sync_value(input: Input) -> Result<Value, String> {
    Ok(json!({"count": input.count}))
}

fn ready<F: Future>(future: F) -> F::Output {
    let mut future = std::pin::pin!(future);
    match future
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
    {
        Poll::Ready(value) => value,
        Poll::Pending => panic!("this fixture must resolve on its first poll"),
    }
}

#[test]
fn async_and_sync_descriptors_preserve_coercion_and_metadata() {
    let input = json!({"count":"42"});
    let expected = (__CAPABILITY_EXECUTOR_SYNC_VALUE.execute)(input.clone()).unwrap();
    assert_eq!(
        ready((__CAPABILITY_EXECUTOR_VALUE.execute)(input)).unwrap(),
        expected
    );
    assert_eq!(
        __CAPABILITY_META_VALUE.input_type,
        __CAPABILITY_META_SYNC_VALUE.input_type
    );
    assert_eq!(
        __CAPABILITY_META_VALUE.output_type,
        __CAPABILITY_META_SYNC_VALUE.output_type
    );
    assert_eq!(__CAPABILITY_EXECUTOR_VALUE.module, "test");
    assert_eq!(__CAPABILITY_EXECUTOR_VALUE.capability_id, "async-value");
}

#[test]
fn async_dispatch_preserves_input_and_capability_error_envelopes() {
    let invalid = ready(__executor_value(json!({"count":{}}))).unwrap_err();
    let invalid: Value = serde_json::from_str(&invalid).unwrap();
    assert_eq!(invalid["code"], "INPUT_DESERIALIZATION_ERROR");
    assert_eq!(invalid["category"], "permanent");
    assert!(
        invalid["message"]
            .as_str()
            .unwrap()
            .starts_with("Invalid input for async-value:")
    );
    let plain = ready(__executor_value(json!({"count":1,"mode":"plain"}))).unwrap_err();
    let plain: Value = serde_json::from_str(&plain).unwrap();
    assert_eq!(
        plain,
        json!({"code":"CAPABILITY_ERROR","message":"plain failure","category":"permanent","severity":"error"})
    );
    let structured = ready(__executor_value(json!({"count":1,"mode":"structured"}))).unwrap_err();
    assert_eq!(
        serde_json::from_str::<Value>(&structured).unwrap(),
        json!({"code":"RETRY_LATER","category":"transient","retry_after_ms":23})
    );
}

struct BrokenOutput;
impl serde::Serialize for BrokenOutput {
    fn serialize<S: serde::Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
        Err(serde::ser::Error::custom("cannot encode output"))
    }
}

#[capability(module = "test")]
async fn broken_output(_input: Input) -> Result<BrokenOutput, String> {
    Ok(BrokenOutput)
}

#[test]
fn async_output_serialization_errors_keep_the_existing_envelope() {
    let error = ready(__executor_broken_output(json!({"count":1}))).unwrap_err();
    let error: Value = serde_json::from_str(&error).unwrap();
    assert_eq!(error["code"], "OUTPUT_SERIALIZATION_ERROR");
    assert_eq!(
        error["message"],
        "Failed to serialize result for broken-output: cannot encode output"
    );
}

static DROPS: AtomicUsize = AtomicUsize::new(0);
struct Guard;
impl Drop for Guard {
    fn drop(&mut self) {
        DROPS.fetch_add(1, Ordering::SeqCst);
    }
}

#[capability(module = "test")]
async fn pending(_input: Input) -> Result<Value, String> {
    let _guard = Guard;
    std::future::pending().await
}

#[test]
fn uniform_component_adapter_preserves_sync_and_async_results() {
    let input = json!({"count":"42"});
    assert_eq!(
        ready(__invoke_sync_value(input.clone())),
        ready(__invoke_value(input))
    );
    let error = ready(__invoke_value(json!({"count":1,"mode":"structured"}))).unwrap_err();
    assert_eq!(
        serde_json::from_str::<Value>(&error).unwrap()["retry_after_ms"],
        23
    );
}

#[test]
fn dropping_the_dispatch_future_drops_the_pending_capability() {
    let calls = [
        (__CAPABILITY_EXECUTOR_PENDING.execute)(json!({"count":1})),
        Box::pin(__invoke_pending(json!({"count":1}))),
    ];
    for (index, mut invocation) in calls.into_iter().enumerate() {
        assert!(
            invocation
                .as_mut()
                .poll(&mut Context::from_waker(Waker::noop()))
                .is_pending()
        );
        assert_eq!(DROPS.load(Ordering::SeqCst), index);
        drop(invocation);
        assert_eq!(DROPS.load(Ordering::SeqCst), index + 1);
    }
}
