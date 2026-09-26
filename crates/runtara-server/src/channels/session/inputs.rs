use super::*;
use runtara_core::persistence::inputs::InputRequest;
use std::collections::HashSet;

#[derive(Debug, PartialEq, Eq)]
pub(super) enum InputProgress {
    NoInput,
    Waiting,
    Ambiguous,
    Retained,
    /// A buffered reply's request closed first; it was dropped, not re-aimed.
    Undelivered,
}

/// Which request an arriving reply answers, decided when it arrives.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum ReplyTarget {
    /// Exactly one open request, and the sender has been shown its prompt.
    Request(String),
    /// Nothing is waiting for this sender's reply right now.
    NotWaiting,
    /// Several inputs are open; a channel reply cannot choose between them.
    Ambiguous,
}

/// Field progress remains actor-local. Complete responses are retained with
/// immutable target/operation identities in the shared managed queue.
pub(super) struct ManagedChannelInputs {
    pub(super) client: Arc<RuntimeClient>,
    pub(super) conn: ConnectionManager,
    pub(super) scope: QueueScope,
    pub(super) prompted: HashSet<(String, String)>,
}
impl ManagedChannelInputs {
    /// Bind an arriving reply to the request it answers, before anything else
    /// can open. A reply is never retained without that binding: delivering it
    /// to whichever request is open later would answer a prompt the sender may
    /// never have seen.
    pub(super) async fn reply_target(&mut self, instance: &str) -> anyhow::Result<ReplyTarget> {
        let page = self
            .client
            .list_input_requests(self.scope.tenant_id(), &[instance.into()], 0, 2)
            .await?;
        if page.total_count > 1 {
            return Ok(ReplyTarget::Ambiguous);
        }
        Ok(match page.requests.into_iter().next() {
            Some(request)
                if self
                    .prompted
                    .contains(&(instance.to_owned(), request.request_id.clone())) =>
            {
                ReplyTarget::Request(request.request_id)
            }
            _ => ReplyTarget::NotWaiting,
        })
    }

    /// Called on every owned-execution poll, independently of historical events.
    pub(super) async fn poll(
        &mut self,
        instance: &str,
        channel: &Arc<dyn Channel>,
        conversation: &str,
        replies: &mut mpsc::Receiver<InboundMessage>,
    ) -> anyhow::Result<InputProgress> {
        let buffered = session_queue::peek_event(
            &mut self.conn,
            self.scope.tenant_id(),
            self.scope.session_id(),
            Some(instance),
        )
        .await?;
        if let Some(source) = buffered {
            // Replay a chosen target before consulting current discovery. The
            // request may have disappeared because the prior acceptance succeeded.
            if source.event.target.is_some() {
                self.handoff(&source).await?;
                return Ok(InputProgress::Retained);
            }
            let page = self
                .client
                .list_input_requests(self.scope.tenant_id(), &[instance.into()], 0, u32::MAX)
                .await?;
            let request = source.event.for_request.as_ref().and_then(|answered| {
                page.requests
                    .into_iter()
                    .find(|request| &request.request_id == answered)
            });
            let Some(request) = request else {
                // The request this reply answered closed (or the reply predates
                // request binding). Never re-aim it at a newer request.
                session_queue::acknowledge_event(&mut self.conn, &source).await?;
                return Ok(InputProgress::Undelivered);
            };
            if !is_structured(&request) {
                let bound = session_queue::bind_event(
                    &mut self.conn,
                    &source,
                    &InputTarget {
                        instance_id: instance.into(),
                        request_id: request.request_id.clone(),
                    },
                )
                .await?;
                self.handoff(&bound).await?;
                return Ok(InputProgress::Retained);
            }
            // An explicit reply to a structured wait (re)starts its collection;
            // the collector consumes this buffered reply as the first answer.
            return self
                .collect_progress(instance, &request, channel, conversation, replies)
                .await;
        }
        let page = self
            .client
            .list_input_requests(self.scope.tenant_id(), &[instance.into()], 0, 2)
            .await?;
        if page.total_count == 0 {
            return Ok(InputProgress::NoInput);
        }
        if page.total_count != 1 || page.requests.len() != 1 {
            return Ok(InputProgress::Ambiguous);
        }
        let request = page.requests.into_iter().next().expect("one request");
        let first_prompt = self
            .prompted
            .insert((instance.into(), request.request_id.clone()));
        if !is_structured(&request) {
            if first_prompt
                && let Err(error) = channel.send_text(conversation, prompt(&request)).await
            {
                self.prompted
                    .remove(&(instance.into(), request.request_id.clone()));
                return Err(error);
            }
            return Ok(InputProgress::Waiting);
        }
        // A cancelled/failed attempt is not restarted every poll. A subsequent
        // explicit reply can begin another attempt for the same still-open wait.
        if !first_prompt {
            return Ok(InputProgress::Waiting);
        }
        self.collect_progress(instance, &request, channel, conversation, replies)
            .await
    }

    async fn collect_progress(
        &mut self,
        instance: &str,
        request: &InputRequest,
        channel: &Arc<dyn Channel>,
        conversation: &str,
        replies: &mut mpsc::Receiver<InboundMessage>,
    ) -> anyhow::Result<InputProgress> {
        match self
            .collect(instance, request, channel, conversation, replies)
            .await
        {
            Ok(()) => Ok(InputProgress::Retained),
            Err(error) if error.is::<collector::CollectionStopped>() => Ok(InputProgress::Waiting),
            Err(error) => {
                self.prompted
                    .remove(&(instance.into(), request.request_id.clone()));
                Err(error)
            }
        }
    }

    async fn handoff(&mut self, source: &session_queue::PeekedEvent) -> managed::QueueResult<()> {
        let target = source
            .event
            .target
            .as_ref()
            .ok_or(managed::QueueError::Invalid)?;
        managed::enqueue_targeted(
            &mut self.conn,
            &self.scope,
            &source.event.message_id,
            &source.event.message_id,
            &source.event.payload,
            target,
        )
        .await?;
        session_queue::acknowledge_event(&mut self.conn, source).await
    }

    /// Settle one buffered reply of an execution that ended or is being left:
    /// a bound reply is handed to the managed queue (whose receipt replay still
    /// works after termination); an unbound one is reported undeliverable.
    /// Returns `true` once the buffer is empty. One message per call keeps
    /// terminal cleanup bounded.
    pub(super) async fn finish_instance(
        &mut self,
        instance: &str,
        channel: &Arc<dyn Channel>,
        conversation: &str,
    ) -> managed::QueueResult<bool> {
        let Some(source) = session_queue::peek_event(
            &mut self.conn,
            self.scope.tenant_id(),
            self.scope.session_id(),
            Some(instance),
        )
        .await?
        else {
            return Ok(true);
        };
        if source.event.target.is_some() {
            self.handoff(&source).await?;
        } else {
            session_queue::acknowledge_event(&mut self.conn, &source).await?;
            let _ = channel.send_text(conversation, UNDELIVERED_NOTICE).await;
        }
        Ok(false)
    }

    /// Drive this session's retained responses while idle. A response the
    /// workflow can no longer accept (its request closed, or it never had one)
    /// is failed explicitly and reported: a channel user has no delivery view
    /// to resolve it in, and leaving it blocked would stop the session.
    /// Returns `true` while a response is still in flight (deferred or leased).
    pub(super) async fn settle_queue(
        &mut self,
        channel: &Arc<dyn Channel>,
        conversation: &str,
    ) -> managed::QueueResult<bool> {
        match managed::deliver_to_instance(&mut self.conn, &self.scope, &self.client).await? {
            managed::DeliveryOutcome::Blocked(envelope) => {
                managed::fail_blocked(&mut self.conn, &self.scope, &envelope.message_id).await?;
                let _ = channel.send_text(conversation, UNDELIVERED_NOTICE).await;
                Ok(false)
            }
            managed::DeliveryOutcome::Deferred(_) | managed::DeliveryOutcome::Busy => Ok(true),
            managed::DeliveryOutcome::Idle | managed::DeliveryOutcome::Accepted(_) => Ok(false),
        }
    }

    async fn collect(
        &mut self,
        instance: &str,
        request: &InputRequest,
        channel: &Arc<dyn Channel>,
        conversation: &str,
        replies: &mut mpsc::Receiver<InboundMessage>,
    ) -> anyhow::Result<()> {
        let client = self.client.clone();
        let scope = self.scope.clone();
        let ensure_open = || async {
            let page = client
                .list_input_requests(scope.tenant_id(), &[instance.into()], 0, u32::MAX)
                .await?;
            anyhow::ensure!(
                page.requests
                    .iter()
                    .any(|current| current.request_id == request.request_id),
                "Input request is no longer active"
            );
            Ok(())
        };
        ensure_open().await?;
        channel.send_text(conversation, prompt(request)).await?;
        let payload = collector::collect_fields(
            request
                .spec
                .response_schema
                .as_ref()
                .expect("structured schema"),
            channel.as_ref(),
            conversation,
            replies,
            Some(collector::BufferedReplies {
                conn: &mut self.conn,
                scope: &scope,
                instance,
                request: &request.request_id,
            }),
            ensure_open,
        )
        .await?;
        self.retain_response(instance, &request.request_id, &payload)
            .await?;
        Ok(())
    }

    async fn retain_response(
        &mut self,
        instance: &str,
        request: &str,
        payload: &Value,
    ) -> managed::QueueResult<()> {
        let operation = Uuid::new_v4().to_string();
        let target = InputTarget {
            instance_id: instance.into(),
            request_id: request.into(),
        };
        loop {
            match managed::enqueue_targeted(
                &mut self.conn,
                &self.scope,
                &operation,
                &operation,
                payload,
                &target,
            )
            .await
            {
                Ok(_) => return Ok(()),
                Err(managed::QueueError::Backend(error)) => {
                    warn!(error = %error, "Collected response enqueue uncertain; retrying original operation");
                    sleep(Duration::from_secs(1)).await;
                }
                Err(error) => return Err(error),
            }
        }
    }
}
pub(super) const UNDELIVERED_NOTICE: &str =
    "Your reply was not delivered: the input it answered is no longer waiting.";

fn is_structured(request: &InputRequest) -> bool {
    request
        .spec
        .response_schema
        .as_ref()
        .is_some_and(|schema| !is_simple_schema(schema))
}

fn prompt(request: &InputRequest) -> &str {
    request
        .spec
        .metadata
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("Please reply to continue.")
}
