# Instance-input reliability follow-ups

Deferred on 2026-09-25 at the user's request to re-center the current fix.
These are separate work proposals, not release requirements for
[stale pending-input correctness](instance-events-fix.md). Do not implement them
as part of that fix solely because some foundation code already exists.

## Provider intake durability

Persist authenticated provider intake and deduplication before acknowledgement,
with stable routing across configuration changes. Cover Slack, Teams, Telegram,
and Mailgun at the HTTP boundary. Verify inbound retry/deadline guarantees from
official documentation; do not infer them from outbound API guidance.

The previous design proposes a connection-scoped inbox and idempotent handoff to
a session queue, retaining destination deduplication while unresolved intake can
retry. This needs a separate architecture review and explicit retention policy.
It is more than replacing event-based pending-input discovery in channels.

## Startup handoff and launch recovery

Separate startup input from managed response delivery, with frozen launch inputs,
pinned version, one launch identity, and a distinct source-handoff outcome. Test
lost source acknowledgements and ensure startup is never delivered again to the
first wait. Complete the source/worker/compiled-workflow handoff as its own change.

Extracted draft work includes `StartupIntent`, `DeliveryMode`,
`HandedOff`, startup branches in `managed.lua`,
`api/services/session_queue/delivery/startup.rs`, and associated delivery status
fields. This draft is not verified or approved for release. Do not finish its
contracts and tests merely to declare the narrower fix complete. Replacement-run
launch intents, route snapshots and launch-recovery tests are extracted as well.
Shared response binding, receipt recovery and read-only initial-source observation
remain in the active fix.

## Restartable structured collectors

Persist field progress, schema/target, processed reply identities, validation
attempts, prompt intents, and the final response operation. Prove restart between
fields and lost final acknowledgement without retaining an actor. Expose
consumed-for-collection outcomes separately from accepted workflow responses.

The current fix still must validate target state and final submission, retain an
uncertain submitted response, and avoid responding on collection cancellation.
It does not promise continuation of an unfinished conversation after process loss.

## Delivery operations and diagnostics

Consider channel owner dashboards, provider-specific resolution, generalized
worker fairness/load tests, expanded completed-message retention policies, and
exactly-once optional diagnostic events separately. Tests required to establish
safe behavior of retained response delivery still belong to the current fix.
Do not defer an actual data-loss, ownership, or stale-target regression behind
this category.

Control-agent implementation and MinIO replacement are separate tasks as well.

## Preservation and separation

The local branch `codex/instance-input-delivery-followup` is checked out at
`/private/tmp/runtara-input-delivery-followup`. It is stacked on a snapshot of the
unfinished correctness fix because the delivery draft depends on new request and
queue types that are not on the original branch's HEAD yet. Prerequisite commit `71b32eae` is the base; top commit `509e2600` restores the
pre-extraction deferred draft and includes `docs/instance-events-delivery-branch.md`
with resumption instructions. The follow-up branch is not pushed. At extraction,
the active worktree and its index were left uncommitted; the correctness fix is
now prepared separately for PR review.

Resume by reviewing the top commit against its parent, then porting only the
needed follow-up changes onto the completed correctness fix. Do not merge the
prerequisite snapshot as if it were a verified release, or blindly cherry-pick
replacement-launch behavior: stale managed replies still must not launch a new
execution. The preserved draft needs design and verification work before use.

The active queue no longer has startup mode or replacement-launch transitions.
Its focused tests cover terminal targets, ambiguity, initial registration delay,
and receipt replay. Unrelated root `Cargo.toml` build overrides and local
`docs/control-agent.md` / `docs/report-demo.html` are excluded from the branch.

The complete prior checkpoint and expanded plan are preserved in
[historical implementation notes](instance-events-implementation-notes.md).
Those notes provide implementation context, not another active checklist.
