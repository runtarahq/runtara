# More than one engine: a plan

Status: built ([As built](#as-built)).

The engine is one process today (`docs/roles.md`: "The engine is one
process"). More than one API may stand in front of it, and executors run its
workflows, but the engine itself is a single point of failure: when it
stops, nothing new starts and nothing is answered until it is back. This
plan lets several engines share one deployment (one server database, one
runtime database, one Valkey), so that one can stop while the others carry
on, and so that the engine can scale out.

It is a plan for what is not shared yet. Most of the engine already is.

## What is already shared

The durable runtime was built for peers on one database. An audit of every
loop the engine starts found these safe with several engines:

- **Runs and launches** are owned by a lease with an owner and an expiry
  (`instance_launches.lease_owner`, `lease_expires_at`), the owner a
  `launch-dispatcher-<uuid>` made at boot. Claims take `FOR UPDATE SKIP
  LOCKED`; recovery touches only a run whose owner's lease expired
  (`recover_orphaned_containers`, `recover_expired_leases`); the heartbeat
  monitor checks the lease before calling a run dead ("Absence from this
  process is not proof of owner death").
- **Stopping a run, and signals**, are written to the database
  (`request_abort`, `insert_signal`); the owner delivers them.
- **Sleeping runs** are woken by a `SKIP LOCKED` claim with a lease
  (`wake_scheduler`).
- **The execution outbox** is claimed with `SKIP LOCKED` and a lease fenced
  on its owner.
- **Triggers** are read from a Valkey consumer group with a consumer per
  worker, handed to a launch under a lease.
- **Cron** enqueues each tick under `cron:<trigger>:<scheduled_at>`, unique
  per tenant and taken under an advisory lock: two engines enqueue it once.
- **Conversations are carried** under `FOR UPDATE SKIP LOCKED` (`advance`),
  and every engine hears every run notice (`pg_notify('runtara_run', …)`).
- **Approvals, OAuth flow state, synchronous runs, compile waits, tenant
  request quotas** are in the database or Valkey already.
- **Migrations** take sqlx's advisory lock.

## What is not

Ordered by what breaks:

1. **The admission counter is seeded and corrected as if one process owned
   it** (`workers/admission_counter.rs`). Each engine `SET`s the tenant's
   counter from the durable reservations when it starts, losing a peer's
   increment in flight, and each applies the same correction (`DECRBY`)
   every pass. The counter falls below the truth, and a tenant is admitted
   past `MAX_CONCURRENT_EXECUTIONS`.
2. **A compile in progress has no owner** (`valkey/compilation_queue.rs`).
   An item is popped (`BLPOP`) and stays in `:pending` until it is done. An
   engine that starts re-queues every pending item not in the queue
   (`recover_orphaned`): a peer's compile in progress is compiled again. An
   engine that dies mid-compile and does not come back leaves its item in
   `:pending` for good, and the version is never compiled: a later request
   for it is taken as a duplicate.
3. **A conversation's readers, and the replies being written, are in one
   engine's memory** (`conversations.rs`, `Wakes`: "It is this process's
   alone. The engine is one process"). Three maps:
   - `readers`: who is waiting to read a conversation. An engine that
     advances a conversation wakes only its own readers; a reader on
     another waits out its 25 seconds. Nothing is lost (the next ask reads
     the stored rows), but a reply is late by up to 25 seconds.
   - `runs`: which conversation each run carries, rebuilt every 5 seconds
     from the 500 oldest unsettled conversations. A run streaming a reply
     through an engine that has not learnt its conversation is refused
     (`piece_of_reply` → `NotFound`), for 5 seconds or, past 500, for good.
   - `deltas`: the pieces of replies being written, numbered by a counter of
     the engine's own. A reader on another engine never sees them, and a
     cursor from one engine is meaningless on another.
4. **Executors report to one engine** (`runtara-runner-executor/src/
   registry.rs`, in memory; `RUNTARA_EXECUTOR_SERVER_URL`). An engine no
   executor reports to cannot place a run, a compile or a validation.
5. **The API and intake know one engine** (`RUNTARA_ENGINE_URL`).
6. **Smaller things in one engine's memory or disk:**
   - workflow agents are staged on the engine's disk at boot
     (`workflow_agents.rs`): one published on engine A is not seen by B
     until B restarts;
   - an OAuth refresh is single-flight per process (`token_cache.rs`,
     `REFRESH_LOCKS`): two engines can refresh one connection at once, and
     a provider that rotates refresh tokens refuses the loser
     (`invalid_grant`, taken as permanent);
   - `in_process_waits` ("One process owns its tenant's runs, so a
     process-local record is complete") shows `waiting_in_process` only
     through the run's owner;
   - artifacts are on the engine's disk unless `RUNTARA_ARTIFACT_STORE_URL`
     is set: an image compiled by one engine is missing on another;
   - the trigger worker's consumers are never removed from the group, and
     every consumer's autoclaim scans the others' pending entries
     (`server.rs`, the comment by the trigger workers).
7. **Work every engine does again, harmlessly:** cleanups (invocations,
   trigger streams, the runtime database, images), the unconfirmed OAuth
   sweep (a provider is asked to revoke twice), the control-children
   publisher, artifact restaging at boot.

## What stays with one engine, on purpose

**A run is its owner's.** The engine whose dispatcher holds a run's launch
lease serves that run's workflow API: the run channel's credentials, its
launch slot, its receipts and its fetchable artifact are in that engine's
memory (`run_channel.rs`, `api_launches.rs`, `MemoryReceiptStore`), and an
executor relays the run's calls to that engine's own
`RUNTARA_RUN_CHANNEL_ENDPOINT`. This stays. Everything that crosses to a run
already goes through the database to its owner (signals, stops, aborts), and
when an owner dies its lease expires and another engine launches the run
again from its last checkpoint. Sharing a live run between engines would buy
nothing that relaunching does not, and would put a run's credentials where
other engines could use them.

So the rule is: **a run is pinned to its owner; everything else is shared.**
Each engine has its own run-channel endpoint, which is never behind a load
balancer.

## Where shared state goes

| State | Where | Why |
|---|---|---|
| Who the engines are | **Database** (runtime), `engine_members` | Seen by operators, outlives Valkey, checked at boot |
| Which engine runs a job no two should run at once | **Valkey** lease (`SET NX PX`, renewed) | Short-lived; correctness still rests on the database guards each job has |
| The admission counter | **Valkey**, as now, corrected by one engine | It is Valkey's already; only its owner changes |
| A compile in progress | **Valkey**, a lease per item | The queue is Valkey's |
| "A conversation has something new" | **Database** `NOTIFY`, in the transaction that stored it | Sent only if the row committed; no lost wake between commit and notify |
| Which conversation a run carries | **Database** (`conversations.instance_id`), cached | It is there already; the cache only saves a query |
| Pieces of a reply being written | **Valkey**, a stream per conversation | Many small writes, short-lived, never durable: the stored reply replaces them |
| Workflow agents published | **Database** `NOTIFY` on publish, plus a periodic sync from the artifact store | The store is the truth; disks are caches |
| An OAuth refresh in flight | **Valkey** lock per connection | A lock across engines; the token itself is never put in Valkey |
| A run waiting in process | **Database**, on the run's launch row | Shown by any engine |

Nothing secret moves: no token, key or credential goes to Valkey or the
database that is not there today.

## The parts

| | What | Tests that say it is done |
|---|---|---|
| E1 | **Engines know each other.** Each engine has an id made at boot (the uuid its lease owners already carry) and a row in `engine_members` (runtime database: id, release, run-channel endpoint, started, last seen), renewed every 10 seconds and removed on shutdown. A helper, `singleton(name)`, holds a Valkey lease (`runtara:singleton:<name>`, owner the engine id) while a job runs, renewed by its holder, taken over when it expires. An engine refuses to start beside a live peer when its artifacts are on its own disk (no `RUNTARA_ARTIFACT_STORE_URL`), or when the peer runs another release (unless told it is a rolling upgrade). | Two engines each see the other's row; one's row goes when it stops, and after its lease when it is killed. A singleton job runs on one engine at a time and moves when that engine is killed. A second engine with a local artifact store refuses to start, naming why. |
| E2 | **The admission counter has one keeper.** Seeding and the correction pass run under `singleton("admission-counter")`. Seeding becomes "set if absent" (`SET NX`) on start; the keeper reseeds when it takes the lease. The correction is applied by the keeper only. | Two engines start together under load and the counter equals the durable reservations; a tenant at its limit is held to it with runs started through both engines. |
| E3 | **A compile has an owner.** A worker takes an item with a script that pops it from the queue and adds it, in the same step, to `<tenant>:compilation:inflight` (a sorted set scored by its lease's expiry, renewed while compiling), instead of `BLPOP`. Recovery re-queues only items whose lease expired; boot no longer re-queues everything pending. | An engine killed mid-compile: its item is compiled by another within a lease. A peer starting while a compile runs does not compile it again. |
| E4 | **Conversations are read from any engine.** (a) The transaction that stores a conversation's event sends `pg_notify('runtara_conversation', '<id>')`; every engine wakes its own readers of that conversation. (b) `piece_of_reply` and `carry` look up a run's conversation in the database when the cache does not have it, so no run is refused, and the 500-row rescan is no longer needed for correctness. (c) Pieces of replies go to a Valkey stream per conversation (`<tenant>:conversation:<id>:pieces`, capped at 512, expiring an hour after the last piece), numbered by an `INCR` on the conversation's counter, so `n` means the same on every engine. A reader waits on its rows' wake and on the stream at once. A stored reply marks its pieces written in the stream; readers drop them as they do today. | A reader on engine B sees a reply streamed by a run whose conversation engine A carries, piece by piece and in order, and then the stored reply; with no wait near 25 seconds. A run streams through an engine that never carried its conversation. Pieces resume after `n` across engines. |
| E5 | **Executors report to every engine.** `RUNTARA_EXECUTOR_SERVER_URL` may name a host that resolves to several engines (a headless service, Cloud Map) or list several; the executor resolves it on each heartbeat and registers with each address. Placement is unchanged: an executor over its runs refuses, and the engine offers the run elsewhere. Engine certificates carry the shared name. A run's channel still points at its placing engine's own endpoint. | Two engines place runs, compiles and validations on the same two executors; an executor's capacity is not exceeded; an engine killed with runs on an executor: the runs end, their launches expire and run again on the other engine, and no effect is applied twice (see "Open questions"). |
| E6 | **The API and intake reach any engine.** `RUNTARA_ENGINE_URL` may be a load-balanced address. Nothing they ask depends on which engine answers after E4. The API warns when the engines' facts (release, compiler, trusted pins) differ. | The API's and intake's operations, held conversation reads included, pass with each request to a random engine. |
| E7 | **The rest of what is per engine.** (a) Workflow agents: a publish or removal sends `pg_notify('runtara_workflow_agents', tenant)`, and each engine restages that tenant from the artifact store; each also resyncs every 5 minutes. (b) An OAuth refresh takes a Valkey lock per connection (`<tenant>:oauth-refresh:<connection>`, 30 s) before it asks the provider, and re-reads the connection after waiting. (c) `waiting_in_process` is written to the launch row by the owner. (d) The trigger group's dead consumers (no pending, idle past an hour) are removed under `singleton("trigger-consumers")`. (e) Cleanups, the unconfirmed OAuth sweep, artifact restaging and the legacy artifact import run under a singleton each. | An agent published through one engine is used by a compile on another within seconds. Two engines refreshing one rotating-token connection at once: both succeed, the provider is asked once. Each cleanup runs on one engine at a time. |
| E8 | **Two engines, end to end.** `e2e/test_two_engines.sh`: two engines, an API and intake asking either, two executors registered with both. A compose overlay, `docker-compose.engines.yml`, runs two. `docs/roles.md`, `docs/executor.md` and `docs/separating-concerns.md` say what holds with several. | The script passes: runs, cron, compiles, conversations streamed across engines, a tenant's admission limit, and one engine killed mid-run and mid-compile, with no run lost or doubled. |

E1 to E3 are right for one engine too and can ship first. E4 is the most
work. E5 needs a change to the executor, which is released with the server.

## Limits that stay per engine

These are bounds of one process and stay so; the docs say they multiply
with engines:

- `RUNTARA_TENANT_MAX_CONCURRENT_REQUESTS` (`tenant_concurrency.rs`): its
  request slots. The tenant's Valkey quota (`tenant_quota.rs`) is shared.
- Compile, validation and agent-test concurrency; the agent-test rate.
- Pools to a tenant's object model database: N engines, N pools.
- Caches with a short life: tenant status (5 s), image rows (5 s), platform
  OAuth apps (60 s), object model metadata (60 s). A change is seen by every
  engine within that time.
- A run's stderr log is on its owner's disk.

## Decisions

1. **A run is pinned to its owner** (above). Failover is relaunching from
   the checkpoint, which the leases already do.
2. **Database for what is durable or must be seen with the row; Valkey for
   what is short-lived or high-rate.** A wake is sent by the transaction
   that stores what it announces (`NOTIFY`), so a reader is never told
   before the row is there and never missed after. Pieces of a reply are
   many and die when the reply is stored: Valkey.
3. **Leases, not leaders.** No engine is special. A job that must run once
   at a time holds a lease and loses it by not renewing; every other
   engine's work is claimed row by row as today.
4. **Engines run one release.** A rolling upgrade has two for a while; the
   check in E1 is relaxed for it explicitly, and the wire between releases
   is held to what it is today.
5. **No sticky sessions.** The API may send any request to any engine; a
   held read moves between engines from one ask to the next.

## Open questions

- **A run whose owner died but whose guest still runs.** An executor keeps
  running a guest after its placing engine is gone; its calls to that
  engine's run channel fail, so it stops at its next call. Meanwhile the
  launch lease expires and another engine launches the run again. Check
  that the old launch can write nothing after the new one starts (its
  checkpoints and events are fenced by the launch generation), and add the
  test to E5.
- **Valkey ACLs.** E2 to E7 add keys the engine's Valkey user must be
  granted (`scripts/roles/`, copied in `docker-compose.roles.yml`); the
  API's and intake's users are given none of them.
- **Valkey pub/sub instead of `NOTIFY`** for conversation wakes if a
  deployment's database is behind a pooler in transaction mode, which
  drops `LISTEN`. The runtime notices have the same constraint today.

## As built

| Part | Where | Tests |
|---|---|---|
| E1 | `peers::members` (`engine_members`, runtime migration `20261119400000`), `peers::singleton` | `tests/peers.rs` |
| E2 | `workers/admission_counter.rs`: a keeper per tenant (`admission-counter:<tenant>`) | `tests/admission_keeper.rs` |
| E3 | `valkey/compilation_queue.rs` (`<tenant>:compilation:inflight`), the worker renews | `tests/shared_valkey.rs` |
| E4 | `conversations.rs`: the `conversation_said` trigger (server migration `20261119600000`), `hear_conversations`, `hear_pieces`, pieces in Valkey | `tests/conversations.rs` |
| E5 | `runtara-executor`'s `Registrars` | `runtara-executor/tests/executor.rs` |
| E6 | `engine/remote.rs` (`Engines`), `release` in the engine's facts | `engine::remote` tests |
| E7 | (a) `workflow_agents::keep_in_sync`; (b) `runtara-connections`' `auth::refresh_lock`; (c) `in_process_waits::share` (runtime migration `20261119500000`); (d) `trigger_worker::keep_consumers_tidy`; (e) `peers::singleton::Job` | `workflow_agents` tests, `tests/sealed_oauth_flow.rs`, `tests/peers.rs`, `tests/shared_valkey.rs` |
| E8 | `docker-compose.engines.yml`, `e2e/test_two_engines.sh`, `docs/roles.md` | the script, against the stack with two engines |

Departures and details:

- **One release, checked at start.** Engines that start at the same moment
  may each miss the other in the check; they run one image in practice.
- **The API spreads requests itself.** HTTP/2 holds one connection to one
  address, so a name that resolves to several engines is looked up every
  10 seconds and each request goes to one of them; one that cannot be
  connected to is passed over. A request already sent to an engine that
  dies fails, as for one engine.
- **The compose overlay scales the engine service.** Every replica answers
  to `runtara`, which executors and the API resolve to each; each works out
  its own run-channel endpoint and data directory, and they share artifacts
  in a `file://` store on the data volume.
- **A refresh re-reads the connection.** A lock alone would not do: an
  engine that waited would refresh with the token it read before, which a
  rotating provider has voided. It reads the connection again under the
  lock and uses what the other stored.
- **Artifact restaging at boot stays each engine's.** It restages only
  what is missing, once.

On the open question of a superseded launch: the e2e kills the engine that
runs a durable run; the run is launched again by the other engine from its
checkpoint and completes once (two launch rows, one completion). A run whose
steps have effects outside runtara between checkpoints may repeat them, as
it would after any crash; that is the checkpointing model, not new here.

Measured locally: `e2e/test_two_engines.sh` passes against the evaluation
stack with two engines and two executors.

## Not here

- Splitting one tenant's runs across engines by tenant (sharding). Every
  engine takes every tenant's work.
- Moving a live run between engines.
- Multi-region.
