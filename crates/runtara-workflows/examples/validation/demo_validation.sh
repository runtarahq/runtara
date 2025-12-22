#!/bin/bash
# Demo script for runtara-workflows validation features
# Run from the repository root: ./crates/runtara-workflows/examples/validation/demo_validation.sh

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../../../.." && pwd)"

cd "$REPO_ROOT"

echo "=============================================="
echo "  Runtara Workflow Validation Demo"
echo "=============================================="
echo ""

# Build the compile binary
echo "Building runtara-compile..."
cargo build -p runtara-workflows --bin runtara-compile --quiet
echo ""

COMPILE_BIN="./target/debug/runtara-compile"
EXAMPLES_DIR="$SCRIPT_DIR"

# Common args for the CLI
COMMON_ARGS="--tenant demo --scenario validation-test"

# Function to run validation and show output
run_validation() {
    local file=$1
    local description=$2
    local extra_flags=${3:-"--validate"}

    echo "----------------------------------------------"
    echo "Test: $description"
    echo "File: $(basename "$file")"
    echo "Flags: $extra_flags"
    echo ""

    # Run and capture output, don't fail on validation errors
    set +e
    $COMPILE_BIN --workflow "$file" $COMMON_ARGS $extra_flags 2>&1
    local exit_code=$?
    set -e

    echo ""
    echo "Exit code: $exit_code"
    echo ""
}

echo "=============================================="
echo "  1. Valid Workflow (should pass)"
echo "=============================================="
run_validation "$EXAMPLES_DIR/valid_workflow.json" "Valid workflow with no errors"

echo "=============================================="
echo "  2. Graph Structure Errors"
echo "=============================================="
run_validation "$EXAMPLES_DIR/error_missing_entry_point.json" "E001: Entry point not found"
run_validation "$EXAMPLES_DIR/error_unreachable_step.json" "E002: Unreachable step"

echo "=============================================="
echo "  3. Reference Errors"
echo "=============================================="
run_validation "$EXAMPLES_DIR/error_invalid_reference.json" "E010: Invalid step reference"

echo "=============================================="
echo "  4. Agent Errors (with suggestions)"
echo "=============================================="
run_validation "$EXAMPLES_DIR/error_unknown_agent.json" "E020: Unknown agent with 'Did you mean?' suggestion"

echo "=============================================="
echo "  5. Security Errors"
echo "=============================================="
run_validation "$EXAMPLES_DIR/error_security_leak.json" "E040: Connection leak to non-secure agent"
run_validation "$EXAMPLES_DIR/error_security_leak_to_finish.json" "E041: Connection leak to Finish step"

echo "=============================================="
echo "  6. Child Scenario Errors"
echo "=============================================="
run_validation "$EXAMPLES_DIR/error_invalid_child_version.json" "E050: Invalid child version"

echo "=============================================="
echo "  7. Configuration Warnings"
echo "=============================================="
run_validation "$EXAMPLES_DIR/warning_high_retry.json" "W030: High retry count warning"
run_validation "$EXAMPLES_DIR/warning_long_timeout.json" "W034: Long timeout warning"
run_validation "$EXAMPLES_DIR/warning_unused_connection.json" "W040: Unused connection warning"

echo "=============================================="
echo "  8. Analyze Mode"
echo "=============================================="
run_validation "$EXAMPLES_DIR/valid_workflow.json" "Workflow analysis report" "--analyze"

echo "=============================================="
echo "  9. Verbose Mode"
echo "=============================================="
run_validation "$EXAMPLES_DIR/valid_workflow.json" "Verbose validation output" "--validate --verbose"

echo "=============================================="
echo "  Demo Complete!"
echo "=============================================="
echo ""
echo "Error Code Reference:"
echo "  E001 - Entry point not found"
echo "  E002 - Unreachable step"
echo "  E003 - Dangling step"
echo "  E010 - Invalid step reference"
echo "  E020 - Unknown agent"
echo "  E021 - Unknown capability"
echo "  E040 - Connection leak to non-secure agent"
echo "  E041 - Connection leak to Finish"
echo "  E050 - Invalid child version"
echo ""
echo "Warning Code Reference:"
echo "  W030 - High retry count"
echo "  W034 - Long timeout"
echo "  W040 - Unused connection"
echo ""
