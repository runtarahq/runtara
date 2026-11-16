// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! `runtara-run`: one workflow run in a process of its own.
//!
//! Its launcher (the executor) starts one per run and hands it three things:
//! a spec on standard input, the compiled workflow on descriptor 3, and on
//! descriptor 4 one end of a socket pair, over which the guest's workflow API
//! calls travel as HTTP/2. The process holds no credential and no address:
//! the launcher at the other end of the socket relays the calls to the
//! server, attaching the run's credential there.
//!
//! Before it reads any of them it sandboxes itself ([`sandbox`]): it can open
//! no file, make no socket, start no program and signal no other process, so
//! a guest that broke out of Wasmtime would find itself somewhere with
//! nothing to take and nowhere to go.
//!
//! ```text
//! runtara-run run      < spec   3: compiled workflow   4: API socket
//! runtara-run compile           3: component           4: compiled workflow (written)
//! runtara-run selftest          whether the sandbox holds here
//! ```
//!
//! `run` prints one line on standard output when the guest ends,
//! `{"ended": "returned" | "failed" | "trapped" | "timeout", "reason": …}`,
//! and exits 0 if the guest's `run` returned ok and 1 otherwise. The run's
//! outcome is not in it: the guest posts that over the API, and a launcher
//! that saw none posted reports the failure itself. The guest's own output
//! goes to standard error. Exit 2 is a refusal before the guest started (a
//! bad spec, no sandbox); 3, that the launcher closed the socket.

pub mod guest;
pub mod sandbox;

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::os::fd::{FromRawFd, OwnedFd};
use std::time::Duration;

use serde::Deserialize;

/// The compiled workflow (`run`) or the component to compile (`compile`).
pub const INPUT_DESCRIPTOR: i32 = 3;
/// The API socket (`run`) or where the compiled workflow is written
/// (`compile`).
pub const OUTPUT_DESCRIPTOR: i32 = 4;

/// The guest's `run` returned ok.
pub const EXIT_RETURNED: i32 = 0;
/// The guest failed, trapped or ran out of time.
pub const EXIT_GUEST: i32 = 1;
/// Refused before the guest started.
pub const EXIT_REFUSED: i32 = 2;
/// The launcher closed the API socket.
pub const EXIT_ABANDONED: i32 = 3;

const MAX_SPEC_BYTES: u64 = 1024 * 1024;
const MAX_COMPONENT_BYTES: u64 = 256 * 1024 * 1024;
const MAX_ENV: usize = 64;

/// What a run may use, as its launcher states it.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Spec {
    /// The guest's linear memory, summed over its memories.
    pub wasm_memory_bytes: u64,
    /// Elements in any one of the guest's tables.
    #[serde(default = "default_table_elements")]
    pub table_elements: u64,
    /// How long the guest may run.
    pub lifetime_seconds: u64,
    /// The guest's environment.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

fn default_table_elements() -> u64 {
    10_000_000
}

impl Spec {
    /// Reads a spec, refusing one out of range.
    pub fn parse(json: &[u8]) -> Result<Self, String> {
        let spec: Self = serde_json::from_slice(json).map_err(|error| error.to_string())?;
        if !(65_536..=4 * 1024 * 1024 * 1024).contains(&spec.wasm_memory_bytes) {
            return Err("wasm_memory_bytes is out of range".to_owned());
        }
        if !(1..=86_400).contains(&spec.lifetime_seconds) {
            return Err("lifetime_seconds is out of range".to_owned());
        }
        if spec.table_elements > 100_000_000 {
            return Err("table_elements is out of range".to_owned());
        }
        if spec.env.len() > MAX_ENV {
            return Err("too many environment variables".to_owned());
        }
        Ok(spec)
    }

    fn limits(self) -> guest::Limits {
        guest::Limits {
            memory_bytes: usize::try_from(self.wasm_memory_bytes).unwrap_or(usize::MAX),
            table_elements: usize::try_from(self.table_elements).unwrap_or(usize::MAX),
            lifetime: Duration::from_secs(self.lifetime_seconds),
            env: self.env,
        }
    }
}

/// How a subcommand was asked to sandbox itself.
#[derive(Clone, Copy, Debug, Default)]
pub struct Flags {
    /// No sandbox at all: development on a host that has none.
    pub unsandboxed: bool,
    /// Go on without Landlock where the kernel has none.
    pub no_landlock: bool,
}

fn enter(flags: Flags, profile: sandbox::Profile) -> Result<(), String> {
    if flags.unsandboxed {
        eprintln!("runtara-run: NOT SANDBOXED (--unsandboxed)");
        return Ok(());
    }
    sandbox::apply(sandbox::Options {
        profile,
        require_landlock: !flags.no_landlock,
        highest_descriptor: OUTPUT_DESCRIPTOR,
    })
    .map(|_| ())
    .map_err(|error| format!("sandbox: {error}"))
}

/// Takes a descriptor this process was started with.
fn inherited(descriptor: i32) -> std::fs::File {
    // SAFETY: the launcher's contract: the descriptor is open and this
    // process's own. It is taken once.
    std::fs::File::from(unsafe { OwnedFd::from_raw_fd(descriptor) })
}

fn read_all(mut from: impl Read, limit: u64, what: &str) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    (&mut from)
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("reading {what}: {error}"))?;
    if bytes.len() as u64 > limit {
        return Err(format!("{what} is too large"));
    }
    Ok(bytes)
}

fn report(ended: &str, reason: Option<&str>) {
    println!("{}", serde_json::json!({"ended": ended, "reason": reason}));
}

fn run_guest(flags: Flags) -> Result<guest::Ended, String> {
    enter(flags, sandbox::Profile::Run)?;
    let spec = Spec::parse(&read_all(std::io::stdin(), MAX_SPEC_BYTES, "the spec")?)?;
    let compiled = read_all(
        inherited(INPUT_DESCRIPTOR),
        MAX_COMPONENT_BYTES,
        "the workflow",
    )?;
    let socket = std::os::unix::net::UnixStream::from(OwnedFd::from(inherited(OUTPUT_DESCRIPTOR)));
    socket
        .set_nonblocking(true)
        .map_err(|error| format!("the API socket: {error}"))?;
    let engine = guest::engine()?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| error.to_string())?;
    runtime.block_on(async move {
        let socket = tokio::net::UnixStream::from_std(socket)
            .map_err(|error| format!("the API socket: {error}"))?;
        let (sender, connection) = hyper::client::conn::http2::handshake(
            hyper_util::rt::TokioExecutor::new(),
            hyper_util::rt::TokioIo::new(socket),
        )
        .await
        .map_err(|error| format!("the API socket: {error}"))?;
        tokio::spawn(async move {
            let _ = connection.await;
            // The launcher is gone, and with it the only way to say anything.
            std::process::exit(EXIT_ABANDONED);
        });
        // SAFETY: the launcher hands this process what `compile` made.
        Ok(unsafe { guest::run(&engine, &compiled, spec.limits(), sender) }.await)
    })
}

/// `runtara-run run`: the exit code.
pub fn run(flags: Flags) -> i32 {
    match run_guest(flags) {
        Ok(guest::Ended::Returned) => {
            report("returned", None);
            EXIT_RETURNED
        }
        Ok(guest::Ended::Failed) => {
            report("failed", None);
            EXIT_GUEST
        }
        Ok(guest::Ended::Trapped(reason)) => {
            report("trapped", Some(&reason));
            EXIT_GUEST
        }
        Ok(guest::Ended::Timeout) => {
            report("timeout", None);
            EXIT_GUEST
        }
        Err(error) => {
            eprintln!("runtara-run: {error}");
            EXIT_REFUSED
        }
    }
}

/// `runtara-run compile`: the exit code. A component that cannot be run is
/// refused with exit 1 and the reason on standard error.
pub fn compile(flags: Flags) -> i32 {
    if let Err(error) = enter(flags, sandbox::Profile::Compile) {
        eprintln!("runtara-run: {error}");
        return EXIT_REFUSED;
    }
    let compiled = read_all(
        inherited(INPUT_DESCRIPTOR),
        MAX_COMPONENT_BYTES,
        "the component",
    )
    .and_then(|wasm| guest::compile(&guest::engine()?, &wasm));
    let written = compiled.and_then(|compiled| {
        inherited(OUTPUT_DESCRIPTOR)
            .write_all(&compiled)
            .map_err(|error| format!("writing the compiled workflow: {error}"))
    });
    match written {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("runtara-run: {error}");
            EXIT_GUEST
        }
    }
}

/// `runtara-run selftest`: applies the sandbox, tries what it must refuse,
/// prints what happened as one JSON line and exits 0 only if all of it was
/// refused and a thread could still be started.
#[cfg(target_os = "linux")]
pub fn selftest(flags: Flags) -> i32 {
    let applied = match sandbox::apply(sandbox::Options {
        profile: sandbox::Profile::Run,
        require_landlock: !flags.no_landlock,
        highest_descriptor: 2,
    }) {
        Ok(applied) => applied,
        Err(error) => {
            eprintln!("runtara-run: sandbox: {error}");
            return EXIT_REFUSED;
        }
    };
    let open = std::fs::File::open("/").is_err();
    // SAFETY: plain system calls with constant arguments.
    let (socket, signal) = unsafe {
        (
            libc::socket(libc::AF_INET, libc::SOCK_STREAM, 0) == -1,
            libc::kill(libc::getppid(), 0) == -1,
        )
    };
    let exec = std::process::Command::new("/bin/sh")
        .args(["-c", "true"])
        .status()
        .is_err();
    let thread = std::thread::spawn(|| ()).join().is_ok();
    let holds = open && socket && signal && exec && thread;
    println!(
        "{}",
        serde_json::json!({
            "holds": holds,
            "landlock": applied.landlock,
            "refused": {"open": open, "socket": socket, "signal": signal, "exec": exec},
            "thread": thread,
        })
    );
    if holds { 0 } else { 1 }
}

/// There is no sandbox to test here.
#[cfg(not(target_os = "linux"))]
pub fn selftest(_flags: Flags) -> i32 {
    eprintln!("runtara-run: the sandbox needs Linux");
    EXIT_REFUSED
}
