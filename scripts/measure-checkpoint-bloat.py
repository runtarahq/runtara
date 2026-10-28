#!/usr/bin/env python3
"""Measure dead tuples, size and WAL on `checkpoints` under scope collapse.

Runs `A -> Split[N] -> B` repeatedly against a runtara-server backed by its
own throwaway Postgres and Valkey containers, and samples the `checkpoints`
table during and between runs. Every Agent result inserts a row, and the
Split's result deletes its iterations in the same transaction, so each run
inserts N + 3 rows and leaves N dead tuples behind for autovacuum.

Each mode starts from an empty table, optionally seeds it with live rows (the
autovacuum trigger is a fraction of the table, so a near-empty table vacuums
far more eagerly than a production-sized one), applies its per-table storage
parameters, and runs the workflow back to back:

- empty:  no seed, server defaults
- seeded: seeded, server defaults
- paused: seeded, autovacuum off for `checkpoints` and its TOAST table
- tuned:  seeded, the `--tuned` storage parameters
- toast:  seeded, server defaults, items large enough that each iteration's
          row is stored out of line

Per run it records duration, `pg_stat_user_tables` counters, relation sizes,
the WAL written from the run's start to the next run's start (total, and the
records that touch `checkpoints`, its indexes or its TOAST table, read with
pg_walinspect), and the statements on `checkpoints` from
pg_stat_statements. A sampler polls the counters every `--sample-interval`
seconds, and each mode ends with `--tail-seconds` of sampling with no runs,
so autovacuum's catch-up is visible.

The containers are separate so the WAL position, which is cluster-wide,
counts only this workload, and so no shared database has its autovacuum
settings changed. `wal_keep_size` only keeps the WAL readable until it is
inspected; it does not change what is written.

Stdlib only. Requires docker, a built `runtara-server` and staged agent
components (`scripts/build-agent-components.sh`).
"""

from __future__ import annotations

import argparse
import base64
import json
import os
import platform
import random
import signal
import socket
import subprocess
import sys
import threading
import time
from pathlib import Path
from typing import Any
from urllib import error as urllib_error
from urllib import request as urllib_request


REPO_ROOT = Path(__file__).resolve().parents[1]
DEFAULT_OUTPUT_DIR = REPO_ROOT / ".data" / "checkpoint-bloat"
PG_USER = "runtara"
PG_PASSWORD = "runtara"
OBJECTS_ROLE = "runtara_objects"
TENANT = "checkpoint-bloat"

MODES = ("empty", "seeded", "paused", "tuned", "toast")

# Storage parameters a mode may set on `checkpoints`. Every mode resets all of
# them (and their `toast.` forms) first, so modes never inherit settings.
STORAGE_PARAMETERS = (
    "autovacuum_enabled",
    "autovacuum_vacuum_threshold",
    "autovacuum_vacuum_scale_factor",
    "autovacuum_vacuum_insert_threshold",
    "autovacuum_vacuum_insert_scale_factor",
    "autovacuum_vacuum_cost_limit",
    "autovacuum_vacuum_cost_delay",
    "autovacuum_analyze_threshold",
    "autovacuum_analyze_scale_factor",
)

POSTGRES_SETTINGS = (
    "autovacuum",
    "autovacuum_naptime",
    "autovacuum_max_workers",
    "autovacuum_vacuum_threshold",
    "autovacuum_vacuum_scale_factor",
    "autovacuum_vacuum_insert_threshold",
    "autovacuum_vacuum_insert_scale_factor",
    "autovacuum_vacuum_max_threshold",
    "autovacuum_vacuum_cost_limit",
    "autovacuum_vacuum_cost_delay",
    "vacuum_cost_limit",
    "vacuum_freeze_min_age",
    "vacuum_max_eager_freeze_failure_rate",
    "data_checksums",
    "wal_log_hints",
    "shared_buffers",
    "checkpoint_timeout",
    "max_wal_size",
    "full_page_writes",
    "wal_compression",
    "synchronous_commit",
)

STATS_SQL = """
SELECT json_build_object(
    'at', extract(epoch FROM clock_timestamp()),
    'lsn', pg_current_wal_lsn(),
    'n_live_tup', s.n_live_tup,
    'n_dead_tup', s.n_dead_tup,
    'n_tup_ins', s.n_tup_ins,
    'n_tup_del', s.n_tup_del,
    'n_ins_since_vacuum', s.n_ins_since_vacuum,
    'autovacuum_count', s.autovacuum_count,
    'vacuum_count', s.vacuum_count,
    'autoanalyze_count', s.autoanalyze_count,
    'last_autovacuum', s.last_autovacuum,
    'reltuples', c.reltuples,
    'relpages', c.relpages,
    -- Postgres 18 scales the insert trigger by the pages not all-frozen.
    -- Read through to_jsonb, it is NULL on a server without the column.
    'relallfrozen', (to_jsonb(c) ->> 'relallfrozen')::bigint,
    'toast_n_live_tup', t.n_live_tup,
    'toast_n_dead_tup', t.n_dead_tup,
    'toast_autovacuum_count', t.autovacuum_count,
    'heap_bytes', pg_relation_size(c.oid),
    'toast_bytes', CASE WHEN c.reltoastrelid = 0 THEN 0
                        ELSE pg_total_relation_size(c.reltoastrelid) END,
    'index_bytes', pg_indexes_size(c.oid),
    'total_bytes', pg_total_relation_size(c.oid))
FROM pg_class c
JOIN pg_stat_user_tables s ON s.relid = c.oid
LEFT JOIN pg_stat_all_tables t ON t.relid = c.reltoastrelid
WHERE c.oid = 'checkpoints'::regclass
"""

# The WAL records between two positions that touch `checkpoints`: its heap,
# its indexes, its TOAST table and that table's index. A record is counted
# once, under the relation of its first block. `by_relation` puts the same
# window's records under every relation, for scale; records that touch no
# block (commits, checkpoints) are in neither.
WAL_SQL = """
WITH all_blocks AS (
    SELECT * FROM pg_get_wal_block_info('{start}', '{end}', false)
),
rels AS (
    SELECT pg_relation_filenode(c.oid) AS node, 'heap' AS kind
    FROM pg_class c WHERE c.oid = 'checkpoints'::regclass
    UNION ALL
    SELECT pg_relation_filenode(i.indexrelid), 'index'
    FROM pg_index i WHERE i.indrelid = 'checkpoints'::regclass
    UNION ALL
    SELECT pg_relation_filenode(c.reltoastrelid), 'toast'
    FROM pg_class c WHERE c.oid = 'checkpoints'::regclass AND c.reltoastrelid <> 0
    UNION ALL
    SELECT pg_relation_filenode(i.indexrelid), 'toast'
    FROM pg_class c JOIN pg_index i ON i.indrelid = c.reltoastrelid
    WHERE c.oid = 'checkpoints'::regclass
),
blocks AS (
    SELECT b.*, r.kind
    FROM all_blocks b
    JOIN rels r ON r.node = b.relfilenode
    WHERE b.reldatabase = (SELECT oid FROM pg_database WHERE datname = current_database())
),
records AS (
    SELECT DISTINCT ON (start_lsn) start_lsn, kind, resource_manager, record_type, record_length
    FROM blocks ORDER BY start_lsn, block_id
),
all_records AS (
    SELECT DISTINCT ON (start_lsn) start_lsn, reldatabase, reltablespace, relfilenode, record_length
    FROM all_blocks ORDER BY start_lsn, block_id
),
named AS (
    SELECT CASE WHEN reldatabase <> (SELECT oid FROM pg_database WHERE datname = current_database())
                THEN 'another database'
                ELSE coalesce(pg_filenode_relation(
                         CASE WHEN reltablespace = (SELECT oid FROM pg_tablespace WHERE spcname = 'pg_default')
                              THEN 0 ELSE reltablespace END, relfilenode)::text,
                     'filenode ' || relfilenode) END AS relation,
           record_length
    FROM all_records
)
SELECT json_build_object(
    'by_relation', (SELECT coalesce(json_agg(x ORDER BY x.bytes DESC), '[]'::json) FROM (
        SELECT relation, count(*) AS records, sum(record_length) AS bytes
        FROM named GROUP BY 1 ORDER BY 3 DESC LIMIT 12) x),
    'total_bytes', pg_wal_lsn_diff('{end}', '{start}')::bigint,
    'checkpoints_bytes', (SELECT coalesce(sum(record_length), 0) FROM records),
    'checkpoints_records', (SELECT count(*) FROM records),
    'checkpoints_fpi_bytes', (SELECT coalesce(sum(block_fpi_length), 0) FROM blocks),
    'breakdown', (SELECT coalesce(json_agg(x ORDER BY x.bytes DESC), '[]'::json) FROM (
        SELECT kind, resource_manager || '/' || record_type AS record,
               count(*) AS records, sum(record_length) AS bytes
        FROM records GROUP BY 1, 2) x))
"""

STATEMENTS_SQL = """
SELECT coalesce(json_agg(x ORDER BY x.total_ms DESC), '[]'::json) FROM (
    SELECT left(regexp_replace(query, '\\s+', ' ', 'g'), 160) AS query, calls, rows,
           round(total_exec_time::numeric, 2) AS total_ms,
           round(mean_exec_time::numeric, 4) AS mean_ms,
           round(max_exec_time::numeric, 2) AS max_ms
    FROM pg_stat_statements
    WHERE dbid = (SELECT oid FROM pg_database WHERE datname = current_database())
      AND query ILIKE '%checkpoints%'
    ORDER BY total_exec_time DESC LIMIT 6) x
"""

TUPLE_SQL = """
SELECT json_build_object(
    'table', (SELECT row_to_json(t) FROM pgstattuple('checkpoints') t),
    'indexes', (SELECT json_object_agg(i.indexrelid::regclass::text, (
        SELECT json_build_object('leaf_pages', s.leaf_pages, 'deleted_pages', s.deleted_pages,
                                 'avg_leaf_density', s.avg_leaf_density,
                                 'index_size', s.index_size)
        FROM pgstatindex(i.indexrelid) s))
        FROM pg_index i WHERE i.indrelid = 'checkpoints'::regclass))
"""


# ------------------------------------------------------------------ workflow


def ref(value: str) -> dict[str, Any]:
    return {"valueType": "reference", "value": value}


def echo(step_id: str, value: dict[str, Any]) -> dict[str, Any]:
    return {
        "stepType": "Agent",
        "id": step_id,
        "name": step_id,
        "agentId": "utils",
        "capabilityId": "return-input",
        "inputMapping": {"value": value},
    }


def build_graph() -> dict[str, Any]:
    """`A -> Split -> B`, each an Agent whose result is a checkpoint row; the
    Split's body is one more such Agent per item."""
    body = {
        "name": "item",
        "entryPoint": "item",
        "steps": {
            "item": echo("item", ref("data")),
            "finish": {"stepType": "Finish", "id": "finish", "inputMapping": {"item": ref("steps.item.outputs")}},
        },
        "executionPlan": [{"fromStep": "item", "toStep": "finish"}],
    }
    return {
        "name": "checkpoint-bloat",
        "inputSchema": {"label": {"type": "string"}, "items": {"type": "array"}},
        "entryPoint": "a",
        "steps": {
            "a": echo("a", ref("data.label")),
            "split": {
                "stepType": "Split",
                "id": "split",
                "name": "split",
                "config": {"value": ref("data.items")},
                "subgraph": body,
            },
            "b": echo("b", ref("steps.a.outputs")),
            "finish": {"stepType": "Finish", "id": "finish", "inputMapping": {"label": ref("steps.b.outputs")}},
        },
        "executionPlan": [
            {"fromStep": "a", "toStep": "split"},
            {"fromStep": "split", "toStep": "b"},
            {"fromStep": "b", "toStep": "finish"},
        ],
    }


def build_items(count: int, item_bytes: int, seed: int) -> list[dict[str, Any]]:
    """Items with an incompressible `pad`, so a row over the TOAST threshold
    is stored out of line rather than compressed in place."""
    rng = random.Random(seed)
    raw = item_bytes * 3 // 4
    return [
        {"i": index, "pad": base64.b64encode(rng.getrandbits(8 * raw).to_bytes(raw, "little")).decode() if raw else ""}
        for index in range(count)
    ]


def parse_parameters(text: str) -> dict[str, str]:
    """`name=value,name=value` into storage parameters, `toast.` forms
    included, rejecting any this script does not reset between modes."""
    parameters: dict[str, str] = {}
    for part in filter(None, (p.strip() for p in text.split(","))):
        name, sep, value = part.partition("=")
        name, value = name.strip(), value.strip()
        if not sep or not value:
            raise ValueError(f"expected name=value, got {part!r}")
        if name.removeprefix("toast.") not in STORAGE_PARAMETERS:
            raise ValueError(f"unsupported storage parameter {name!r}")
        if not all(c.isalnum() or c in "._-" for c in value):
            raise ValueError(f"unsupported value {value!r} for {name}")
        parameters[name] = value
    return parameters


def mode_parameters(mode: str, tuned: dict[str, str]) -> dict[str, str]:
    if mode == "paused":
        return {"autovacuum_enabled": "false", "toast.autovacuum_enabled": "false"}
    if mode == "tuned":
        return dict(tuned)
    return {}


# ------------------------------------------------------------------ plumbing


def free_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def docker(*args: str, check: bool = True, **kwargs: Any) -> subprocess.CompletedProcess:
    result = subprocess.run(["docker", *args], text=True, capture_output=True, **kwargs)
    if check and result.returncode != 0:
        raise RuntimeError(f"docker {' '.join(args[:2])} failed: {result.stderr.strip()}")
    return result


class Database:
    def __init__(self, container: str, name: str = PG_USER):
        self.container = container
        self.name = name

    def run(self, sql: str) -> str:
        result = docker(
            "exec", "-i", self.container, "psql", "-X", "-q", "-At", "-v", "ON_ERROR_STOP=1",
            "-U", PG_USER, "-d", self.name, input=sql, check=False,
        )
        if result.returncode != 0:
            raise RuntimeError(f"psql failed: {result.stderr.strip()}\n{sql[:400]}")
        return result.stdout

    def json(self, sql: str) -> Any:
        return json.loads(self.run(sql))


def start_postgres(image: str, name: str) -> tuple[str, int]:
    port = free_port()
    docker(
        "run", "-d", "--name", name,
        "-e", f"POSTGRES_USER={PG_USER}", "-e", f"POSTGRES_PASSWORD={PG_PASSWORD}", "-e", f"POSTGRES_DB={PG_USER}",
        "-p", f"127.0.0.1:{port}:5432", image,
        "-c", "shared_preload_libraries=pg_stat_statements",
        "-c", "log_autovacuum_min_duration=0",
        "-c", "wal_keep_size=4096",
    )
    deadline = time.monotonic() + 90
    while time.monotonic() < deadline:
        # The image restarts Postgres once after init; wait for the final one.
        result = docker("logs", name, check=False)
        logs = result.stdout + result.stderr
        if logs.count("database system is ready to accept connections") >= 2 or "Skipping initialization" in logs:
            ready = docker("exec", name, "pg_isready", "-U", PG_USER, check=False)
            if ready.returncode == 0:
                break
        time.sleep(0.5)
    else:
        raise RuntimeError(f"Postgres container {name} did not become ready")
    # As the dev compose file does: the object-model role is not a superuser,
    # and the server and object-model stores need these extensions.
    Database(name).run(f"""
        CREATE ROLE {OBJECTS_ROLE} LOGIN PASSWORD '{OBJECTS_ROLE}'
          NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS;
        CREATE DATABASE runtara_server OWNER {PG_USER};
        CREATE DATABASE runtara_objects OWNER {OBJECTS_ROLE};
        CREATE EXTENSION pg_walinspect;
        CREATE EXTENSION pg_stat_statements;
        CREATE EXTENSION pgstattuple;
        \\c runtara_server
        CREATE EXTENSION pg_trgm; CREATE EXTENSION vector; CREATE EXTENSION fuzzystrmatch;
        \\c runtara_objects
        CREATE EXTENSION pg_trgm; CREATE EXTENSION vector; CREATE EXTENSION fuzzystrmatch;
        ALTER SCHEMA public OWNER TO {OBJECTS_ROLE};
        REVOKE ALL ON SCHEMA public FROM PUBLIC;
    """)
    return name, port


def start_valkey(image: str, name: str) -> int:
    port = free_port()
    docker("run", "-d", "--name", name, "-p", f"127.0.0.1:{port}:6379", image)
    return port


class Server:
    def __init__(self, args: argparse.Namespace, pg_port: int, valkey_port: int, out: Path):
        self.port = free_port()
        self.base = f"http://127.0.0.1:{self.port}/api/runtime"
        url = f"postgres://{PG_USER}:{PG_PASSWORD}@127.0.0.1:{pg_port}"
        # Nothing may reach a shared database through an inherited URL.
        env = {k: v for k, v in os.environ.items() if not k.endswith("DATABASE_URL")}
        env.update({
            "SERVER_HOST": "127.0.0.1",
            "SERVER_PORT": str(self.port),
            "RUNTARA_DATABASE_URL": f"{url}/{PG_USER}",
            "RUNTARA_SERVER_DATABASE_URL": f"{url}/runtara_server",
            "OBJECT_MODEL_DATABASE_URL": f"postgres://{OBJECTS_ROLE}:{OBJECTS_ROLE}@127.0.0.1:{pg_port}/runtara_objects",
            "VALKEY_HOST": "127.0.0.1",
            "VALKEY_PORT": str(valkey_port),
            "AUTH_PROVIDER": "local",
            "RUNTARA_DEV_USER_ID": "local",
            "TENANT_ID": TENANT,
            "DATA_DIR": str(out / "data"),
            "RUNTARA_AGENT_COMPONENTS_DIR": str(args.components),
            # Retention would delete the seeded rows mid-measurement.
            "RUNTARA_DB_CLEANUP_ENABLED": "false",
            "RUNTARA_INVOCATION_CLEANUP_ENABLED": "false",
            "RUNTARA_IMAGE_CLEANUP_ENABLED": "false",
            # A large Split outlives the 300 s default on a busy machine.
            "RUNTARA_DEFAULT_EXECUTION_TIMEOUT_SECS": str(min(int(args.run_timeout), 3600)),
            "OTEL_SDK_DISABLED": "true",
            "RUST_LOG": "warn",
        })
        self.log = open(out / "server.log", "w")
        self.process = subprocess.Popen([str(args.server)], cwd=out, env=env, stdout=self.log, stderr=subprocess.STDOUT)
        deadline = time.monotonic() + 180
        while time.monotonic() < deadline:
            if self.process.poll() is not None:
                raise RuntimeError(f"server exited with {self.process.returncode}; see {out / 'server.log'}")
            try:
                urllib_request.urlopen(f"http://127.0.0.1:{self.port}/health", timeout=2).read()
                return
            except (urllib_error.URLError, ConnectionError, OSError):
                time.sleep(1)
        raise RuntimeError("server did not become healthy")

    def call(self, method: str, path: str, body: Any = None, timeout: float = 300) -> Any:
        data = None if body is None else json.dumps(body).encode()
        req = urllib_request.Request(self.base + path, data=data, method=method,
                                     headers={"Content-Type": "application/json"})
        try:
            with urllib_request.urlopen(req, timeout=timeout) as response:
                raw = response.read()
        except urllib_error.HTTPError as exc:
            raise RuntimeError(f"{method} {path}: {exc.code} {exc.read()[:500]!r}") from None
        payload = json.loads(raw) if raw else None
        return payload.get("data", payload) if isinstance(payload, dict) else payload

    def deploy(self, graph: dict[str, Any]) -> str:
        workflow = self.call("POST", "/workflows/create", {"name": "checkpoint-bloat", "description": "checkpoint bloat measurement"})["id"]
        self.call("POST", f"/workflows/{workflow}/update", {"executionGraph": graph})
        versions = self.call("GET", f"/workflows/{workflow}/versions")
        version = max(v.get("version", v.get("versionNumber", 1)) for v in versions)
        self.call("POST", f"/workflows/{workflow}/versions/{version}/compile?forceRecompile=true", {}, timeout=900)
        deadline = time.monotonic() + 900
        while time.monotonic() < deadline:
            state = (self.call("GET", f"/workflows/{workflow}/versions/{version}/compilation-progress") or {}).get("status")
            if state == "success":
                return workflow
            if state == "failed":
                raise RuntimeError("workflow compilation failed")
            time.sleep(1)
        raise RuntimeError("workflow compilation timed out")

    def execute(self, workflow: str, data: dict[str, Any], timeout: float) -> tuple[str, str]:
        instance = self.call("POST", f"/workflows/{workflow}/execute",
                             {"inputs": {"data": data, "variables": {}}})["instanceId"]
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            status = (self.call("GET", f"/workflows/instances/{instance}") or {}).get("status")
            if status in ("completed", "failed", "cancelled", "timeout"):
                return instance, status
            time.sleep(0.5)
        raise RuntimeError(f"instance {instance} did not finish in {timeout}s")

    def stop(self) -> None:
        if self.process.poll() is None:
            self.process.terminate()
            try:
                self.process.wait(timeout=60)
            except subprocess.TimeoutExpired:
                self.process.kill()
        self.log.close()


class Sampler(threading.Thread):
    """Polls the table's counters until stopped, tagging each sample with the
    run in progress (None between runs and in the tail)."""

    def __init__(self, db: Database, interval: float):
        super().__init__(daemon=True)
        self.db = db
        self.interval = interval
        self.samples: list[dict[str, Any]] = []
        self.run_index: int | None = None
        self.stopping = threading.Event()

    def run(self) -> None:
        while not self.stopping.is_set():
            try:
                sample = self.db.json(STATS_SQL)
                sample["run"] = self.run_index
                self.samples.append(sample)
            except RuntimeError as exc:
                print(f"sample failed: {exc}", file=sys.stderr)
            self.stopping.wait(self.interval)


# ------------------------------------------------------------------ modes


def reset_table(db: Database, seed_rows: int, seed_state_bytes: int, parameters: dict[str, str]) -> None:
    names = ", ".join(STORAGE_PARAMETERS)
    toast = ", ".join(f"toast.{n}" for n in STORAGE_PARAMETERS)
    sql = [f"ALTER TABLE checkpoints RESET ({names}, {toast});", "TRUNCATE checkpoints;"]
    if seed_rows:
        per_instance = 100
        sql.append(f"""
            INSERT INTO instances (instance_id, tenant_id, status, finished_at)
            SELECT 'seed-' || g, '{TENANT}-seed', 'completed', now()
            FROM generate_series(0, {(seed_rows - 1) // per_instance}) g
            ON CONFLICT (instance_id) DO NOTHING;
            INSERT INTO checkpoints (instance_id, checkpoint_id, state, scope)
            SELECT 'seed-' || (g / {per_instance}), 'seed:' || g,
                   convert_to(repeat(md5(g::text), {max(1, seed_state_bytes // 32)}), 'UTF8'),
                   '[["wf","seed"],["base","agent",["s' || g || '"]]]'
            FROM generate_series(0, {seed_rows - 1}) g;
        """)
    db.run("\n".join(sql))
    # VACUUM sets the live and dead counts outright, but a session's own
    # counts are flushed when it ends: in one session the seed's inserts
    # would be added again after the VACUUM and double the live count.
    sql = ["VACUUM (ANALYZE) checkpoints;"]
    if parameters:
        sql.append("ALTER TABLE checkpoints SET ({});".format(", ".join(f"{k} = {v}" for k, v in parameters.items())))
    # Every mode starts right after a checkpoint, so full-page images are
    # comparable between modes.
    sql.append("CHECKPOINT;")
    db.run("\n".join(sql))


def run_mode(server: Server, db: Database, workflow: str, mode: str, args: argparse.Namespace,
             tuned: dict[str, str]) -> dict[str, Any]:
    toast = mode == "toast"
    seed_rows = 0 if mode == "empty" else args.seed_rows
    parameters = mode_parameters(mode, tuned)
    items = args.toast_items if toast else args.items
    item_bytes = args.toast_item_bytes if toast else args.item_bytes
    runs = args.toast_runs if toast else args.runs
    data = {"label": mode, "items": build_items(items, item_bytes, seed=760)}

    print(f"[{mode}] resetting table (seed {seed_rows} rows, parameters {parameters or 'none'})", flush=True)
    reset_table(db, seed_rows, args.seed_state_bytes, parameters)
    baseline = db.json(STATS_SQL)
    sampler = Sampler(db, args.sample_interval)
    sampler.start()

    records: list[dict[str, Any]] = []
    window_start = baseline["lsn"]
    for index in range(runs):
        db.run("SELECT pg_stat_statements_reset();")
        lsn = db.json("SELECT to_json(pg_current_wal_lsn()::text)")
        if records:
            records[-1]["wal"] = db.json(WAL_SQL.format(start=window_start, end=lsn))
        window_start = lsn
        sampler.run_index = index
        started = time.monotonic()
        instance, status = server.execute(workflow, data, args.run_timeout)
        duration = time.monotonic() - started
        sampler.run_index = None
        time.sleep(args.settle_seconds)
        record = {
            "run": index,
            "instance": instance,
            "status": status,
            "duration_s": round(duration, 2),
            "after": db.json(STATS_SQL),
            "statements": db.json(STATEMENTS_SQL),
        }
        records.append(record)
        after = record["after"]
        print(f"[{mode}] run {index + 1}/{runs}: {status} in {duration:.1f}s, live {after['n_live_tup']}, "
              f"dead {after['n_dead_tup']}, autovacuums {after['autovacuum_count'] - baseline['autovacuum_count']}, "
              f"total {after['total_bytes'] / 1048576:.1f} MiB", flush=True)
        if status != "completed":
            # Keep what was measured: stop this mode's runs but still sample
            # the tail and write the report.
            print(f"[{mode}] stopping after run {index + 1}: instance {instance} ended {status}", flush=True)
            break

    end_tuples = db.json(TUPLE_SQL)
    print(f"[{mode}] tail: sampling for {args.tail_seconds}s", flush=True)
    time.sleep(args.tail_seconds)
    sampler.stopping.set()
    sampler.join()
    tail = db.json(STATS_SQL)
    records[-1]["wal"] = db.json(WAL_SQL.format(start=window_start, end=tail["lsn"]))
    return {
        "mode": mode,
        "seed_rows": seed_rows,
        "storage_parameters": parameters,
        "items": items,
        "item_bytes": item_bytes,
        "baseline": baseline,
        "runs": records,
        "end_of_runs_tuples": end_tuples,
        "tail": tail,
        "tail_tuples": db.json(TUPLE_SQL),
        "samples": sampler.samples,
        "summary": summarize(baseline, records, sampler.samples, tail),
    }


def summarize(baseline: dict[str, Any], runs: list[dict[str, Any]], samples: list[dict[str, Any]],
              tail: dict[str, Any]) -> dict[str, Any]:
    def mib(n: float) -> float:
        return round(n / 1048576, 2)

    def growth(key: str) -> float:
        window = [r["after"][key] for r in runs[-10:]]
        return mib(window[-1] - window[0])

    sizes = [r["after"]["total_bytes"] for r in runs]
    last = sizes[-10:]
    wal = [r["wal"] for r in runs if "wal" in r]
    wal_total = sum(w["total_bytes"] for w in wal)
    wal_checkpoints = sum(w["checkpoints_bytes"] for w in wal)
    by_record: dict[str, int] = {}
    by_relation: dict[str, int] = {}
    for w in wal:
        for row in w["breakdown"]:
            key = f"{row['kind']} {row['record']}"
            by_record[key] = by_record.get(key, 0) + row["bytes"]
        for row in w.get("by_relation", []):
            by_relation[row["relation"]] = by_relation.get(row["relation"], 0) + row["bytes"]
    dead = [s["n_dead_tup"] for s in samples] + [r["after"]["n_dead_tup"] for r in runs]
    return {
        "runs": len(runs),
        "failed_runs": [r["run"] for r in runs if r["status"] != "completed"],
        "mean_duration_s": round(sum(r["duration_s"] for r in runs) / len(runs), 2),
        "first_last_duration_s": [runs[0]["duration_s"], runs[-1]["duration_s"]],
        "autovacuums": tail["autovacuum_count"] - baseline["autovacuum_count"],
        "toast_autovacuums": (tail["toast_autovacuum_count"] or 0) - (baseline["toast_autovacuum_count"] or 0),
        "peak_dead_tuples": max(dead),
        "dead_tuples_end_of_runs": runs[-1]["after"]["n_dead_tup"],
        "dead_tuples_after_tail": tail["n_dead_tup"],
        "live_tuples_after_tail": tail["n_live_tup"],
        "total_mib_baseline": mib(baseline["total_bytes"]),
        "total_mib_after_each_run": [mib(s) for s in sizes],
        "total_mib_after_tail": mib(tail["total_bytes"]),
        "growth_last_10_runs_pct": round(100 * (last[-1] - last[0]) / last[0], 2) if last[0] else None,
        # The Split's result row stays live and lands in TOAST, so a growing
        # total alone does not mean bloat; the heap and indexes say whether
        # the space deleted rows leave is being reused.
        "growth_last_10_runs_mib": {
            "heap": growth("heap_bytes"),
            "indexes": growth("index_bytes"),
            "toast": growth("toast_bytes"),
        },
        "wal_mib_total": mib(wal_total),
        "wal_mib_per_run": mib(wal_total / len(runs)),
        "wal_mib_checkpoints": mib(wal_checkpoints),
        "wal_mib_checkpoints_by_record": {k: mib(v) for k, v in sorted(by_record.items(), key=lambda kv: -kv[1])},
        "wal_mib_by_relation": {k: mib(v) for k, v in sorted(by_relation.items(), key=lambda kv: -kv[1])[:10]},
    }


def environment(db: Database, args: argparse.Namespace) -> dict[str, Any]:
    names = ", ".join(f"'{n}'" for n in POSTGRES_SETTINGS)
    revision_file = Path(args.server).with_name("REVISION")
    revision = revision_file.read_text().strip() if revision_file.exists() else subprocess.run(
        ["git", "rev-parse", "HEAD"], cwd=REPO_ROOT, text=True, capture_output=True).stdout.strip()
    return {
        "postgres": db.json("SELECT to_json(version())"),
        "settings": db.json(f"SELECT json_object_agg(name, setting || coalesce(unit, '')) FROM pg_settings WHERE name IN ({names})"),
        "server_revision": revision,
        "platform": platform.platform(),
        "argv": sys.argv[1:],
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--modes", default="empty,seeded,paused", help=f"comma-separated, from {', '.join(MODES)}")
    parser.add_argument("--runs", type=int, default=30)
    parser.add_argument("--items", type=int, default=10_000)
    parser.add_argument("--item-bytes", type=int, default=256, help="incompressible padding per item")
    parser.add_argument("--seed-rows", type=int, default=1_000_000)
    parser.add_argument("--seed-state-bytes", type=int, default=384)
    parser.add_argument("--tuned", default="autovacuum_vacuum_scale_factor=0.02,autovacuum_vacuum_insert_scale_factor=0.02",
                        help="storage parameters for the tuned mode, name=value,...")
    parser.add_argument("--toast-runs", type=int, default=5)
    parser.add_argument("--toast-items", type=int, default=2_000)
    parser.add_argument("--toast-item-bytes", type=int, default=3_000)
    parser.add_argument("--sample-interval", type=float, default=2.0)
    parser.add_argument("--settle-seconds", type=float, default=2.0)
    parser.add_argument("--tail-seconds", type=float, default=180.0)
    parser.add_argument("--run-timeout", type=float, default=1800.0)
    parser.add_argument("--server", type=Path, default=REPO_ROOT / "target" / "debug" / "runtara-server")
    parser.add_argument("--components", type=Path, default=REPO_ROOT / "target" / "agent-components")
    parser.add_argument("--postgres-image", default="pgvector/pgvector:pg18")
    parser.add_argument("--valkey-image", default="valkey/valkey:8-alpine")
    parser.add_argument("--output", type=Path, help="report path (default .data/checkpoint-bloat/<time>.json)")
    parser.add_argument("--keep", action="store_true", help="leave the containers running")
    args = parser.parse_args()

    modes = [m.strip() for m in args.modes.split(",") if m.strip()]
    unknown = set(modes) - set(MODES)
    if unknown:
        parser.error(f"unknown modes: {', '.join(sorted(unknown))}")
    tuned = parse_parameters(args.tuned)
    for path in (args.server, args.components):
        if not path.exists():
            parser.error(f"{path} does not exist")

    # Stopping the harness must still stop the server and remove the
    # containers. A shell starts background jobs with SIGINT ignored, so
    # restore it, and turn SIGTERM into an exit that runs `finally`.
    signal.signal(signal.SIGINT, signal.default_int_handler)
    signal.signal(signal.SIGTERM, lambda *_: sys.exit(143))

    # Unique per process, so modes can run as parallel invocations.
    stamp = f"{time.strftime('%Y%m%d-%H%M%S')}-{os.getpid()}"
    out = (args.output.parent if args.output else DEFAULT_OUTPUT_DIR) / f"run-{stamp}"
    out.mkdir(parents=True, exist_ok=True)
    report_path = args.output or DEFAULT_OUTPUT_DIR / f"{stamp}.json"
    pg_name = f"runtara-checkpoint-bloat-pg-{stamp}"
    valkey_name = f"runtara-checkpoint-bloat-valkey-{stamp}"
    server = None
    try:
        _, pg_port = start_postgres(args.postgres_image, pg_name)
        valkey_port = start_valkey(args.valkey_image, valkey_name)
        print(f"postgres {pg_name} on {pg_port}, valkey on {valkey_port}", flush=True)
        server = Server(args, pg_port, valkey_port, out)
        db = Database(pg_name)
        workflow = server.deploy(build_graph())
        print(f"workflow {workflow} compiled; server on {server.port}", flush=True)
        report: dict[str, Any] = {"environment": environment(db, args), "modes": []}
        for mode in modes:
            report["modes"].append(run_mode(server, db, workflow, mode, args, tuned))
            report_path.parent.mkdir(parents=True, exist_ok=True)
            report_path.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n")
        print(json.dumps({m["mode"]: m["summary"] for m in report["modes"]}, indent=2))
        print(f"report: {report_path}")
        failed = [m["mode"] for m in report["modes"] if m["summary"]["failed_runs"]]
        if failed:
            print(f"runs failed in: {', '.join(failed)}", file=sys.stderr)
            return 1
        return 0
    finally:
        if server:
            server.stop()
        if not args.keep:
            for name in (pg_name, valkey_name):
                docker("rm", "-f", name, check=False)


if __name__ == "__main__":
    sys.exit(main())
