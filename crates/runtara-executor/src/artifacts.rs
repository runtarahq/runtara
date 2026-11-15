// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Staged workflows: each component the server uploads is compiled once, by
//! a sandboxed `runtara-run compile`, and kept as what a run loads.
//!
//! The cache is per tenant: a run loads compiled code without checking it, so
//! one tenant's upload must never become what another tenant's run loads,
//! even if a compile of it went wrong in the worst way.

use std::collections::HashMap;
use std::os::fd::OwnedFd;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use sha2::Digest;

use crate::protocol::{valid_digest, valid_tenant};

/// How long a compile may take.
const COMPILE_TIMEOUT: Duration = Duration::from_secs(300);
/// How much of a refused compile's reason is kept.
const MAX_REASON: usize = 2048;

/// The `runtara-run` this executor starts, and how.
#[derive(Clone, Debug)]
pub struct Runner {
    pub binary: PathBuf,
    /// `--unsandboxed`, `--no-landlock`: what the operator allowed.
    pub flags: Vec<String>,
    /// Names what this build of the runner compiles: a cached compile is for
    /// one runner only.
    pub fingerprint: String,
}

impl Runner {
    /// The runner at `binary`, fingerprinted by its contents.
    pub fn new(binary: PathBuf, flags: Vec<String>) -> Result<Self, String> {
        let contents =
            std::fs::read(&binary).map_err(|error| format!("{}: {error}", binary.display()))?;
        let fingerprint = hex(&sha2::Sha256::digest(&contents)[..8]);
        Ok(Self {
            binary,
            flags,
            fingerprint,
        })
    }

    /// `runtara-run <subcommand>` with `three` and `four` as its descriptors
    /// 3 and 4, an empty environment, and nothing else inherited.
    pub fn command(
        &self,
        subcommand: &str,
        three: OwnedFd,
        four: OwnedFd,
    ) -> tokio::process::Command {
        use std::os::fd::AsRawFd;
        let mut command = tokio::process::Command::new(&self.binary);
        command
            .arg(subcommand)
            .args(&self.flags)
            .env_clear()
            .kill_on_drop(true);
        // SAFETY: only fcntl, dup2 and close, which are async-signal-safe.
        unsafe {
            command.pre_exec(move || {
                // Out of the way first: either may already sit on 3 or 4.
                let high_three = libc::fcntl(three.as_raw_fd(), libc::F_DUPFD, 10);
                let high_four = libc::fcntl(four.as_raw_fd(), libc::F_DUPFD, 10);
                if high_three < 0
                    || high_four < 0
                    || libc::dup2(high_three, 3) < 0
                    || libc::dup2(high_four, 4) < 0
                {
                    return Err(std::io::Error::last_os_error());
                }
                libc::close(high_three);
                libc::close(high_four);
                Ok(())
            });
        }
        command
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Why a workflow was not staged.
#[derive(Debug)]
pub enum StageError {
    /// The request is wrong: a bad name, bytes that are not the digest's.
    Invalid(String),
    /// The component is not a workflow this executor can run.
    Compile(String),
    /// The executor failed.
    Internal(String),
}

/// The staged workflows of every tenant.
pub struct Artifacts {
    root: PathBuf,
    runner: Runner,
    /// One compile of a workflow at a time.
    compiling: Mutex<HashMap<PathBuf, Arc<tokio::sync::Mutex<()>>>>,
}

impl Artifacts {
    pub fn new(state: &Path, runner: Runner) -> Result<Self, String> {
        let root = state.join("compiled");
        std::fs::create_dir_all(&root).map_err(|error| format!("{}: {error}", root.display()))?;
        Ok(Self {
            root,
            runner,
            compiling: Mutex::default(),
        })
    }

    fn compiled(&self, tenant: &str, digest: &str) -> Option<PathBuf> {
        (valid_tenant(tenant) && valid_digest(digest)).then(|| {
            self.root
                .join(tenant)
                .join(format!("{digest}-{}.cwasm", self.runner.fingerprint))
        })
    }

    /// Whether `tenant`'s workflow `digest` is staged.
    pub fn has(&self, tenant: &str, digest: &str) -> bool {
        self.compiled(tenant, digest)
            .is_some_and(|path| path.exists())
    }

    /// The compiled workflow, for a run to load.
    pub fn open(&self, tenant: &str, digest: &str) -> Option<std::fs::File> {
        std::fs::File::open(self.compiled(tenant, digest)?).ok()
    }

    /// Stages `component` as `tenant`'s workflow `digest`.
    pub async fn put(
        &self,
        tenant: &str,
        digest: &str,
        component: &[u8],
    ) -> Result<(), StageError> {
        let compiled = self
            .compiled(tenant, digest)
            .ok_or_else(|| StageError::Invalid("not a tenant and a SHA-256".to_owned()))?;
        if hex(&sha2::Sha256::digest(component)) != digest {
            return Err(StageError::Invalid(
                "the bytes are not the digest's".to_owned(),
            ));
        }
        let lock = self
            .compiling
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .entry(compiled.clone())
            .or_default()
            .clone();
        let _one_at_a_time = lock.lock().await;
        let staged = if compiled.exists() {
            Ok(())
        } else {
            self.compile(&compiled, component).await
        };
        self.compiling
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&compiled);
        staged
    }

    async fn compile(&self, compiled: &Path, component: &[u8]) -> Result<(), StageError> {
        let internal = |error: std::io::Error| StageError::Internal(error.to_string());
        let directory = compiled.parent().expect("a tenant's directory");
        tokio::fs::create_dir_all(directory)
            .await
            .map_err(internal)?;
        let unique = uuid::Uuid::new_v4().simple().to_string();
        let source = directory.join(format!(".{unique}.wasm"));
        let partial = directory.join(format!(".{unique}.cwasm"));
        let result = async {
            tokio::fs::write(&source, component)
                .await
                .map_err(internal)?;
            let input = std::fs::File::open(&source).map_err(internal)?;
            let output = std::fs::File::create(&partial).map_err(internal)?;
            let child = self
                .runner
                .command("compile", input.into(), output.into())
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .spawn()
                .map_err(internal)?;
            let output = tokio::time::timeout(COMPILE_TIMEOUT, child.wait_with_output())
                .await
                .map_err(|_| StageError::Compile("compiling took too long".to_owned()))?
                .map_err(internal)?;
            if !output.status.success() {
                // The runner's last line says why; anything before it is
                // what it said on the way.
                let mut reason = String::from_utf8_lossy(&output.stderr)
                    .lines()
                    .rev()
                    .find(|line| !line.trim().is_empty())
                    .unwrap_or_default()
                    .trim()
                    .to_owned();
                if reason.len() > MAX_REASON {
                    let mut end = MAX_REASON;
                    while !reason.is_char_boundary(end) {
                        end -= 1;
                    }
                    reason.truncate(end);
                }
                // The runner refuses a component with exit 1, and anything
                // else is its own failure (no sandbox, a crash).
                return Err(match output.status.code() {
                    Some(1) => StageError::Compile(reason),
                    _ => StageError::Internal(reason),
                });
            }
            tokio::fs::rename(&partial, compiled)
                .await
                .map_err(internal)
        }
        .await;
        let _ = tokio::fs::remove_file(&source).await;
        let _ = tokio::fs::remove_file(&partial).await;
        result
    }
}
