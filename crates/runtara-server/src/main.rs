// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    runtara_server::boot::run(runtara_server::ServerOptions::default()).await
}
