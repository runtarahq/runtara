// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later

use std::{env, fs, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../runtara-agents/Cargo.toml");
    println!("cargo:rerun-if-changed=../runtara-agents/src");
    println!("cargo:rerun-if-changed=../runtara-dsl/Cargo.toml");
    println!("cargo:rerun-if-changed=../runtara-dsl/src");

    let mut agents = runtara_agents::registry::get_agents();
    agents.sort_by(|a, b| a.id.cmp(&b.id));
    for agent in &mut agents {
        agent.capabilities.sort_by(|a, b| a.id.cmp(&b.id));
    }

    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR must be set"));
    let output_path = out_dir.join("agents.json");
    let json = serde_json::to_string(&agents).expect("agent metadata must serialize to JSON");
    fs::write(output_path, json).expect("failed to write generated agent metadata");
}
