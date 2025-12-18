// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Signal conversion utilities.

use crate::types::{Signal, SignalType};
use runtara_protocol::instance_proto as proto;

/// Convert a proto signal to an SDK signal.
pub(crate) fn from_proto_signal(signal: proto::Signal) -> Signal {
    Signal {
        signal_type: SignalType::from(signal.signal_type),
        payload: signal.payload,
        checkpoint_id: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_from_proto_signal() {
        let proto_signal = proto::Signal {
            instance_id: "test".to_string(),
            signal_type: proto::SignalType::SignalCancel.into(),
            payload: b"reason".to_vec(),
        };

        let signal = from_proto_signal(proto_signal);
        assert_eq!(signal.signal_type, SignalType::Cancel);
        assert_eq!(signal.payload, b"reason".to_vec());
        assert!(signal.checkpoint_id.is_none());
    }
}
