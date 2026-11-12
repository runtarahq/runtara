// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! What this process gives up before it reads a byte it does not trust.
//!
//! Applied once, at the start, while the process has one thread:
//!
//! 1. No core dump, a small descriptor table, no new privileges, no
//!    capabilities, not dumpable, and every descriptor above the ones it was
//!    handed closed.
//! 2. Landlock: every filesystem and network right handled and none allowed;
//!    signals and abstract Unix sockets scoped to this process where the
//!    kernel can.
//! 3. seccomp: a list of the system calls a Wasmtime guest and its relay need
//!    (memory, futexes, the event loop, clocks, I/O on descriptors it
//!    already holds, threads). Everything else fails with `EPERM`: no file is
//!    opened, no socket made, nothing executed, no other process signalled.
//!
//! Either layer alone keeps the process from files and the network; they are
//! both applied because each has had bypasses the other did not.
//!
//! Threads started afterwards inherit all of it.

/// What a process needs beyond the common list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Profile {
    /// A run: the guest and its relay.
    Run,
    /// A compile: Cranelift, which writes what it made to a descriptor.
    Compile,
}

/// What [`apply`] is asked to do.
#[derive(Clone, Copy, Debug)]
pub struct Options {
    pub profile: Profile,
    /// Whether a kernel without Landlock is an error. It is unless the
    /// operator said otherwise: seccomp alone still denies files and sockets.
    pub require_landlock: bool,
    /// The highest descriptor the process was handed; the ones above it are
    /// closed.
    pub highest_descriptor: i32,
}

/// What was applied.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Applied {
    /// Whether Landlock enforces every right asked of it (`full`), some
    /// (`partial`: an older kernel), or none.
    pub landlock: &'static str,
    pub seccomp: bool,
}

#[cfg(target_os = "linux")]
pub use linux::apply;

/// There is no sandbox here: only Linux has one.
#[cfg(not(target_os = "linux"))]
pub fn apply(_options: Options) -> Result<Applied, String> {
    Err("the sandbox needs Linux".to_owned())
}

#[cfg(target_os = "linux")]
mod linux {
    use std::collections::BTreeMap;

    use landlock::{
        ABI, Access, AccessFs, AccessNet, CompatLevel, Compatible, Ruleset, RulesetAttr,
        RulesetStatus, Scope,
    };
    use seccompiler::{
        BpfProgram, SeccompAction, SeccompCmpArgLen, SeccompCmpOp, SeccompCondition, SeccompFilter,
        SeccompRule, TargetArch,
    };

    use super::{Applied, Options, Profile};

    fn last_error(what: &str) -> String {
        format!("{what}: {}", std::io::Error::last_os_error())
    }

    fn limit(resource: libc::__rlimit_resource_t, value: u64) -> Result<(), String> {
        let limit = libc::rlimit {
            rlim_cur: value,
            rlim_max: value,
        };
        // SAFETY: `limit` is a valid rlimit for the duration of the call.
        if unsafe { libc::setrlimit(resource, &limit) } != 0 {
            return Err(last_error("setrlimit"));
        }
        Ok(())
    }

    /// Drops every capability, in every set.
    fn drop_capabilities() -> Result<(), String> {
        #[repr(C)]
        struct Header {
            version: u32,
            pid: i32,
        }
        #[repr(C)]
        #[derive(Clone, Copy)]
        struct Data {
            effective: u32,
            permitted: u32,
            inheritable: u32,
        }
        const VERSION_3: u32 = 0x2008_0522;
        let header = Header {
            version: VERSION_3,
            pid: 0,
        };
        let data = [Data {
            effective: 0,
            permitted: 0,
            inheritable: 0,
        }; 2];
        // SAFETY: the header and the two data elements version 3 takes are
        // valid for the call.
        unsafe {
            if libc::prctl(
                libc::PR_CAP_AMBIENT,
                libc::PR_CAP_AMBIENT_CLEAR_ALL,
                0,
                0,
                0,
            ) != 0
            {
                return Err(last_error("clearing ambient capabilities"));
            }
            if libc::syscall(libc::SYS_capset, &header, data.as_ptr()) != 0 {
                return Err(last_error("capset"));
            }
        }
        Ok(())
    }

    fn process(options: Options) -> Result<(), String> {
        limit(libc::RLIMIT_CORE, 0)?;
        limit(libc::RLIMIT_NOFILE, 64)?;
        if options.profile == Profile::Compile {
            // What a compile may write, and how long it may take.
            limit(libc::RLIMIT_FSIZE, 256 * 1024 * 1024)?;
            limit(libc::RLIMIT_CPU, 300)?;
        }
        // SAFETY: plain prctl and close_range calls on this process.
        unsafe {
            if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0 {
                return Err(last_error("no_new_privs"));
            }
            if libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0) != 0 {
                return Err(last_error("clearing dumpable"));
            }
            // Whatever else the launcher left open. A kernel without
            // close_range (before 5.9) leaves them, as before this call.
            let first = (options.highest_descriptor + 1) as libc::c_uint;
            libc::syscall(libc::SYS_close_range, first, libc::c_uint::MAX, 0);
        }
        drop_capabilities()
    }

    fn landlock(require: bool) -> Result<&'static str, String> {
        // The newest ABI this build knows; an older kernel enforces what it
        // has (best effort), and the status says which it was.
        let abi = ABI::V6;
        let status = Ruleset::default()
            .set_compatibility(CompatLevel::BestEffort)
            .handle_access(AccessFs::from_all(abi))
            .and_then(|ruleset| ruleset.handle_access(AccessNet::from_all(abi)))
            .and_then(|ruleset| ruleset.scope(Scope::from_all(abi)))
            .and_then(|ruleset| ruleset.create())
            .and_then(|ruleset| ruleset.restrict_self())
            .map_err(|error| format!("landlock: {error}"))?;
        match status.ruleset {
            RulesetStatus::FullyEnforced => Ok("full"),
            RulesetStatus::PartiallyEnforced => Ok("partial"),
            RulesetStatus::NotEnforced if require => Err(
                "this kernel has no Landlock; the executor's operator may run without it"
                    .to_owned(),
            ),
            RulesetStatus::NotEnforced => Ok("none"),
        }
    }

    fn always() -> Vec<SeccompRule> {
        Vec::new()
    }

    fn when(index: u8, operator: SeccompCmpOp, value: u64) -> Result<Vec<SeccompRule>, String> {
        let condition = SeccompCondition::new(index, SeccompCmpArgLen::Qword, operator, value)
            .map_err(|error| error.to_string())?;
        Ok(vec![
            SeccompRule::new(vec![condition]).map_err(|error| error.to_string())?,
        ])
    }

    fn any(rules: Vec<Vec<SeccompRule>>) -> Vec<SeccompRule> {
        rules.into_iter().flatten().collect()
    }

    fn allowed(profile: Profile) -> Result<BTreeMap<i64, Vec<SeccompRule>>, String> {
        let mut rules = BTreeMap::new();
        for call in [
            // I/O on descriptors the process already holds.
            libc::SYS_read,
            libc::SYS_write,
            libc::SYS_readv,
            libc::SYS_writev,
            libc::SYS_close,
            libc::SYS_fstat,
            libc::SYS_lseek,
            libc::SYS_fcntl,
            libc::SYS_recvfrom,
            libc::SYS_sendto,
            libc::SYS_recvmsg,
            libc::SYS_sendmsg,
            libc::SYS_shutdown,
            // Memory.
            libc::SYS_mmap,
            libc::SYS_munmap,
            libc::SYS_mprotect,
            libc::SYS_mremap,
            libc::SYS_madvise,
            libc::SYS_brk,
            libc::SYS_membarrier,
            // Anonymous memory behind a descriptor: Wasmtime's memory images.
            libc::SYS_memfd_create,
            libc::SYS_ftruncate,
            libc::SYS_pwrite64,
            // Threads and their bookkeeping.
            libc::SYS_futex,
            libc::SYS_set_robust_list,
            libc::SYS_rseq,
            libc::SYS_sched_yield,
            libc::SYS_sched_getaffinity,
            libc::SYS_gettid,
            libc::SYS_getpid,
            libc::SYS_getppid,
            // Signals: Wasmtime turns a guest's fault into a trap.
            libc::SYS_rt_sigaction,
            libc::SYS_rt_sigprocmask,
            libc::SYS_rt_sigreturn,
            libc::SYS_sigaltstack,
            // The event loop.
            libc::SYS_epoll_create1,
            libc::SYS_epoll_ctl,
            libc::SYS_epoll_pwait,
            libc::SYS_epoll_pwait2,
            libc::SYS_eventfd2,
            // Clocks and randomness.
            libc::SYS_clock_gettime,
            libc::SYS_clock_getres,
            libc::SYS_clock_nanosleep,
            libc::SYS_nanosleep,
            libc::SYS_getrandom,
            // Leaving.
            libc::SYS_exit,
            libc::SYS_exit_group,
            libc::SYS_restart_syscall,
        ] {
            rules.insert(call, always());
        }
        #[cfg(target_arch = "x86_64")]
        for call in [libc::SYS_epoll_wait, libc::SYS_gettimeofday, libc::SYS_time] {
            rules.insert(call, always());
        }
        // A thread of this process, and nothing else: no fork, no namespace.
        let thread = libc::CLONE_THREAD as u64;
        rules.insert(
            libc::SYS_clone,
            when(0, SeccompCmpOp::MaskedEq(thread), thread)?,
        );
        // A signal to this process only: an abort, a thread's cancellation.
        rules.insert(
            libc::SYS_tgkill,
            when(0, SeccompCmpOp::Eq, std::process::id() as u64)?,
        );
        // Naming a thread and its memory; nothing that changes what the
        // process may do.
        const PR_SET_VMA: u64 = 0x5356_4d41;
        rules.insert(
            libc::SYS_prctl,
            any(vec![
                when(0, SeccompCmpOp::Eq, libc::PR_SET_NAME as u64)?,
                when(0, SeccompCmpOp::Eq, PR_SET_VMA)?,
            ]),
        );
        // Making a descriptor it holds non-blocking. (The request's type is
        // the C library's: not the same width under each.)
        #[allow(clippy::unnecessary_cast)]
        let non_blocking = libc::FIONBIO as u64;
        rules.insert(libc::SYS_ioctl, when(1, SeccompCmpOp::Eq, non_blocking)?);
        // A pair of sockets joined to each other, which reaches nothing
        // outside this process: the async runtime keeps one for itself.
        rules.insert(
            libc::SYS_socketpair,
            when(0, SeccompCmpOp::Eq, libc::AF_UNIX as u64)?,
        );
        if profile == Profile::Compile {
            // Cranelift's worker threads ask how they are scheduled.
            rules.insert(libc::SYS_sched_getparam, always());
            rules.insert(libc::SYS_sched_getscheduler, always());
        }
        Ok(rules)
    }

    fn program(
        rules: BTreeMap<i64, Vec<SeccompRule>>,
        mismatch: SeccompAction,
        matched: SeccompAction,
    ) -> Result<BpfProgram, String> {
        let arch: TargetArch = std::env::consts::ARCH
            .try_into()
            .map_err(|_| format!("no seccomp filter for {}", std::env::consts::ARCH))?;
        SeccompFilter::new(rules, mismatch, matched, arch)
            .and_then(|filter| filter.try_into())
            .map_err(|error| format!("seccomp: {error}"))
    }

    fn seccomp(profile: Profile) -> Result<(), String> {
        let list = program(
            allowed(profile)?,
            SeccompAction::Errno(libc::EPERM as u32),
            SeccompAction::Allow,
        )?;
        // The C library starts a thread with clone3, whose flags a filter
        // cannot read, and falls back to clone only when told the call does
        // not exist. So, ahead of the list: clone3 answers ENOSYS. (Its
        // number is the same on every architecture this builds for.)
        const LOAD_CALL_NUMBER: u16 = 0x20; // BPF_LD | BPF_W | BPF_ABS
        const JUMP_IF_EQUAL: u16 = 0x15; // BPF_JMP | BPF_JEQ | BPF_K
        const RETURN: u16 = 0x06; // BPF_RET | BPF_K
        const ERRNO: u32 = 0x0005_0000; // SECCOMP_RET_ERRNO
        let instruction = |code, jt, jf, k| seccompiler::sock_filter { code, jt, jf, k };
        let mut filter: BpfProgram = vec![
            instruction(LOAD_CALL_NUMBER, 0, 0, 0),
            instruction(JUMP_IF_EQUAL, 0, 1, libc::SYS_clone3 as u32),
            instruction(RETURN, 0, 0, ERRNO | libc::ENOSYS as u32),
        ];
        filter.extend(list);
        seccompiler::apply_filter(&filter).map_err(|error| format!("seccomp: {error}"))
    }

    /// Applies the sandbox to this process. It must have one thread: what is
    /// applied covers the calling thread and those it starts.
    pub fn apply(options: Options) -> Result<Applied, String> {
        process(options)?;
        let landlock = landlock(options.require_landlock)?;
        seccomp(options.profile)?;
        Ok(Applied {
            landlock,
            seccomp: true,
        })
    }
}
