# runtara-agent-conversation

The built-in `conversation` agent: a workflow says something to whoever it
is talking with, from any step of a run.

| Capability | |
|---|---|
| `reply` | Text, and attachments by where to fetch them |

A run started by a conversation (`docs/native-chat.md`) is read by it: what
`reply` says becomes the conversation's `reply` events, which its client
reads and its reply workflow is started with. A run that never calls
`reply` still answers with what it returns. In a run no conversation
carries, `reply` leaves an event on the run and nothing reads it.

`reply` leaves an event on the run and holds no credential, connection or
network access. It is available on every pricing tier.
