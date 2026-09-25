use super::*;
use runtara_core::persistence::inputs::InputRequest;
use std::collections::HashSet;

#[derive(Debug, PartialEq, Eq)]
pub(super) enum InputProgress {
    NoInput,
    Waiting,
    Ambiguous,
    Retained,
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
        )
        .await?;
        if let Some(source) = &buffered {
            // Replay a chosen target before consulting current discovery. The
            // request may have disappeared because the prior acceptance succeeded.
            if source.event.target.is_some() {
                self.handoff(source).await?;
                return Ok(InputProgress::Retained);
            }
            anyhow::ensure!(
                source.event.instance_id.as_deref() == Some(instance),
                "Buffered message belongs to another execution or startup"
            );
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
        let structured = request
            .spec
            .response_schema
            .as_ref()
            .is_some_and(|schema| !is_simple_schema(schema));
        if !structured {
            if let Some(source) = buffered {
                let bound = session_queue::bind_event(
                    &mut self.conn,
                    self.scope.tenant_id(),
                    self.scope.session_id(),
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
            if self
                .prompted
                .insert((instance.into(), request.request_id.clone()))
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
        let first_prompt = self
            .prompted
            .insert((instance.into(), request.request_id.clone()));
        if !first_prompt && buffered.is_none() {
            return Ok(InputProgress::Waiting);
        }
        match self
            .collect(instance, &request, channel, conversation, replies)
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
        session_queue::acknowledge_event(
            &mut self.conn,
            self.scope.tenant_id(),
            self.scope.session_id(),
            source,
        )
        .await
    }

    /// On root termination, preserve replies received during that execution as
    /// responses requiring resolution. They must not become the next startup.
    /// One message per tick keeps terminal cleanup bounded.
    pub(super) async fn finish_instance(&mut self, instance: &str) -> managed::QueueResult<bool> {
        let Some(source) = session_queue::peek_event(
            &mut self.conn,
            self.scope.tenant_id(),
            self.scope.session_id(),
        )
        .await?
        else {
            return Ok(true);
        };
        if source.event.instance_id.is_none() {
            return Ok(true);
        }
        if source.event.instance_id.as_deref() != Some(instance) {
            return Err(managed::QueueError::Conflict);
        }
        if source.event.target.is_some() {
            self.handoff(&source).await?;
        } else {
            managed::enqueue(
                &mut self.conn,
                &self.scope,
                &source.event.message_id,
                &source.event.message_id,
                &source.event.payload,
            )
            .await?;
            session_queue::acknowledge_event(
                &mut self.conn,
                self.scope.tenant_id(),
                self.scope.session_id(),
                &source,
            )
            .await?;
        }
        Ok(false)
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
fn prompt(request: &InputRequest) -> &str {
    request
        .spec
        .metadata
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("Please reply to continue.")
}
