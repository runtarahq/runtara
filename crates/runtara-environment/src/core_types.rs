// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Domain values rendered for management responses and telemetry.

use runtara_core::domain::{EventType, InstanceStatus};

pub(crate) fn status_name(value: InstanceStatus) -> &'static str {
    match value {
        InstanceStatus::Pending => "pending",
        InstanceStatus::Running => "running",
        InstanceStatus::Suspended => "suspended",
        InstanceStatus::Completed => "completed",
        InstanceStatus::Failed => "failed",
        InstanceStatus::Cancelled => "cancelled",
    }
}

pub(crate) fn event_name(value: EventType) -> &'static str {
    match value {
        EventType::Started => "started",
        EventType::Progress => "progress",
        EventType::Heartbeat => "heartbeat",
        EventType::Completed => "completed",
        EventType::Failed => "failed",
        EventType::Suspended => "suspended",
        EventType::Custom => "custom",
    }
}
