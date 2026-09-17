#!/usr/bin/env python3
"""Validate paired emitted-workflow measurements and render comparable evidence.

This exploratory report does not establish production performance budgets.
Only standard-library Python is required. Raw samples remain in the JSON output.
"""
import argparse
import json
import math
from pathlib import Path

PREFIX = "WORKFLOW_COMPARISON_JSON="
BACKENDS = ("legacy", "isolated-agent-adapter-v1")
COUNTS = {
    "random_1_defaults": 1, "random_1": 1, "random_1_durable": 1,
    "random_1_events": 1, "random_chain_10": 10, "random_chain_100": 100,
    "random_split_100_sequential": 100, "random_split_100_parallel4": 100,
    "random_embed_1": 1, "finish_only": 0, "finish_payload_1mib": 0,
}
REPLAY = {"random_1_defaults", "random_1_durable"}
CONFIG = ("source_revision", "benchmark_executable_sha256", "format_version", "quantile_method", "profile", "os", "arch", "wasmtime",
          "workers", "epoch_tick_ms", "disk_cache", "preparation_method", "runtime",
          "child_limits", "store_limits", "warmups", "smoke", "dependency_hashes",
          "missing_metrics")
WORKLOAD = ("graph", "child_graph", "graph_sha256", "input_sha256", "input_bytes",
            "random_values", "track_events")
METRICS = ("parse_and_emit", "compose_and_package", "worker_precompile",
           "trusted_deserialize", "prepare_link", "dsl_to_first_result",
           "prepared_full_run", "instrumented_prepared_full_run", "durable_replay")


def index(run):
    rows = {(r["name"], r["backend"]): r for r in run["reports"]}
    expected = {(name, backend) for name in COUNTS for backend in BACKENDS}
    if len(rows) != len(run["reports"]) or rows.keys() != expected:
        raise ValueError("duplicate, missing or unexpected workload/backend")
    return rows


def check_samples(metric):
    values = metric["samples_us"]
    if not values or any(isinstance(v, bool) or not isinstance(v, (float, int))
                         or not math.isfinite(v) or v < 0 for v in values):
        raise ValueError("invalid elapsed-time samples")
    ordered = sorted(values)
    expected = {"samples": len(values), "min_us": ordered[0], "max_us": ordered[-1],
                "p50_us": ordered[math.ceil((len(values) - 1) * .5)],
                "p95_us": ordered[math.ceil((len(values) - 1) * .95)]}
    if metric["summary"] != expected:
        raise ValueError("summary disagrees with raw samples")


def validate(report):
    runs = report["runs"]
    if not runs:
        raise ValueError("no paired runs")
    reference = runs[0]
    if reference["format_version"] != 1 or reference["quantile_method"] != "ceil((n-1)*p)":
        raise ValueError("unsupported measurement format or quantile method")
    if reference["profile"] != "release" or reference["smoke"] or reference["disk_cache"]:
        raise ValueError("measurement requires release, no smoke mode, and disabled disk cache")
    if reference["source_revision"] != report["source_revision"]:
        raise ValueError("report source revision disagrees with measured executable run")
    first_rows = index(reference)
    for run in runs:
        for key in CONFIG:
            if run[key] != reference[key]:
                raise ValueError(f"configuration changed: {key}")
        if run["first_backend"] not in BACKENDS:
            raise ValueError("invalid backend order")
        rows = index(run)
        if any(row["backend"] != run["first_backend"] for row in run["reports"][::2]):
            raise ValueError("declared backend order disagrees with recorded order")
        for name, count in COUNTS.items():
            baseline = rows[name, BACKENDS[0]]
            candidate = rows[name, BACKENDS[1]]
            for backend in BACKENDS:
                row = rows[name, backend]
                for key in WORKLOAD:
                    if row[key] != baseline[key] or row[key] != first_rows[name, backend][key]:
                        raise ValueError(f"workload changed: {name}/{key}")
                if row["random_values"] != count:
                    raise ValueError(f"wrong output count: {name}")
                if row["sizes"] != first_rows[name, backend]["sizes"]:
                    raise ValueError(f"artifact changed across sessions: {name}/{backend}")
                sizes = row["sizes"]
                if sizes["workflow_wasm_bytes"] != sum(sizes[k] for k in
                        ("root_component_bytes", "unique_child_bytes", "package_index_and_framing_bytes")):
                    raise ValueError("incomplete package size accounting")
                isolated = backend == BACKENDS[1] and count > 0
                evidence = row["isolation"]
                if evidence["agent_boundary"] != isolated or evidence["embed_boundary"]:
                    raise ValueError("incorrect isolation claim")
                if evidence["verified_starts_per_instrumented_run"] != (count if isolated else 0):
                    raise ValueError("missing actual isolated-call evidence")
                if sizes["unique_children"] != int(isolated) or sizes["bindings"] != int(isolated):
                    raise ValueError("unexpected package binding/deduplication count")
                if not count and not evidence["control_reason"]:
                    raise ValueError("Finish-only control must be labeled")
                if evidence["context_contract"] != ("live-adapter-call:1" if isolated else "none"):
                    raise ValueError("unexpected invocation context contract")
                if set(row["metrics"]) != set(METRICS):
                    raise ValueError("metric inventory changed")
                for name_metric, metric in row["metrics"].items():
                    if name_metric == "durable_replay" and name not in REPLAY:
                        if metric is not None:
                            raise ValueError("unexpected replay measurement")
                    else:
                        check_samples(metric)
                if name in REPLAY and (row["checkpoint_count"] == 0 or evidence["verified_replay_starts"] != 0):
                    raise ValueError("checkpoint replay lacks evidence")
            if baseline["checkpoint_count"] != candidate["checkpoint_count"]:
                raise ValueError("checkpoint behavior changed")
            for key in METRICS:
                a, b = baseline["metrics"][key], candidate["metrics"][key]
                if a is not None and len(a["samples_us"]) != len(b["samples_us"]):
                    raise ValueError("sample counts differ across backends")
    return [index(run) for run in runs]


def span(values, digits=3):
    low, high = min(values), max(values)
    return f"{low:,.{digits}f}" if low == high else f"{low:,.{digits}f}–{high:,.{digits}f}"


def delta(left, right):
    absolute = [b - a for a, b in zip(left, right)]
    percent = [100 * (b - a) / a for a, b in zip(left, right) if a != 0]
    return span(absolute), span(percent, 2) + "%" if len(percent) == len(left) else "N/A"


def metric_table(indexes, metric, percentile="p50_us", divisor=1000):
    lines = ["| Workload | Legacy (ms) | Isolated Agent (ms) | Paired delta (ms) | Paired delta % | Samples/backend/run |",
             "|---|---:|---:|---:|---:|---:|"]
    for name in COUNTS:
        if indexes[0][name, BACKENDS[0]]["metrics"][metric] is None:
            continue
        left, right = [[rows[name, b]["metrics"][metric]["summary"][percentile] / divisor
                        for rows in indexes] for b in BACKENDS]
        absolute, percent = delta(left, right)
        counts = [rows[name, BACKENDS[0]]["metrics"][metric]["summary"]["samples"] for rows in indexes]
        lines.append("| " + " | ".join([name, span(left), span(right), absolute, percent, span(counts, 0)]) + " |")
    return lines


def render(report):
    indexes = validate(report)
    run = report["runs"][0]
    orders = ", ".join(r["first_backend"] for r in report["runs"])
    lines = ["# Paired emitted Agent isolation measurements", "",
             f"Recorded {report['date']}; implementation `{report['source_revision']}`; {len(indexes)} paired sessions.", "",
             f"Host: {report['machine']}; {run['os']}/{run['arch']}; release; Wasmtime {run['wasmtime']}; {run['workers']} Tokio workers; disk cache disabled.", "",
             f"First backend in each session: {orders}. Backends within each session ran serially.", "",
             "These are exploratory local measurements of `isolated-agent-adapter-v1`, not production qualification. The same 11 graphs and inputs run through both backends. An embedded graph remains inline; only its Agent call is isolated. Finish-only workloads use the legacy executor on both sides and are controls.", "",
             "Each cell shows the range across independent sessions. Timing cells contain per-session p50/p95 values; they are not pooled percentiles or confidence intervals. Deltas compare corresponding sessions and use `100 × (isolated − legacy) / legacy`; a zero baseline yields N/A. Raw samples and configuration are retained in the [JSON companion](workflow-performance-comparison.json).", "",
             "## Complete artifact sizes", "",
             "| Workload | Artifact metric | Legacy bytes | Isolated bytes | Delta bytes | Delta % |",
             "|---|---|---:|---:|---:|---:|"]
    for name in COUNTS:
        sizes = [indexes[0][name, b]["sizes"] for b in BACKENDS]
        for label, key in [("Complete WASM", "workflow_wasm_bytes"),
                           ("Complete gzip", "workflow_wasm_gzip_bytes"),
                           ("Native worker payload", "serialized_native_package_bytes")]:
            a, b = [s[key] for s in sizes]
            percent = f"{100 * (b - a) / a:+.2f}%" if a else "N/A"
            cells = [name, label, f"{a:,}", f"{b:,}", f"{b-a:+,}", percent]
            lines.append("| " + " | ".join(cells) + " |")
    lines += ["", "WASM and gzip cover the complete distributable package. Native bytes cover the full worker payload, including its child index; they are neither RSS nor portable WASM. JSON also separates root component, logic, unique child bytes and package framing. Logic is a subset of the root and must not be added again. Every isolated random workload has one binding and one unique utils component, including 100-call and parallel workloads.", "",
              "## Prepared full execution p50", ""] + metric_table(indexes, "prepared_full_run")
    lines += ["", "## Prepared full execution p95", ""] + metric_table(indexes, "prepared_full_run", "p95_us")
    lines += ["", "The timer surrounds fresh execution, including input cloning, per-run execution-context/launcher construction when needed, fresh Store setup, guest orchestration, result collection and owned child teardown. The in-memory runtime host is constructed before this timer. Headline runs do not increment the optional child-launch counter. Separate instrumented runs verify the actual child counts and zero retained task-result bytes. This is the full Agent + Finish workflow cost, not an Agent-only or parent-step span.", "",
              "## DSL to first result p50", ""] + metric_table(indexes, "dsl_to_first_result")
    lines += ["", "Cold preparation has only three samples per session, and identical Finish-only controls show substantial variation. Treat these cold deltas as exploratory, not regression gates. Larger controlled runs are required for qualification.", "",
              "The engine already exists. This timer includes DSL parse/emission, composition/packaging, worker-format precompile, trusted deserialization, linking and first execution. The production worker codec is invoked synchronously in process with an explicitly cache-disabled engine. Process startup, IPC, server queueing and database persistence are excluded. Filesystem pages may be warm; no claim is made about a cold OS cache. Gzip, output correctness checks and artifact size inspection occur after this timer.", ""]
    for label, key in [("Parse and emit", "parse_and_emit"), ("Compose and package", "compose_and_package"), ("Worker precompile", "worker_precompile"), ("Trusted deserialize", "trusted_deserialize"), ("Prepared linking", "prepare_link"), ("Durable checkpoint replay", "durable_replay")]:
        lines += [f"## {label} p50", ""] + metric_table(indexes, key) + [""]
    lines += ["Worker precompile includes bounded source read, hash/validation, root and unique-child native compilation, serialization and integrity hashing. Deserialization includes integrity/configuration checks and loading native members. Phase medians need not sum to the median total. Replay asserts byte-identical cached random results and zero new isolated starts.", "",
              "## Instrumentation comparison", "",
              "| Workload | Isolated plain p50 (ms) | Isolated instrumented p50 (ms) | Paired delta (ms) | Paired delta % |",
              "|---|---:|---:|---:|---:|"]
    for name in COUNTS:
        a, b = [[rows[name, BACKENDS[1]]["metrics"][key]["summary"]["p50_us"] / 1000 for rows in indexes]
                for key in ("prepared_full_run", "instrumented_prepared_full_run")]
        absolute, percent = delta(a, b)
        lines.append("| " + " | ".join([name, span(a), span(b), absolute, percent]) + " |")
    lines += ["", "Instrumented runs add launch counting and retained-result assertions; execution order alternates with plain runs. Small or negative differences can be measurement noise and must not be interpreted as an optimization. Instrumentation does not provide a direct Agent service span.", "",
              "## Remaining measurements and acceptance", "",
              "- Agent-only service and parent-step spans remain pending for both backends.",
              "- Server end-to-end time, persistence, targeted cancellation and aggregate memory/RSS remain pending.",
              "- The JSON root-memory diagnostic is the largest single memory in the root Store; it excludes isolated child Stores and is not a capacity comparison.",
              "- Concurrency/load qualification, adequate production-tail samples, agreed budgets and all P5/P6 compatibility gates remain pending. These local timings do not authorize default enablement.", ""]
    return "\n".join(lines)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--logs", nargs="+", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--markdown", type=Path, required=True)
    parser.add_argument("--date", required=True)
    parser.add_argument("--source-revision", required=True)
    parser.add_argument("--machine", required=True)
    args = parser.parse_args()
    runs = []
    for path in args.logs:
        found = [json.loads(line[len(PREFIX):]) for line in path.read_text().splitlines() if line.startswith(PREFIX)]
        if len(found) != 1:
            raise ValueError(f"expected one paired result in {path}")
        runs.extend(found)
    report = {"date": args.date, "source_revision": args.source_revision, "machine": args.machine, "runs": runs}
    rendered = render(report)
    args.output.write_text(json.dumps(report, indent=2) + "\n")
    args.markdown.write_text(rendered)


if __name__ == "__main__":
    main()
