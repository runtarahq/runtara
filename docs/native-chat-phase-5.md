# The native chat, phase 5: what a platform needs, and the first adapter

Phase 4 ([native-chat-phase-4.md](native-chat-phase-4.md)) made the
conversation whole without a platform. Phase 5 adds what a chat platform's
webhook needs of the server, and then the first adapter on it. Nothing of
any platform is written in the server: what a platform needs is declared
as data or done by workflows and agents (AGENTS.md, `docs/native-chat.md`).

## The parts

| | What | Tests that say it is done |
|---|---|---|
| 5.1 | **A webhook's headers in the trigger's input**: `data.headers`, names in lower case, without the caller's credentials (`authorization`, `cookie`, `proxy-authorization`) and without a header a check found a secret in. | A run started by a webhook reads a header it was sent; it is not handed a cookie, nor a checked secret token. |
| 5.2 | **Webhook checks declared, not written.** A connection type declares how its webhooks are signed; intake (and the engine, when it receives webhooks itself) checks every webhook that way with seal. Mailgun's code is replaced by its declaration; Slack and Telegram are declared beside it. | Each declared check accepts what its platform sends and refuses a changed body, a wrong secret, a missing signature and an old timestamp. A trigger bound to a Slack connection refuses an unsigned webhook end to end. |
| 5.3 | **A workflow's own answer to a synchronous trigger.** An HTTP trigger with `synchronous: true` runs its workflow and waits; a workflow whose output has `httpResponse` (`status`, `headers`, `body`) is answered with exactly that, on that trigger and on `http-sync`. | Slack's handshake is answered with its challenge; a workflow without `httpResponse` is answered as before; a status out of range, or a header that is not the workflow's to set, is not sent. |
| 5.4 | **The first adapter: Slack.** Two workflows (`integrations/slack/`): the inbound one, behind a synchronous trigger bound to the Slack connection, answers the handshake and sends each message into the conversation with `conversation.send`; the outbound one, the conversation's reply workflow, posts each reply with the `slack` agent. | Against a stand-in Slack: a signed message reaches the conversation once, its reply is posted to the same channel and thread once, a forged message is refused, and the bot's own messages are not sent back in. |

## The declaration

A declared check is data, kept beside where each secret goes
(`runtara-connections`' `webhooks` module, next to `sealed::policy`):

- `secretField`: the connection's parameter that holds the secret. Its seal
  policy only verifies: it is never sent.
- `kind`: `token` (the secret, sent as it is in a header: Telegram) or
  `hmacSha256` (a signature over a message: Slack, Mailgun).
- For `hmacSha256`, three templates over the request: `message` (what is
  signed), `signature` (where the signature is), and optionally
  `timestamp` with `maxAgeSecs`. A template is text with `{body}`,
  `{header:NAME}` and `{field:/json/pointer}` in it; a field may name
  alternatives, `{field:/signature/token|/token}`. `signaturePrefix` is
  removed from the signature (`v0=`), and `encoding` says how it is
  written (`hex` or `base64`).
- For `token`, `header`: the header the secret comes in.

seal is asked whether the signature is the secret's HMAC of the message, or
whether the token is the secret; it answers yes or no and the server never
reads the secret. A timestamp too old, or a template that names what the
request does not have, refuses the webhook without asking seal.

What is kept with a trigger for intake (`webhook_verification`) carries the
declaration with the sealed secret, so intake checks from that alone.

## A platform that signs another way

Teams signs with a token checked against published keys. It is not
declared: its agent checks it, in the integration's inbound workflow.

## Decisions

1. **Declared in code, as data.** The declarations sit with the
   connection types' secret policies, which already say each secret is only
   for verifying. An agent built elsewhere declares no check: a connection
   type that signs webhooks needs a secret seal verifies with, and that is
   a compiled connection type.
2. **The synchronous trigger is a trigger setting**, not a new route, so
   it is checked as every trigger's webhook is.
3. **`httpResponse` is opt-in.** A workflow that does not set it is
   answered as it is today.
4. **The first adapter is Slack.** It needs all of 5.1 to 5.3: signed
   webhooks, a handshake answered at once, and a reply with the agent that
   exists. Telegram's check is declared too; its adapter needs a Telegram
   agent and follows.
5. **A stand-in for a provider's fixed API in end-to-end tests**:
   `RUNTARA_STAND_IN_API_BASES` (`slack_bot=http://127.0.0.1:18996`) points
   a connection type's API at a stand-in, in seal's policy and in the
   proxy. It is for tests, as `RUNTARA_PROXY_ALLOWED_HOSTS` is.
