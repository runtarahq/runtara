#!/usr/bin/env python3
"""Extract real workflow benchmark logs and render a repeatable comparison table.

Only standard-library Python is required. Candidate comparisons reject changed
workloads or host configurations; dependency changes are listed for review.
"""
import argparse
import json
import statistics
from pathlib import Path

PREFIX = "WORKFLOW_BASELINE_JSON="


def extract(path):
    rows = [json.loads(line[len(PREFIX):]) for line in path.read_text().splitlines()
            if line.startswith(PREFIX)]
    if len(rows) != 1:
        raise ValueError(f"expected one benchmark result in {path}")
    return rows[0]


def cases(run):
    result = {row["name"]: row for row in run["reports"]}
    if len(result) != len(run["reports"]):
        raise ValueError("duplicate case names")
    return result


def compatible(left, right):
    for key in ["profile", "os", "arch", "workers", "disk_cache", "runtime", "wasmtime"]:
        if left[key] != right[key]:
            raise ValueError(f"incomparable configuration: {key}")
    a, b = cases(left), cases(right)
    if a.keys() != b.keys():
        raise ValueError("case inventory changed")
    for name in a:
        for key in ["graph_sha256", "input_sha256", "child_graph", "track_events", "random_values"]:
            if a[name][key] != b[name][key]:
                raise ValueError(f"workload changed: {name}/{key}")


def span(values, divisor=1, digits=3):
    low, high = min(values) / divisor, max(values) / divisor
    return f"{low:.{digits}f}" if low == high else f"{low:.{digits}f}–{high:.{digits}f}"


def render(report):
    runs = report["runs"]
    lines = ["# Measured workflow performance baseline", "",
             f"Recorded {report['date']}; source `{report['source_revision']}`; {len(runs)} runs.", "",
             f"Host: {report['machine']}; {runs[0]['os']}/{runs[0]['arch']}; release profile; Wasmtime {runs[0]['wasmtime']}; four Tokio workers.", "",
             "Values below are ranges across runs, not confidence intervals. Each run uses five warmups, 100 measured fresh-Store executions for small cases and 30 for 100-call/large-payload cases. Compile/cold phases have three samples per case per run.", "",
             "| Workflow | WASM bytes | Gzip bytes | Logic bytes | Cached full run p50 (ms) | Cached full run p95 (ms) | JSON → first result p50 (ms) |",
             "|---|---:|---:|---:|---:|---:|---:|"]
    for name in cases(runs[0]):
        rows = [cases(run)[name] for run in runs]
        for size in ["workflow_wasm_bytes", "workflow_wasm_gzip_bytes", "workflow_logic_wasm_bytes"]:
            if len({r["sizes"][size] for r in rows}) != 1:
                raise ValueError(f"artifact size changed between runs: {name}/{size}")
        sizes = rows[0]["sizes"]
        cells = [name, *(f"{sizes[k]:,}" for k in ["workflow_wasm_bytes", "workflow_wasm_gzip_bytes", "workflow_logic_wasm_bytes"]),
                 span([r["cached_full_run"]["p50_us"] for r in rows], 1000),
                 span([r["cached_full_run"]["p95_us"] for r in rows], 1000),
                 span([r["json_to_first_completed_run"]["p50_us"] for r in rows], 1000)]
        lines.append("| " + " | ".join(cells) + " |")
    lines += ["", "## Preparation and memory", "",
              "| Workflow | JSON → WASM p50 (ms) | Native compile p50 (ms) | Prepared linking p50 (µs) | Serialized native bytes | Largest guest memory bytes |",
              "|---|---:|---:|---:|---:|---:|"]
    for name in cases(runs[0]):
        rows = [cases(run)[name] for run in runs]
        cells = [name, span([r["json_to_wasm"]["p50_us"] for r in rows], 1000),
                 span([r["native_compile"]["p50_us"] for r in rows], 1000),
                 span([r["prepare_link"]["p50_us"] for r in rows]),
                 f"{rows[0]['sizes']['serialized_native_bytes']:,}",
                 f"{max(r['largest_guest_memory_bytes'] for r in rows):,}"]
        lines.append("| " + " | ".join(cells) + " |")
    lines += ["", "The memory column is the largest individual guest memory, **not aggregate guest memory or process RSS**. Serialized native bytes are cache-artifact size, not resident memory.", "",
              "## Durability and comparison boundary", ""]
    for name in ["random_1_defaults", "random_1_durable"]:
        rows = [cases(run)[name] for run in runs]
        lines.append(f"- `{name}`: cached-result replay p50 {span([r['durable_cached_result_replay']['p50_us'] for r in rows], 1000)} ms; checkpoint entries after initial execution: {rows[0]['checkpoint_count']}. Replay asserts byte-identical random output.")
    lines += ["", "`random_1_defaults` leaves durability and retries at DSL defaults. Other random cases set maxRetries=0 and explicitly select durability. A single-call workflow contains one Agent plus a Finish that exposes its result. Chains expose every generated number; Splits validate 100 results. The embedded case invokes a child with one random call.", "",
              "`cached_full_run` measures the production WorkflowExecutor entry through return, including input cloning, fresh Store/WASI construction, instantiation, invocation, in-memory runtime calls, outcome handling and teardown. It excludes component compilation/preparation and construction of the test runtime host. It is not a bare agent-function measurement.", "",
              "`JSON → first result` includes parsing/emission, composition and artifact I/O, uncached native compilation, linking and first execution. The engine and Tokio runtime already exist; filesystem pages may be warm. It excludes Cargo compilation, server request handling, authentication, image registration/download, admission queues and real persistence. Native serialization and gzip measurement occur after this timer. Phase medians do not necessarily sum to the median total.", "",
              "The runtime is the existing in-memory capturing test host. No database, external HTTP service, sleep or signal wait is involved; durable figures exclude network/storage latency. Random-double uses the real utils component and WASI randomness. Event tracking uses in-memory capture, not production log ingestion.", "",
              "Selective-isolation results are **not yet available**. This baseline is not evidence of isolated DSL performance. Re-run these exact cases through the implemented backend and compare raw sizes, p50/p95, preparation and full-run totals. Earlier per-Store microbenchmarks must not be substituted or simply multiplied into these numbers.", "",
              "Raw data includes workload definitions and hashes, dependency hashes, compiler artifact hashes, sample counts and configuration. Use `workflow_baseline_report.py --compare baseline.json candidate.json` for deltas after measuring a candidate on the same host; run identity and dependency differences require review."
              ]
    return "\n".join(lines) + "\n"


def compare(left, right):
    if left["machine"] != right["machine"]:
        raise ValueError("machine description changed; remeasure both sides together")
    compatible(left["runs"][0], right["runs"][0])
    for report in [left, right]:
        for run in report["runs"][1:]:
            compatible(report["runs"][0], run)
            if run["dependency_hashes"] != report["runs"][0]["dependency_hashes"]:
                raise ValueError("dependencies changed within a report")
    lines = [f"Baseline: {left['runs'][0]['backend']}; candidate: {right['runs'][0]['backend']}", "",
             "| Case | WASM delta | Cached p50 delta | Cached p95 delta | JSON → first result p50 delta |",
             "|---|---:|---:|---:|---:|"]
    for name in cases(left["runs"][0]):
        a, b = [[cases(run)[name] for run in report["runs"]] for report in [left, right]]
        deltas = []
        for key, field in [("sizes", "workflow_wasm_bytes"), ("cached_full_run", "p50_us"),
                           ("cached_full_run", "p95_us"), ("json_to_first_completed_run", "p50_us")]:
            baseline = statistics.median(r[key][field] for r in a)
            candidate = statistics.median(r[key][field] for r in b)
            deltas.append(f"{(candidate/baseline-1)*100:+.2f}%")
        lines.append("| " + " | ".join([name, *deltas]) + " |")
    if left["runs"][0]["dependency_hashes"] != right["runs"][0]["dependency_hashes"]:
        lines += ["", "Dependency hashes changed: review raw reports before attributing deltas to isolation."]
    return "\n".join(lines) + "\n"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--logs", nargs="+", type=Path)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--markdown", type=Path)
    parser.add_argument("--date")
    parser.add_argument("--source-revision")
    parser.add_argument("--machine")
    parser.add_argument("--compare", nargs=2, type=Path)
    args = parser.parse_args()
    if args.compare:
        print(compare(*(json.loads(path.read_text()) for path in args.compare)), end="")
        return
    if not all([args.logs, args.output, args.markdown, args.date, args.source_revision, args.machine]):
        parser.error("provide --logs, --output, --markdown, --date, --source-revision and --machine")
    runs = [extract(path) for path in args.logs]
    for run in runs[1:]:
        compatible(runs[0], run)
        if run["dependency_hashes"] != runs[0]["dependency_hashes"]:
            raise ValueError("dependencies changed between baseline runs")
    report = {"date":args.date,"source_revision":args.source_revision,"machine":args.machine,"runs":runs}
    rendered = render(report)
    args.output.write_text(json.dumps(report, indent=2) + "\n")
    args.markdown.write_text(rendered)


if __name__ == "__main__":
    main()
