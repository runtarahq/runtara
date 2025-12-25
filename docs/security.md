# Runtara Security Analysis Report

## Executive Summary

This report identifies security vulnerabilities in the Runtara durable execution engine and recommends open-source tools to improve security posture. The analysis covers authentication, container isolation, network security, input validation, and dependency management.

**Key Findings:**
- 3 Critical issues (missing auth, TLS bypass, minimal image validation) - Host networking FIXED (now defaults to Pasta)
- ~~2 High severity issues~~ FIXED: seccomp/capabilities, user namespaces, and rootless containers now enabled by default
- 2 Medium severity issues (path traversal, input size) - /proc exposure FIXED
- 2 Low severity issues (error leakage, unwrap usage)

**Positive Observations:**
- SQL injection well-protected via parameterized queries
- Modern TLS stack (rustls + ring)
- Security policy documentation exists
- No unsafe Rust blocks in application code

---

## Part 1: Identified Security Issues

### Critical Severity

| Issue | Location | Description |
|-------|----------|-------------|
| No Authentication | [server.rs](../crates/runtara-environment/src/server.rs), [client.rs](../crates/runtara-management-sdk/src/client.rs) | QUIC handlers accept requests without credential validation. Any connected client can perform any operation with any tenant_id. |
| TLS Bypass Option | [config.rs:21-22](../crates/runtara-sdk/src/config.rs#L21-L22), [client.rs:114-125](../crates/runtara-protocol/src/client.rs#L114-L125) | `RUNTARA_SKIP_CERT_VERIFICATION` disables TLS validation. `localhost()` enables this by default. |
| ~~Host Networking~~ | [bundle.rs](../crates/runtara-environment/src/runner/oci/bundle.rs) | FIXED: Default now uses Pasta networking with network namespace isolation. Use `RUNTARA_NETWORK_MODE=host` if host networking is explicitly needed. |
| Minimal Image Validation | [handlers.rs:160-183](../crates/runtara-environment/src/handlers.rs#L160-L183) | Only checks non-empty fields. No binary format, size, or integrity validation. |

### High Severity (FIXED)

| Issue | Location | Status |
|-------|----------|--------|
| ~~No Seccomp/Capabilities~~ | [bundle.rs](../crates/runtara-environment/src/runner/oci/bundle.rs) | FIXED: Seccomp enabled by default with comprehensive syscall allowlist. All capabilities dropped by default. |
| ~~Optional UID Config~~ | [bundle.rs](../crates/runtara-environment/src/runner/oci/bundle.rs) | FIXED: Containers run as root inside user namespace, mapped to host user (rootless). |
| ~~No User Namespace~~ | [bundle.rs](../crates/runtara-environment/src/runner/oci/bundle.rs) | FIXED: User namespace enabled with UID/GID mappings for rootless container operation. |

### Medium Severity

| Issue | Location | Description |
|-------|----------|-------------|
| Path Traversal Risk | [instance_output.rs:124-129](../crates/runtara-environment/src/instance_output.rs#L124-L129) | tenant_id/instance_id used in path construction without validation for special characters. |
| ~~`/proc` Exposure~~ | [bundle.rs](../crates/runtara-environment/src/runner/oci/bundle.rs) | FIXED: `/proc` mounted with `hidepid=2`. Masked and readonly paths configured. |
| No Input Size Limit | [runner.rs:641-643](../crates/runtara-environment/src/runner/oci/runner.rs#L641-L643) | JSON input passed via env var without size limits. DoS potential. |

### Low Severity

| Issue | Location | Description |
|-------|----------|-------------|
| Error Information Leakage | [handlers.rs:194-198](../crates/runtara-environment/src/handlers.rs#L194-L198) | Full error messages with internal paths returned to clients. |
| Excessive unwrap() | [sqlite.rs](../crates/runtara-core/src/persistence/sqlite.rs) | Multiple .unwrap() calls that could panic. |

---

## Part 2: What's Working Well

### SQL Injection Protection
All database queries use parameterized queries via sqlx:
```rust
// Example from postgres.rs
INSERT INTO instances (...) VALUES ($1, $2, ...)
```

### Modern Cryptography Stack
- **TLS**: rustls 0.23.35 (memory-safe, modern)
- **Crypto**: ring backend (vetted)
- **Database TLS**: sqlx uses rustls-ring-webpki

### Security Documentation
- [SECURITY.md](../SECURITY.md) exists with responsible disclosure process
- 48-hour acknowledgment, 30-day fix timeline
- Production security recommendations documented

### Frame Size Limits
- Protocol frames limited to 64MB (MAX_FRAME_SIZE in frame.rs)
- Prevents unbounded deserialization

### Container Security (Implemented)

The OCI container runtime now includes comprehensive security hardening:

**User Namespaces & Rootless Containers:**
- User namespace enabled by default for all containers
- Container UID 0 maps to host user (rootless operation)
- No actual root privileges on the host system

**Seccomp Syscall Filtering:**
- Default action: `SCMP_ACT_ERRNO` (deny unlisted syscalls)
- Comprehensive allowlist for safe operations:
  - File I/O: `read`, `write`, `open`, `openat`, `close`, `stat`, `fstat`, `mkdir`, etc.
  - Memory: `mmap`, `mprotect`, `munmap`, `brk`, `mremap`
  - Process: `clone`, `execve`, `exit`, `wait4`, `getpid`, etc.
  - Networking: `socket`, `connect`, `bind`, `sendmsg`, `recvmsg`, `sendmmsg`, `recvmmsg`, etc.
  - Time: `clock_gettime`, `nanosleep`, `gettimeofday`
  - Signals: `rt_sigaction`, `rt_sigprocmask`, `sigaltstack`
- Architecture support: x86_64 and aarch64

**Capability Dropping:**
- All Linux capabilities dropped by default
- Containers run with minimal privileges

**Filesystem Isolation:**
- Root filesystem mounted read-only
- `/proc` mounted with `hidepid=2` (process isolation)
- Sensitive paths masked: `/proc/acpi`, `/proc/kcore`, `/proc/keys`, `/sys/firmware`, etc.
- System paths read-only: `/proc/bus`, `/proc/fs`, `/proc/sys`

**Network Modes:**
- `Pasta` (default): Network namespace with user-mode NAT via pasta (see below)
- `Host`: Direct host network access (set `RUNTARA_NETWORK_MODE=host`)
- `None`: Full network isolation (set `RUNTARA_NETWORK_MODE=none`)

**Pasta Networking Details:**
Pasta networking is enabled by default:
- Containers run in an isolated network namespace
- Pasta provides user-mode networking with NAT for outbound connections
- Localhost addresses (127.0.0.1) are automatically transformed to the gateway IP
- This allows containers to reach runtara-core on the host via the gateway
- No special configuration needed - works automatically on systems with a default gateway

**Resource Limits:**
- Memory limit: 512MB default
- CPU quota: 50% default (50000/100000 period)

---

## Part 3: Recommended Open Source Security Tools

### Essential (Add Immediately)

#### 1. cargo-audit
Scans dependencies against RustSec Advisory Database for known CVEs.

```bash
cargo install cargo-audit
cargo audit
```

**CI Integration:**
```yaml
- name: Security audit
  run: cargo audit --deny warnings
```

#### 2. cargo-deny
Policy enforcement for licenses, sources, advisories, and duplicates.

```bash
cargo install cargo-deny
```

**Configuration (deny.toml):**
```toml
[advisories]
vulnerability = "deny"
unmaintained = "warn"
unsound = "warn"

[licenses]
allow = ["MIT", "Apache-2.0", "BSD-2-Clause", "BSD-3-Clause", "ISC", "Zlib", "MPL-2.0", "AGPL-3.0"]
copyleft = "warn"

[bans]
multiple-versions = "warn"
wildcards = "deny"

[sources]
unknown-registry = "deny"
unknown-git = "deny"
```

#### 3. Dependabot
Automated dependency update PRs with security alerts.

**Configuration (.github/dependabot.yml):**
```yaml
version: 2
updates:
  - package-ecosystem: "cargo"
    directory: "/"
    schedule:
      interval: "weekly"
    open-pull-requests-limit: 5
```

### Recommended (Add Soon)

#### 4. Trivy
Comprehensive vulnerability scanner for filesystems and containers.

```yaml
- name: Trivy scan
  uses: aquasecurity/trivy-action@master
  with:
    scan-type: 'fs'
    scan-ref: '.'
    severity: 'CRITICAL,HIGH'
    exit-code: '1'
```

#### 5. cargo-geiger
Detects unsafe Rust code in entire dependency tree.

```bash
cargo install cargo-geiger
cargo geiger --all-features
```

#### 6. Semgrep
Static analysis with security-focused rules.

```yaml
- name: Semgrep
  uses: returntocorp/semgrep-action@v1
  with:
    config: p/rust
```

### Advanced Security

#### 7. SBOM Generation
Software Bill of Materials for compliance and tracking.

```bash
cargo install cargo-sbom
cargo sbom > runtara-sbom.json
```

Or use cyclonedx-rust-cargo for CycloneDX format.

#### 8. cargo-outdated
Track outdated dependencies.

```bash
cargo install cargo-outdated
cargo outdated --depth 1
```

### Container Runtime Hardening

#### 9. gVisor
User-space kernel that provides stronger container isolation than Linux namespaces.

#### 10. Falco
Runtime security monitoring for container anomaly detection.

---

## Part 4: CI/CD Security Pipeline Addition

Add this job to [.github/workflows/ci.yml](../.github/workflows/ci.yml):

```yaml
security:
  runs-on: ubuntu-latest
  needs: build
  steps:
    - uses: actions/checkout@v4

    - name: Setup Rust
      uses: actions-rust-lang/setup-rust-toolchain@v1

    - name: Install cargo-audit
      run: cargo install cargo-audit

    - name: Install cargo-deny
      run: cargo install cargo-deny

    - name: Run security audit
      run: cargo audit --deny warnings

    - name: Check dependency policy
      run: cargo deny check

    - name: Trivy filesystem scan
      uses: aquasecurity/trivy-action@master
      with:
        scan-type: 'fs'
        scan-ref: '.'
        severity: 'CRITICAL,HIGH'
        exit-code: '1'
```

---

## Part 5: Dependency Analysis

### Current State
- Cargo.lock committed (reproducible builds)
- Workspace-level dependency management
- Modern versions of critical crates

### Security-Relevant Dependencies

| Dependency | Version | Notes |
|------------|---------|-------|
| quinn | 0.11.9 | QUIC transport - properly configured |
| rustls | 0.23.35 | Modern TLS - good choice |
| sqlx | 0.8 | Uses rustls for DB TLS |
| openssl | 0.10.75 (vendored) | Only for ssh2 support |
| tokio | 1.48.0 | `features=["full"]` - could optimize |

### Recommendations
1. Optimize tokio features to only required subset
2. Evaluate pure-Rust SSH alternative to eliminate openssl dependency
3. Add MSRV (Minimum Supported Rust Version) to Cargo.toml

---

## Part 6: Remediation Priority

### Immediate (This Sprint)
1. Add cargo-audit to CI
2. Add cargo-deny with deny.toml
3. Configure Dependabot

### Short-Term (1-2 Sprints)
4. Validate tenant_id/instance_id format (UUID/alphanumeric only)
5. ~~Add network namespace to container configuration~~ DONE: `RUNTARA_NETWORK_MODE=pasta|none`
6. ~~Enforce non-root UID by default in containers~~ DONE: User namespace with rootless mapping
7. ~~Add user namespace isolation~~ DONE: User namespace enabled by default

### Medium-Term (1-2 Months)
8. ~~Define seccomp profile for containers~~ DONE: Comprehensive syscall allowlist including QUIC networking
9. ~~Drop unnecessary container capabilities~~ DONE: All capabilities dropped by default
10. ~~Mount /proc with hidepid=2~~ DONE: Process isolation enabled
11. Add input size limits
12. Implement authentication layer

### Long-Term
13. Consider gVisor for stronger isolation
14. Add runtime security monitoring (Falco)
15. Generate and publish SBOMs with releases

---

## Appendix: Tool Comparison

| Tool | Purpose | Effort | Impact |
|------|---------|--------|--------|
| cargo-audit | CVE scanning | Low | High |
| cargo-deny | Policy enforcement | Low | High |
| Dependabot | Auto-updates | Low | Medium |
| Trivy | Vuln scanning | Low | High |
| cargo-geiger | Unsafe detection | Low | Medium |
| Semgrep | Static analysis | Medium | Medium |
| SBOM | Compliance | Low | Medium |
| gVisor | Runtime isolation | High | Very High |
| Falco | Runtime monitoring | High | High |
