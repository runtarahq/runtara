// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! See the library's documentation.

use runtara_run::{EXIT_REFUSED, Flags};

const USAGE: &str = "usage: runtara-run <run|compile|selftest> [--unsandboxed] [--no-landlock]
  run       spec on stdin, compiled workflow on descriptor 3, API socket on 4
  compile   component on descriptor 3, compiled workflow written to 4
  selftest  whether the sandbox holds on this host";

fn main() {
    let mut arguments = std::env::args().skip(1);
    let command = arguments.next();
    let mut flags = Flags::default();
    for argument in arguments {
        match argument.as_str() {
            "--unsandboxed" => flags.unsandboxed = true,
            "--no-landlock" => flags.no_landlock = true,
            _ => {
                eprintln!("{USAGE}");
                std::process::exit(EXIT_REFUSED);
            }
        }
    }
    let code = match command.as_deref() {
        Some("run") => runtara_run::run(flags),
        Some("compile") => runtara_run::compile(flags),
        Some("selftest") => runtara_run::selftest(flags),
        _ => {
            eprintln!("{USAGE}");
            EXIT_REFUSED
        }
    };
    std::process::exit(code);
}
