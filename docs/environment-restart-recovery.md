# Environment-restart recovery

When the Runtara Environment restarts (a release, a worker rolling restart, an
OOM, a crash) while a guest workflow is running, the in-process guest dies with
it. Runtara recovers such instances automatically instead of losing them.

## What happens

- **Graceful shutdown (SIGTERM).** The server stops accepting new external
  work, but keeps the internal API and core serving while it *drains*: it
  signals every running guest to shut down. Each guest observes the signal at
  its next checkpoint boundary and **suspends cleanly** (`status=suspended`,
  `termination_reason=shutdown_requested`), recording a checkpoint. Guests that
  don't reach a checkpoint within the grace window are force-stopped and also
  marked suspended. On restart the wake scheduler relaunches every suspended
  instance.
- **Abrupt shutdown (SIGKILL / crash / OOM).** No drain runs; instances are
  left `running` in core. On the next startup, orphan recovery finds them
  (process gone, core shows running) and routes them into the same
  suspend → wake → relaunch path, with `termination_reason=environment_restart`.

Either way the instance is **relaunched and replayed from the start**. The
engine is replay-from-start with checkpoints as a result cache, so every
*completed* durable step is served from cache (exactly-once) and only the
remaining work re-executes.

## Idempotency — what authors must know

- **Completed durable steps are exactly-once on replay** (served from the
  checkpoint cache). You don't need to do anything for these.
- **The step that was in flight when the restart hit is at-least-once** — its
  side effect may run again on replay. This is the same guarantee the graceful
  drain has always had. For steps with external side effects (creating rows,
  calling third-party APIs), make that step idempotent — e.g. use
  `object_model:create-if-not-exists` (keyed on a business key) instead of
  `create-instance`, or an upsert/dedupe key on the external call.

## Crash-loop protection

Auto-recovery is bounded so a "poison" instance that crashes the Environment on
startup can't loop forever:

- `RUNTARA_MAX_AUTO_RESTARTS` (default `5`) caps **consecutive no-progress**
  relaunches. The counter resets whenever the instance's checkpoint count
  advances between recoveries, so a genuinely long-running workflow that keeps
  making progress recovers across any number of restarts; only an instance that
  crashes *before* making new progress is bounded.
- When the cap is exceeded the instance fails terminally with
  `termination_reason=environment_restart` and the error
  `"Killed by Environment restart; exceeded automatic restart limit (N)"`.

## Operator controls

| Setting | Default | Effect |
|---|---|---|
| `RUNTARA_AUTO_RECOVER` | `true` | Set `false`/`0` to disable auto-recovery for the whole Environment. Restart-killed instances then fail terminally (`environment_restart`) instead of relaunching. |
| `RUNTARA_MAX_AUTO_RESTARTS` | `5` | Max consecutive no-progress auto-restarts before terminal failure. |
| `RUNTARA_SHUTDOWN_GRACE_MS` | `60000` | How long the graceful drain waits for in-flight guests to reach a checkpoint and suspend before force-stopping them. |

### Deployment: give the drain enough time

For a *clean* graceful shutdown the orchestrator must send SIGTERM and wait at
least `RUNTARA_SHUTDOWN_GRACE_MS` before SIGKILL. A too-short stop timeout kills
the server mid-drain and falls back to abrupt-restart recovery (still correct,
just not a clean suspend, and slower to converge).

- **Docker / Compose**: the image sets `STOPSIGNAL SIGTERM`; set
  `stop_grace_period` ≥ `RUNTARA_SHUTDOWN_GRACE_MS` (the bundled
  `docker-compose.yml` uses a 30s drain + 35s grace).
- **Kubernetes**: set `terminationGracePeriodSeconds` ≥
  `RUNTARA_SHUTDOWN_GRACE_MS / 1000`.

## Observing it

- A recovered instance ends `completed` like any other run; the recovery is
  visible in the Environment startup log (`recovered` / `failed` counts) and in
  the per-instance `recovery_attempts`.
- An instance that exhausted the cap (or had auto-recovery disabled) ends
  `failed` with the error string above — this surfaces in the execution's
  `error` field, so operators can distinguish an infra restart from an
  application bug.

## Roadmap

A per-workflow `autoRecover` opt-out (for workflows that must never replay) is
planned; today the control is the Environment-wide `RUNTARA_AUTO_RECOVER`.
