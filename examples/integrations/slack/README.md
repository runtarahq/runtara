# Slack adapter

Lets people talk to a Runtara workflow from Slack. It is made of two
workflows and the `slack` agent, with no code in the server
(`docs/native-chat-phase-5.md`):

| File | What it does |
| --- | --- |
| `inbound.json` | Behind a synchronous HTTP trigger bound to the Slack connection. Answers Slack's URL handshake, and sends each person's message into the conversation of its channel (`slack:<team>:<channel>`) with `conversation.send`, keyed by Slack's event id so a delivery Slack repeats is one message. The bot's own messages, and edits and joins, are left out. |
| `outbound.json` | The conversation's reply workflow: posts each reply to the channel, and to the thread the message came from, with `slack.send-message`. |
| `install.sh` | Installs both and the trigger, and prints the URL to give Slack. |

The workflow that answers is any workflow: it is started with each message
(`data.message`), and what it replies, by its output or with
`conversation.reply`, is posted back.

## Set up

1. Create a Slack app with a bot token (`chat:write`, and the `*:history`
   scopes of the conversations it should hear) and note its signing secret.
2. In Runtara, create a **Slack Bot** connection with both. The signing
   secret is sealed and only used to check Slack's calls.
3. Install:

   ```bash
   RUNTARA_API=https://runtara.example.com/api/runtime RUNTARA_API_KEY=… \
   SLACK_CONNECTION_ID=… TARGET_WORKFLOW_ID=… \
     examples/integrations/slack/install.sh
   ```

4. In the app's **Event Subscriptions**, set the Request URL to the printed
   `requestUrl` (Slack checks it with the handshake), and subscribe to the
   bot events it should see (`message.im`, `message.channels`, …).

Each call is checked against the signing secret before anything runs; one
that is unsigned, signed with another secret or over another body, or more
than five minutes old is refused.

`e2e/test_slack_adapter.sh` installs it with `install.sh` against a stand-in
Slack and checks all of the above.
