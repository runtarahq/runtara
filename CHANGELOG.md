# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.20] - 2025-12-17

Initial open-source release of Runtara.

### Added

- **runtara-core**: Durable execution engine with checkpoint persistence, signal handling, and wake scheduling
- **runtara-environment**: Execution environment with OCI container runner and image registry
- **runtara-protocol**: QUIC transport layer with Protobuf message definitions
- **runtara-sdk**: Instance SDK for building durable workflows with checkpoint-based crash recovery
- **runtara-sdk-macros**: `#[durable]` proc macro for transparent function durability
- **runtara-management-sdk**: Management client for external tools to control instances
- **runtara-workflows**: Workflow compiler for JSON DSL definitions
- **runtara-dsl**: Type definitions for workflow DSL
- **runtara-agents**: Built-in agents (HTTP, delay, transform, conditional, child workflow)
- **runtara-agent-macro**: `#[agent]` proc macro for custom agent definitions
- **runtara-workflow-stdlib**: Standard library for compiled workflows
- **runtara-test-harness**: Testing utilities for agent development
- **durable-example**: Example workflows demonstrating SDK usage patterns

### Features

- Checkpoint-based durability with PostgreSQL persistence
- Durable sleep with automatic instance wake scheduling
- Signal handling (cancel, pause, resume) with efficient polling
- QUIC transport with TLS for secure communication
- OCI container runtime for isolated workflow execution
- Multi-tenant architecture with tenant isolation
- JSON-based workflow DSL with step sequencing and branching
- Extensible agent framework for custom workflow actions
