# Validation Example Workflows

This directory contains example workflow JSON files that demonstrate the validation system in `runtara-workflows`.

## Valid Workflows

- **valid_workflow.json** - A simple valid workflow that passes all validation checks

## Error Examples (Compilation Fails)

### Graph Structure Errors (E001-E004)

- **error_missing_entry_point.json** - Entry point references a non-existent step (E001)
- **error_unreachable_step.json** - Contains a step that cannot be reached from entry point (E002)

### Reference Errors (E010-E011)

- **error_invalid_reference.json** - References a step that doesn't exist (E010)

### Agent/Capability Errors (E020-E022)

- **error_unknown_agent.json** - Uses an agent ID with a typo, demonstrates "Did you mean?" suggestion (E020)

### Child Workflow Errors (E050)

- **error_invalid_child_version.json** - Invalid child workflow version format (E050)

## Warning Examples (Compilation Succeeds with Warnings)

### Configuration Warnings (W030-W034)

- **warning_high_retry.json** - Excessive retry count (W030)
- **warning_long_timeout.json** - Very long timeout configuration (W034)

## Testing Validation

You can test these examples using the `runtara-compile` CLI:

```bash
# Validate only (no compilation)
cargo run -p runtara-workflows --bin runtara-compile -- --validate examples/validation/error_missing_entry_point.json

# Analyze workflow structure
cargo run -p runtara-workflows --bin runtara-compile -- --analyze examples/validation/valid_workflow.json

# Verbose output showing all validation steps
cargo run -p runtara-workflows --bin runtara-compile -- --verbose --validate examples/validation/warning_high_retry.json
```

## Code Reference

The authoritative list of error (`E…`) and warning (`W…`) codes is the table at
the top of [`src/validation.rs`](../../src/validation.rs). The
`doc_examples` test checks that every file here parses and produces the outcome
its name promises (`valid_*` clean, `error_*` an error, `warning_*` a warning).
