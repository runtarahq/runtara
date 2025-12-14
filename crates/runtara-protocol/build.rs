// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
use std::io::Result;

fn main() -> Result<()> {
    // Compile instance protocol (used by instances to communicate with core)
    prost_build::compile_protos(&["proto/instance.proto"], &["proto/"])?;

    // Compile management protocol (internal API for Core, used by Environment for signal proxying)
    prost_build::compile_protos(&["proto/management.proto"], &["proto/"])?;

    // Compile environment protocol (used by management SDK to manage Environment)
    prost_build::compile_protos(&["proto/environment.proto"], &["proto/"])?;

    Ok(())
}
