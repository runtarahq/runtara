# AI Agent streaming

Status: built (see [As built](#as-built)). What follows it is the spike
that answered whether an AI Agent step's model answer can reach a
conversation as it is written
([native-chat-phase-4.md](native-chat-phase-4.md), 4.7, the part not built),
and the decisions made on it.

## As built

An AI Agent step in a run that carries a conversation says its model's
answer there as the model writes it, then whole as the reply. A step whose
name begins with `_` (its id, when it has none) does not, nor one with an
output schema: its answer is data, not text for a person.

| Part | Where | What it does |
|---|---|---|
| The compiler | `runtara-workflows`, `manifest.rs` (`ai_agent_is_shown`) | gives a shown step's `chat-completion` or `chat-turn` input `stream: true`. Nothing in the DSL changes. |
| The step | `runtara-agent-ai-tools`, `streaming.rs` | asks the run whether it carries a conversation (a history read); if not, calls the model as before. If so, calls it streamed, says the first piece at once and the rest gathered about every 50 ms (`ConversationDelta`), under a reply id it makes, then the whole answer as `conversation_reply` under that id. A turn that asks for tools says its pieces and no reply. |
| The providers | `runtara-ai`, `completion_streamed` | OpenAI (`stream: true`, server-sent events), Gemini (`streamGenerateContent?alt=sse`) and Bedrock (`converse-stream`, AWS event stream framing). Each answers the same `CompletionResponse` as unstreamed, read by the same parser. An error, or an API that answers whole, is read as unstreamed. |
| The guest's HTTP | `runtara-http` `call_streamed_async`; `runtara-workflow-sdk` `http_streamed` | a body read a piece at a time; a body that was cut ends with an error, not as if whole. The native backend reads it whole, as one piece. |
| The way out | `/v1/http` with `stream`; `runtara-component-host` `send_streamed`; the server's `NativeOutboundHttp::request_streamed` | the answer is handed on as it arrives, through seal (`send_for_proxy`) or reqwest; no receipt is kept of it. In a run's own process (`runtara-run`) and the executor's relay, the body passes on unread. |
| The bounds | `StreamLimits` | a streamed call is not held to the 120 s bound: no first byte in 60 s, none for 30 s after, or 10 minutes in all ends it (`RUNTARA_STREAM_FIRST_BYTE_SECS`, `RUNTARA_STREAM_STALL_SECS`, `RUNTARA_STREAM_TOTAL_SECS`). The bytes are still capped. |

Where it departs from what the spike planned:

- **The reply id is made by the step's component**, at random, not by the
  compiled step: a component can make one as well, and the step need not
  pass it in.
- **A run is asked first whether it carries a conversation.** Only the
  first piece would have told it; asking first leaves every run without
  one calling its model as before, keeping the receipt and the 120 s bound
  of an unstreamed call.
- **A step with an output schema does not stream.**
- **The text of a turn that asks for tools** is said in pieces with no
  reply under their id; the pieces go when the run ends, as a retry's do.

Tested: each provider's parser and the event stream framing (unit tests);
a guest reading a streamed answer and being told when it is cut
(`runtara-environment`, `embedded_runner_test.rs`); the server's outbound
streaming through seal and reqwest (`runtara-server`,
`tests/outbound_http.rs`); and on running servers,
`e2e/test_ai_streaming.sh` (one process, and on an executor with
`e2e/test_executor_runner.sh ai_streaming`) and `e2e/test_two_roles.sh`
section 4b (the API and the engine apart, runs on executors, seal a process
of its own).

## The two questions

1. **Can the host hand a guest a response body as it arrives?** Yes.
2. **Does seal pass a streamed response through?** Yes, measured in both of
   the ways it runs.

Nothing on the way is whole by the shape of an interface but two of our own
types. Everything else buffers by choice, in a line or two that can pass the
stream on instead.

## How a model call travels

An AI Agent step runs in the guest (`runtara-workflow-stdlib`). Its provider
(`runtara-ai`) sends the request with `runtara-http`, which asks the
workflow API: `POST /v1/http`, with the destination and the connection in a
header. The host makes the call (through seal when a connection is named)
and answers with the upstream's status, headers and body.

| Hop | Where | Today | Streams? |
|---|---|---|---|
| Guest reads the `/v1/http` answer | `runtara-workflow-sdk`, `transport::raw` | `consume_body(..).collect()` | **By choice.** The body is a WASI 0.3 stream; read it a chunk at a time. |
| Host hands the guest the answer | wasmtime-wasi-http 50, `p3::body::HostBodyStreamProducer` | writes each frame of the host's body into the guest's stream as it is polled | **Yes**, given a body that streams. |
| In process: the run's hook to the `/v1` router | `runtara-component-host`, `workflow_api.rs` and `api_launches::RouterTransport` | `WorkflowApiTransport::call` answers `http::Response<Bytes>`; the router's body is read with `to_bytes` | **By type.** The trait answers whole bytes. Change it to answer a body, as the hook already takes one (`HookBody`). |
| On an executor: `runtara-run` to the executor | `runtara-run`, `guest.rs` | hyper answer, `Limited::new(..).collect()` | **By choice.** |
| The executor to the server's run channel | `runtara-executor`, `runs.rs` `Upstream::call` and `relay` | hyper answer, collected; answered as `Full` | **By choice.** |
| The server's run channel | `ApiLaunches::credentialed_router`, axum over TLS | axum answers a stream | **Yes.** |
| Idempotency receipts on `POST /v1/*` | `runtara-workflow-api`, the receipt layer | reads the answer whole to keep it for a retry | **By choice.** Pass it on and keep a copy, stored when the body ends. |
| `/v1/http` handler and the backend's `http` | `runtara-workflow-api` `post_http`; `ApiBackend::http` | `OutboundReply { body: Vec<u8> }` | **By type.** |
| The outbound host | `runtara-component-host` `OutboundHttpHost`; the server's `NativeOutboundHttp` | `Response { body: Vec<u8> }`; `read_bounded_response`, `send_bounded` | **By type**, and read whole in two places. |
| seal | `SealedCredentials::send` | answers `http::Response<seal_client::Body>`, a stream; `send_bounded` collects it | **Yes** (measured below). |

## Measured: seal streams

`crates/runtara-connections/tests/sealed_streaming.rs` points a sealed
OpenAI key at a stand-in provider on this machine that answers with four
server-sent events 400 ms apart, sends through `SealedCredentials::send`, and
records when each frame of the body arrives:

| seal | Pieces arrived at |
|---|---|
| in this process (`in_process`) | 0, 403, 805, 1207 ms |
| a process of its own, over its socket (`remote`, `SEAL_BIN`) | 3, 407, 809, 1212 ms |

So each piece arrives as the provider sends it, and seal attached the key,
which this process never held. The remote client is gRPC server streaming,
a message for each piece of the body (`seal-client` `grpc.rs`). seal holds
back the end of a piece while it might be the start of a credential echoed
back (`docs/threat-model.md` in seal), a few bytes at most, and reads a
response whole only where a policy seals fields of it, which a model
provider's key does not.

The second test runs where `SEAL_BIN` names a seal binary and is passed
over otherwise, so CI runs the first.

## Not measured: the host to the guest

That a body reaches a guest a frame at a time is read from wasmtime's code,
not measured: no guest here reads its body in pieces yet. The first step of
the build makes the in-process transport pass a body through and measures
it from a guest, before anything else depends on it.

## What building it takes

In order, each its own change with its own test:

1. **The workflow API answers with a body that streams.**
   `WorkflowApiTransport::call` answers `http::Response<HookBody>`;
   `RouterTransport`, `runtara-run` and the executor's relay pass bodies on.
   The receipt layer passes the answer on and keeps a copy. A body stays
   capped (`Caps::response_body_bytes`), counted as it passes.
2. **`/v1/http` streams.** `ApiBackend::http` and `OutboundHttpHost` answer
   a body; the server sends through `SealedCredentials::send` and reqwest's
   stream instead of `send_bounded` and `read_bounded_response`, counting
   bytes and the outbound budget as they pass. A call that asks for none of
   this (most) behaves as now.
3. **The guest reads it in pieces.** `runtara-workflow-sdk` gains a call that
   answers the upstream status and headers, then the body as a stream;
   `runtara-http`'s WASI backend uses it for a request that asks to stream.
4. **Providers stream.** OpenAI-style server-sent events (`stream: true`),
   Gemini's (`streamGenerateContent?alt=sse`), and Bedrock's
   `ConverseStream` (`application/vnd.amazon.eventstream`, binary frames).
   Each gives text pieces, then the same `CompletionResponse` as now, so the
   rest of the loop is unchanged. Tool calls are gathered whole from their
   pieces, as now.
5. **The AI Agent step says the pieces.** Gathered about every 50 ms, through
   the control call 4.7 added (`/v1/conversations/delta`), under a reply id
   the step makes (it knows its step, unlike a component). A run that
   carries no conversation is told so on the first piece and the step stops
   trying.
6. **Tests** with a stand-in provider that streams, in one process, on an
   executor, and through the API and the engine apart; for each provider's
   format, a unit test of its parser.

A rough size: 1 and 2 are the larger, three to four days together; 3 and 5
a day each; 4 a day for OpenAI and Gemini and a day for Bedrock's framing;
tests throughout.

## Decisions

Made on 2026-11-19.

1. **Which steps stream: by the step's name, not by the DSL.** The DSL is
   not to carry anything chat-specific. An AI Agent step whose run carries
   a conversation streams its answer there unless its name begins with `_`
   (a step with no name goes by its id). `_classify` is not shown;
   `answer` is. Nothing else in the DSL changes, and no validation or
   editor change follows. An existing chat workflow shows every AI Agent
   step not so named, so a step whose answer is not for the user is renamed
   with a leading `_`.
2. **How a streamed answer ends: as the whole reply, under the same id.**
   A step that streams says its whole answer as the conversation's reply
   under the reply id its pieces carried, when it finishes. A client
   replaces the pieces with it. A run that replied this way is not
   answered for again from its output.
3. **A retry part-way through** is as the spike found: the step streams
   its answer again from the start, under a new reply id, and the first
   attempt's pieces go when the run ends.
4. **Timeouts: a streamed call is judged by whether bytes keep coming.**
   The 120 s bound on an outbound call stays as it is for every call that
   does not stream. A streamed call is not held to it; it fails, as any
   outbound call can, when:

   | | Default |
   |---|---|
   | no first byte arrives within | 60 s |
   | no byte arrives for | 30 s, once the first has |
   | it has run in all for | 10 min |

   The first is long because a reasoning model may think before it writes;
   the second because a stream that is alive sends something every few
   seconds; the third so a provider that trickles for ever still ends. They
   are settings with these defaults, not constants.
