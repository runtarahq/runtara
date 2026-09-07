//! Real SQS component, standard parent cancellation and a local proxy fixture.
//! No AWS account, real queue, receipt handle or message is used.
use super::real_agent::{
    compose_agent, invoke_named_agent, read_proxy, respond, run_cancellation_fixture,
};
use super::*;
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use runtara_component_host::CallContext;
use serde_json::{Value, json};

const QUEUE: &str = "https://sqs.invalid/fixture/queue.fifo";

fn connection() -> Value {
    json!({"connection_id":"fixture-connection","integration_id":"aws_credentials","parameters":{}})
}

struct Case {
    capability: &'static str,
    operation: &'static str,
    input: Value,
    wire: Value,
}

fn cases() -> Vec<Case> {
    [
        ("queue-send-message", "SendMessage",
         json!({"queue_url":QUEUE,"message_body":"before","delay_seconds":"3","message_group_id":"fixture-group","message_deduplication_id":"fixture-dedup","message_attributes":{"kind":"fixture"}}),
         json!({"QueueUrl":QUEUE,"MessageBody":"before","DelaySeconds":3,"MessageGroupId":"fixture-group","MessageDeduplicationId":"fixture-dedup","MessageAttributes":{"kind":{"DataType":"String","StringValue":"fixture"}}})),
        ("queue-send-message-batch", "SendMessageBatch",
         json!({"queue_url":QUEUE,"entries":[{"id":"one","message_body":"before","message_group_id":"fixture-group"}]}),
         json!({"QueueUrl":QUEUE,"Entries":[{"Id":"one","MessageBody":"before","MessageGroupId":"fixture-group"}]})),
        ("queue-receive-messages", "ReceiveMessage",
         json!({"queue_url":QUEUE,"wait_time_seconds":"20","max_number_of_messages":"10","visibility_timeout":"60","attribute_names":["All"],"message_attribute_names":["kind"]}),
         json!({"QueueUrl":QUEUE,"WaitTimeSeconds":20,"MaxNumberOfMessages":10,"VisibilityTimeout":60,"MessageSystemAttributeNames":["All"],"MessageAttributeNames":["kind"]})),
        ("queue-delete-message", "DeleteMessage",
         json!({"queue_url":QUEUE,"receipt_handle":"fixture-receipt"}),
         json!({"QueueUrl":QUEUE,"ReceiptHandle":"fixture-receipt"})),
        ("queue-delete-message-batch", "DeleteMessageBatch",
         json!({"queue_url":QUEUE,"entries":[{"id":"one","receipt_handle":"fixture-receipt"}]}),
         json!({"QueueUrl":QUEUE,"Entries":[{"Id":"one","ReceiptHandle":"fixture-receipt"}]})),
        ("queue-change-message-visibility", "ChangeMessageVisibility",
         json!({"queue_url":QUEUE,"receipt_handle":"fixture-receipt","visibility_timeout":"0"}),
         json!({"QueueUrl":QUEUE,"ReceiptHandle":"fixture-receipt","VisibilityTimeout":0})),
        ("queue-change-message-visibility-batch", "ChangeMessageVisibilityBatch",
         json!({"queue_url":QUEUE,"entries":[{"id":"one","receipt_handle":"fixture-receipt","visibility_timeout":15}]}),
         json!({"QueueUrl":QUEUE,"Entries":[{"Id":"one","ReceiptHandle":"fixture-receipt","VisibilityTimeout":15}]})),
        ("queue-create-queue", "CreateQueue",
         json!({"queue_name":"fixture.fifo","fifo_queue":"true","attributes":{"VisibilityTimeout":"30"},"tags":{"kind":"fixture"}}),
         json!({"QueueName":"fixture.fifo","Attributes":{"FifoQueue":"true","VisibilityTimeout":"30"},"tags":{"kind":"fixture"}})),
        ("queue-delete-queue", "DeleteQueue", json!({"queue_url":QUEUE}), json!({"QueueUrl":QUEUE})),
        ("queue-list-queues", "ListQueues",
         json!({"queue_name_prefix":"before","max_results":"7","next_token":"fixture-page"}),
         json!({"QueueNamePrefix":"before","MaxResults":7,"NextToken":"fixture-page"})),
        ("queue-get-queue-url", "GetQueueUrl",
         json!({"queue_name":"fixture.fifo","queue_owner_aws_account_id":"000000000000"}),
         json!({"QueueName":"fixture.fifo","QueueOwnerAWSAccountId":"000000000000"})),
        ("queue-get-queue-attributes", "GetQueueAttributes", json!({"queue_url":QUEUE}), json!({"QueueUrl":QUEUE,"AttributeNames":["All"]})),
        ("queue-set-queue-attributes", "SetQueueAttributes",
         json!({"queue_url":QUEUE,"attributes":{"VisibilityTimeout":"30"},"sqs_managed_sse_enabled":"true"}),
         json!({"QueueUrl":QUEUE,"Attributes":{"VisibilityTimeout":"30","SqsManagedSseEnabled":"true"}})),
        ("queue-purge-queue", "PurgeQueue", json!({"queue_url":QUEUE}), json!({"QueueUrl":QUEUE})),
        ("queue-list-queue-tags", "ListQueueTags", json!({"queue_url":QUEUE}), json!({"QueueUrl":QUEUE})),
        ("queue-tag-queue", "TagQueue", json!({"queue_url":QUEUE,"tags":{"kind":"fixture"}}), json!({"QueueUrl":QUEUE,"Tags":{"kind":"fixture"}})),
        ("queue-untag-queue", "UntagQueue", json!({"queue_url":QUEUE,"tag_keys":["kind"]}), json!({"QueueUrl":QUEUE,"TagKeys":["kind"]})),
    ].into_iter().map(|(capability,operation,mut input,wire)| {
        input["_connection"] = connection();
        Case { capability, operation, input, wire }
    }).collect()
}

async fn request(
    socket: &mut tokio::net::TcpStream,
    operation: &str,
    wire: &Value,
) -> anyhow::Result<()> {
    let envelope = read_proxy(socket).await?;
    assert_eq!(envelope["url"], "/");
    assert_eq!(envelope["method"], "POST");
    assert_eq!(envelope["connection_id"], "fixture-connection");
    assert_eq!(envelope["aws_service"], "sqs");
    assert_eq!(envelope["timeout_ms"], 65_000);
    assert_eq!(
        envelope["headers"]["X-Amz-Target"],
        format!("AmazonSQS.{operation}")
    );
    assert_eq!(
        envelope["headers"]["Content-Type"],
        "application/x-amz-json-1.0"
    );
    let body = BASE64.decode(envelope["body_raw"].as_str().unwrap())?;
    assert_eq!(&serde_json::from_slice::<Value>(&body)?, wire);
    Ok(())
}

async fn cancellation(partial: bool) -> anyhow::Result<()> {
    for case in cases() {
        let bytes = compose_agent(
            "sqs",
            case.capability,
            &serde_json::to_vec(&case.input)?,
            "queue-list-queues",
            &serde_json::to_vec(&json!({"_connection":connection(),"queue_name_prefix":"after"}))?,
        )?;
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let proxy = format!("http://{}/proxy", listener.local_addr()?);
        let started = Arc::new(Notify::new());
        let cleaned = Arc::new(Notify::new());
        let server = tokio::spawn({
            let started = started.clone();
            let cleaned = cleaned.clone();
            async move {
                let (mut socket, _) = listener.accept().await?;
                request(&mut socket, case.operation, &case.wire).await?;
                if partial {
                    socket
                        .write_all(
                            b"HTTP/1.1 200 OK\r\nContent-Length: 400\r\nConnection: close\r\n\r\n{",
                        )
                        .await?;
                }
                started.notify_one();
                match socket.read(&mut [0]).await {
                    Ok(0) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::ConnectionReset => {}
                    other => anyhow::bail!("SQS wait was not closed: {other:?}"),
                }
                cleaned.notify_one();
                // No retry, DeleteMessage or visibility adjustment may run as
                // implicit cleanup. The next call must be this fresh invocation.
                let (mut socket, _) = listener.accept().await?;
                request(
                    &mut socket,
                    "ListQueues",
                    &json!({"QueueNamePrefix":"after"}),
                )
                .await?;
                respond(&mut socket, json!({"status":200,"headers":{},"body":{"QueueUrls":[QUEUE],"NextToken":"next-page"}})).await
            }
        });
        let output = run_cancellation_fixture(
            bytes,
            CallContext::for_test("fixture-tenant", proxy, "", "", ""),
            started,
            cleaned,
            server,
        )
        .await?;
        assert_eq!(
            output,
            json!({"success":true,"queue_urls":[QUEUE],"next_token":"next-page"})
        );
    }
    Ok(())
}

#[tokio::test]
async fn sqs_cancel_closes_pending_headers_for_every_capability() -> anyhow::Result<()> {
    cancellation(false).await
}

#[tokio::test]
async fn sqs_cancel_closes_partial_body_for_every_capability() -> anyhow::Result<()> {
    cancellation(true).await
}

async fn invoke_response(
    case: Case,
    envelope: Value,
) -> anyhow::Result<Result<Vec<u8>, runtara_component_host::ErrorInfo>> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let proxy = format!("http://{}/proxy", listener.local_addr()?);
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await?;
        request(&mut socket, case.operation, &case.wire).await?;
        respond(&mut socket, envelope).await
    });
    let result = tokio::time::timeout(
        Duration::from_secs(10),
        invoke_named_agent(
            "sqs",
            CallContext::for_test("fixture-tenant", proxy, "", "", ""),
            case.capability,
            serde_json::to_vec(&case.input)?,
        ),
    )
    .await;
    let result = match result {
        Ok(Ok(result)) => result,
        other => {
            server.abort();
            let _ = server.await;
            anyhow::bail!("SQS invocation failed: {other:?}");
        }
    };
    server.await??;
    Ok(result)
}

#[tokio::test]
async fn sqs_http_failures_remain_soft_results_for_every_capability() -> anyhow::Result<()> {
    for status in [403, 503] {
        for case in cases() {
            let bytes = invoke_response(case,json!({"status":status,"headers":{},"body":{"__type":"com.amazonaws.sqs#FixtureFailure","message":"fixture rejection"}})).await?.expect("HTTP errors are existing soft results");
            let output: Value = serde_json::from_slice(&bytes)?;
            assert_eq!(output["success"], false);
            assert_eq!(output["error"], "FixtureFailure: fixture rejection");
        }
    }
    Ok(())
}

#[tokio::test]
async fn sqs_success_preserves_receipts_partial_batch_failures_and_metadata() -> anyhow::Result<()>
{
    for case in cases() {
        let (body, expected) = match case.operation {
            "SendMessage" => (
                json!({"MessageId":"one","SequenceNumber":"123","MD5OfMessageBody":"fixture-md5"}),
                json!({"success":true,"message_id":"one","sequence_number":"123","md5_of_message_body":"fixture-md5"}),
            ),
            "SendMessageBatch" | "DeleteMessageBatch" | "ChangeMessageVisibilityBatch" => (
                json!({"Successful":[{"Id":"one","MessageId":"m1"}],"Failed":[{"Id":"two","Code":"FixtureFailure","Message":"fixture rejection","SenderFault":true}]}),
                json!({"success":true,"successful":[{"id":"one","message_id":"m1"}],"failed":[{"id":"two","code":"FixtureFailure","message":"fixture rejection","sender_fault":true}]}),
            ),
            "ReceiveMessage" => (
                json!({"Messages":[{"MessageId":"one","ReceiptHandle":"fixture-receipt","Body":"hello","Attributes":{"ApproximateReceiveCount":"1"},"MessageAttributes":{"kind":{"DataType":"String","StringValue":"fixture"}}}]}),
                json!({"success":true,"count":1,"messages":[{"message_id":"one","receipt_handle":"fixture-receipt","body":"hello","attributes":{"ApproximateReceiveCount":"1"},"message_attributes":{"kind":{"DataType":"String","StringValue":"fixture"}}}]}),
            ),
            "CreateQueue" | "GetQueueUrl" => (
                json!({"QueueUrl":QUEUE}),
                json!({"success":true,"queue_url":QUEUE}),
            ),
            "ListQueues" => (
                json!({"QueueUrls":[QUEUE],"NextToken":"next-page"}),
                json!({"success":true,"queue_urls":[QUEUE],"next_token":"next-page"}),
            ),
            "GetQueueAttributes" => (
                json!({"Attributes":{"VisibilityTimeout":"30"}}),
                json!({"success":true,"attributes":{"VisibilityTimeout":"30"}}),
            ),
            "ListQueueTags" => (
                json!({"Tags":{"kind":"fixture"}}),
                json!({"success":true,"tags":{"kind":"fixture"}}),
            ),
            _ => (Value::Null, json!({"success":true})),
        };
        let envelope = if body.is_null() {
            json!({"status":200,"headers":{},"body_raw":""})
        } else {
            json!({"status":200,"headers":{},"body":body})
        };
        let bytes = invoke_response(case, envelope)
            .await?
            .expect("successful SQS response");
        assert_eq!(serde_json::from_slice::<Value>(&bytes)?, expected);
    }
    Ok(())
}

#[tokio::test]
async fn sqs_transport_and_missing_connection_keep_distinct_retry_contracts() -> anyhow::Result<()>
{
    let error = invoke_response(
        cases().remove(0),
        json!({"status":200,"headers":{},"body_raw":"!invalid-base64!"}),
    )
    .await?
    .expect_err("malformed transport must fail");
    assert_eq!(error.code, "SQS_NETWORK_ERROR");
    assert_eq!(error.category, "transient");
    assert!(error.retryable);

    let result = tokio::time::timeout(
        Duration::from_secs(10),
        invoke_named_agent(
            "sqs",
            CallContext::for_test("fixture-tenant", "http://127.0.0.1:1/unused", "", "", ""),
            "queue-receive-messages",
            serde_json::to_vec(&json!({"queue_url":QUEUE}))?,
        ),
    )
    .await??;
    let error = result.expect_err("missing connection must fail before I/O");
    assert_eq!(error.code, "SQS_MISSING_CONNECTION");
    assert_eq!(error.category, "permanent");
    assert!(!error.retryable);
    Ok(())
}
