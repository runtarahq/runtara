// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! `runtara-server`, with workflows run by the delegating runner.

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    runtara_server::boot::run(runtara_server::ServerOptions {
        runner_factory: Some(runtara_runner_delegate::factory()),
        ..Default::default()
    })
    .await
}
