// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! The compile recurses with the longest step chain in the graph, and WASI
//! has no thread to give it a large stack: the main thread gets one.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("wasi") {
        // As runtara-compile's (see runtara-workflows' build.rs).
        println!("cargo:rustc-link-arg-bin=runtara-compile-guest=-zstack-size=33554432");
        // The validator walks the same graphs.
        println!("cargo:rustc-link-arg-bin=runtara-validate-guest=-zstack-size=33554432");
    }
}
