#!/usr/bin/env bash
# OTLP pipeline integration test; requires a built server, receiver and components.
# Uses isolated databases, retained for inspection. See docs/pipeline-monitoring.md.
set -euo pipefail
exec python3 "$(dirname "$0")/pipeline_otel.py"
