// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Built for WASI (to compile in a sandbox), `runtara-compile` has no thread
//! to give the compile its large stack, so its main thread gets one: the
//! compile recurses with the longest step chain in the graph.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("wasi") {
        // 32 MiB: some 16 times the 2 MiB a native release build overflowed
        // between 400 and 800 chained steps on.
        println!("cargo:rustc-link-arg-bin=runtara-compile=-zstack-size=33554432");
    }
}
