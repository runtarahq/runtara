# Contributing to Runtara

Thank you for your interest in contributing to Runtara! This document provides guidelines and instructions for contributing.

## Contributor License Agreement (CLA)

**All contributions require signing our [Contributor License Agreement (CLA)](CLA.md).** We cannot accept pull requests from contributors who have not signed the CLA.

Before your first contribution, please read the CLA and include the following statement in your pull request:

```
I have read the Runtara Contributor License Agreement and I agree to its terms.

Name: [Your Full Name]
Email: [Your Email]
Date: [Date]
GitHub Username: [Your GitHub Username]
```

If you have questions about the CLA, contact us at legal@syncmyorders.com.

## Code of Conduct

This project adheres to a [Code of Conduct](CODE_OF_CONDUCT.md). By participating, you are expected to uphold this code.

## How to Contribute

### Reporting Bugs

Before creating a bug report, please check existing issues to avoid duplicates. When creating a bug report, include:

- A clear, descriptive title
- Steps to reproduce the issue
- Expected vs actual behavior
- Your environment (OS, Rust version, PostgreSQL version)
- Relevant logs or error messages

### Suggesting Features

Feature requests are welcome! Please:

- Check existing issues and discussions first
- Describe the problem your feature would solve
- Explain how you envision the solution
- Consider if it fits the project's scope as a durable execution platform

### Pull Requests

1. **Sign the CLA** if this is your first contribution (see above)
2. **Fork the repository** and create your branch from `main`
3. **Follow the coding standards** (see below)
4. **Add tests** for new functionality
5. **Ensure all tests pass** locally
6. **Update documentation** if needed
7. **Submit your PR** with a clear description

## Development Setup

### Prerequisites

- Rust 1.75+ (Edition 2024 features used)
- PostgreSQL 14+
- Protocol Buffers compiler (`protoc`)

### Building

```bash
# Clone your fork
git clone https://github.com/YOUR_USERNAME/runtara.git
cd runtara

# Build all crates
cargo build

# Run tests (no database required)
cargo test -p runtara-sdk
cargo test -p runtara-workflows

# Run tests with database
export TEST_DATABASE_URL=postgres://user:pass@localhost/runtara_test
cargo test -p runtara-core
cargo test -p runtara-environment
```

### Running Locally

```bash
# Set environment variables
export RUNTARA_DATABASE_URL=postgres://user:pass@localhost/runtara

# Run runtara-core
cargo run -p runtara-core

# Run runtara-environment (in another terminal)
cargo run -p runtara-environment

# Run examples (standalone, no server needed)
cargo run -p durable-example --bin basic_example
```

## Coding Standards

### Formatting

All code must be formatted with `rustfmt`:

```bash
cargo fmt --all
```

### Linting

Code must pass clippy with no warnings:

```bash
cargo clippy --all-targets -- -D warnings
```

### Commit Messages

- Use clear, descriptive commit messages
- Start with a verb in imperative mood (e.g., "Add", "Fix", "Update")
- Reference issues when applicable (e.g., "Fix #123")

### Code Style

- Follow Rust API guidelines
- Document public APIs with doc comments
- Add `#[must_use]` where appropriate
- Prefer descriptive names over comments
- Keep functions focused and reasonably sized

### Testing

- Write tests for new functionality
- Tests should be deterministic and fast
- Use descriptive test names that explain what's being tested
- Integration tests go in `tests/` directories

## Project Structure

```
crates/
├── runtara-core/          # Execution engine
├── runtara-environment/   # OCI runner, image registry
├── runtara-protocol/      # QUIC + Protobuf
├── runtara-sdk/           # Instance SDK
├── runtara-management-sdk/# Management client
├── runtara-sdk-macros/    # #[durable] macro
├── runtara-workflows/     # Workflow compiler
├── runtara-dsl/           # DSL types
├── runtara-agents/        # Built-in agents
├── runtara-agent-macro/   # #[agent] macro
├── runtara-workflow-stdlib/# Workflow standard library
├── runtara-test-harness/  # Agent testing utilities
└── durable-example/       # SDK examples
```

## License

By contributing, you agree that your contributions will be licensed under the AGPL-3.0-or-later license.

## Questions?

Feel free to open an issue for questions about contributing. We're happy to help!
