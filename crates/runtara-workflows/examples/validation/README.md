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

### Security Errors (E040-E042)

- **error_security_leak.json** - Passes connection credentials to a non-secure agent (E040)
- **error_security_leak_to_finish.json** - Exposes connection data in workflow outputs (E041)

### Child Scenario Errors (E050)

- **error_invalid_child_version.json** - Invalid child scenario version format (E050)

## Warning Examples (Compilation Succeeds with Warnings)

### Configuration Warnings (W030-W034)

- **warning_high_retry.json** - Excessive retry count (W030)
- **warning_long_timeout.json** - Very long timeout configuration (W034)

### Connection Warnings (W040)

- **warning_unused_connection.json** - Connection step that is never referenced (W040)

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

## Error Code Reference

| Code | Category | Description |
|------|----------|-------------|
| E001 | Graph | Entry point not found |
| E002 | Graph | Unreachable step |
| E004 | Graph | Empty workflow |
| E010 | Reference | Invalid step reference |
| E011 | Reference | Invalid reference path syntax |
| E020 | Agent | Unknown agent |
| E021 | Agent | Unknown capability |
| E022 | Agent | Missing required input |
| E030 | Connection | Unknown integration ID |
| E040 | Security | Connection leak to non-secure agent |
| E041 | Security | Connection leak to Finish step |
| E042 | Security | Connection leak to Log step |
| E050 | Child | Invalid child scenario version |

## Warning Code Reference

| Code | Category | Description |
|------|----------|-------------|
| W003 | Graph | Dangling step (no outgoing edges, terminal without Finish) |
| W020 | Agent | Unknown input field |
| W030 | Config | High retry count |
| W031 | Config | Long retry delay |
| W032 | Config | High parallelism |
| W033 | Config | High max iterations |
| W034 | Config | Long timeout |
| W040 | Connection | Unused connection |
| W050 | Reference | Self-reference (may be intentional in loops) |
