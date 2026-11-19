// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! A model's answer said to the run's conversation as it is written
//! (`docs/ai-agent-streaming.md`): its text in pieces, gathered about every
//! 50 ms, under a reply id made for it; then the whole answer as the reply,
//! under the same id, which a client puts in the pieces' place.
//!
//! A run that carries no conversation is told so on the first piece, and
//! nothing more is said. A piece that is not said is not a failure of the
//! step: the whole reply still is, and the answer is the step's either way.

use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// How long text is held to be said with what follows it.
const GATHER: Duration = Duration::from_millis(50);
/// The most text one piece carries (the conversation's own bound).
const MAX_PIECE_BYTES: usize = 8 * 1024;
/// The most text one reply carries (the conversation's own bound).
const MAX_REPLY_BYTES: usize = 64 * 1024;
/// The kind of event a reply is.
const REPLY_EVENT: &str = "conversation_reply";

/// The conversation a streamed answer is said to.
pub(crate) struct ToConversation<H: Host = Run> {
    host: H,
    reply_id: String,
    held: String,
    last_said: Option<Instant>,
    /// Whether the run carries no conversation, as the first piece found.
    no_conversation: bool,
    /// Whether pieces are no longer said (none can be).
    stopped: bool,
}

/// What a streamed answer is said through: the run's workflow API.
pub(crate) trait Host {
    /// Says a piece; `Err(true)` when there is no conversation to say it to.
    async fn piece(&mut self, piece: Value) -> Result<(), bool>;
    async fn reply(&mut self, reply: Value) -> Result<(), String>;
}

impl ToConversation<Run> {
    pub(crate) fn new() -> Self {
        Self::with(Run)
    }
}

impl<H: Host> ToConversation<H> {
    pub(crate) fn with(host: H) -> Self {
        Self {
            host,
            reply_id: reply_id(),
            held: String::new(),
            last_said: None,
            no_conversation: false,
            stopped: false,
        }
    }

    /// Says what is held, in pieces of at most the conversation's bound.
    async fn say_held(&mut self) {
        let held = std::mem::take(&mut self.held);
        self.last_said = Some(Instant::now());
        if self.stopped || held.is_empty() {
            return;
        }
        for piece in split(&held, MAX_PIECE_BYTES) {
            let said = self
                .host
                .piece(json!({"replyId": self.reply_id, "text": piece}))
                .await;
            if let Err(no_conversation) = said {
                self.no_conversation |= no_conversation;
                self.stopped = true;
                return;
            }
        }
    }

    /// Says the whole answer as the reply, under the pieces' id; nothing if
    /// the run carries no conversation or the answer has no text.
    pub(crate) async fn reply(mut self, text: &str) -> Result<(), String> {
        self.say_held().await;
        if self.no_conversation || text.trim().is_empty() {
            return Ok(());
        }
        let text = split(text, MAX_REPLY_BYTES).next().unwrap_or_default();
        self.host
            .reply(json!({
                "format": "markdown",
                "attachments": [],
                "text": text,
                "replyId": self.reply_id,
            }))
            .await
    }

    /// Says what is held, and nothing after; for a turn whose answer is not
    /// the reply (it asks for tools).
    pub(crate) async fn end(mut self) {
        self.say_held().await;
    }
}

impl<H: Host> runtara_ai::Pieces for ToConversation<H> {
    fn piece<'a>(
        &'a mut self,
        text: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + 'a>> {
        Box::pin(async move {
            self.held.push_str(text);
            // The first piece is said at once; the rest about every GATHER.
            if self
                .last_said
                .is_none_or(|last_said| last_said.elapsed() >= GATHER)
            {
                self.say_held().await;
            }
        })
    }
}

/// `text` in pieces of at most `bound` bytes, cut between characters.
fn split(text: &str, bound: usize) -> impl Iterator<Item = &str> {
    let mut rest = text;
    std::iter::from_fn(move || {
        if rest.is_empty() {
            return None;
        }
        let mut end = rest.len().min(bound);
        while !rest.is_char_boundary(end) {
            end -= 1;
        }
        let (piece, after) = rest.split_at(end);
        rest = after;
        Some(piece)
    })
}

/// An id no other reply has: 128 bits the system makes at random.
fn reply_id() -> String {
    use std::hash::{BuildHasher, Hasher};
    // Each RandomState is keyed from the system's randomness.
    let half = || {
        let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
        hasher.write_u8(0);
        hasher.finish()
    };
    format!("ai-{:016x}{:016x}", half(), half())
}

/// The run's workflow API.
pub(crate) struct Run;

#[cfg(target_arch = "wasm32")]
impl Host for Run {
    async fn piece(&mut self, piece: Value) -> Result<(), bool> {
        use runtara_workflow_api::ControlCall;
        match runtara_workflow_sdk::control(&ControlCall::ConversationDelta(piece), None).await {
            Ok(_) => Ok(()),
            Err(runtara_workflow_sdk::Error::Api(error)) => Err(matches!(
                error.code.as_str(),
                "not-found" | "requires-instance"
            )),
            Err(runtara_workflow_sdk::Error::Transport(_)) => Err(false),
        }
    }

    async fn reply(&mut self, reply: Value) -> Result<(), String> {
        runtara_workflow_sdk::emit_events(&[runtara_workflow_api::Event {
            kind: REPLY_EVENT.to_owned(),
            payload: reply,
        }])
        .await
        .map_err(|error| format!("the reply was not left: {error:?}"))
    }
}

/// Outside a component there is no run, and so no conversation.
#[cfg(not(target_arch = "wasm32"))]
impl Host for Run {
    async fn piece(&mut self, _: Value) -> Result<(), bool> {
        Err(true)
    }

    async fn reply(&mut self, _: Value) -> Result<(), String> {
        let _ = REPLY_EVENT;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtara_ai::Pieces;

    #[derive(Default)]
    struct Kept {
        pieces: Vec<Value>,
        replies: Vec<Value>,
        no_conversation: bool,
    }

    impl Host for &mut Kept {
        async fn piece(&mut self, piece: Value) -> Result<(), bool> {
            if self.no_conversation {
                return Err(true);
            }
            self.pieces.push(piece);
            Ok(())
        }

        async fn reply(&mut self, reply: Value) -> Result<(), String> {
            self.replies.push(reply);
            Ok(())
        }
    }

    #[tokio::test]
    async fn pieces_are_gathered_then_the_whole_answer_is_the_reply() {
        let mut kept = Kept::default();
        let mut to = ToConversation::with(&mut kept);
        for text in ["Once", " upon", " a"] {
            to.piece(text).await;
        }
        std::thread::sleep(GATHER);
        to.piece(" time").await;
        let reply_id = to.reply_id.clone();
        to.reply("Once upon a time").await.unwrap();
        let said: Vec<&str> = kept
            .pieces
            .iter()
            .map(|piece| piece["text"].as_str().unwrap())
            .collect();
        // The first at once; the next two held together; the last after GATHER.
        assert_eq!(said, ["Once", " upon a time"]);
        assert!(
            kept.pieces
                .iter()
                .all(|piece| piece["replyId"] == reply_id.as_str())
        );
        assert_eq!(kept.replies.len(), 1);
        assert_eq!(kept.replies[0]["text"], "Once upon a time");
        assert_eq!(kept.replies[0]["replyId"], reply_id.as_str());
    }

    #[tokio::test]
    async fn a_run_with_no_conversation_is_told_once_and_not_replied_for() {
        let mut kept = Kept {
            no_conversation: true,
            ..Kept::default()
        };
        let mut to = ToConversation::with(&mut kept);
        to.piece("Hi").await;
        to.piece(" there").await;
        to.reply("Hi there").await.unwrap();
        assert!(kept.pieces.is_empty());
        assert!(kept.replies.is_empty());
    }

    #[test]
    fn long_text_is_cut_between_characters() {
        let text = "é".repeat(5);
        let pieces: Vec<&str> = split(&text, 3).collect();
        assert_eq!(pieces.concat(), text);
        assert!(pieces.iter().all(|piece| piece.len() <= 3));
    }

    #[test]
    fn each_reply_has_an_id_of_its_own() {
        assert_ne!(reply_id(), reply_id());
    }
}
