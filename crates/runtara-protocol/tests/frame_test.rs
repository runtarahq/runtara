// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Frame encoding/decoding tests for runtara-protocol.

use bytes::Bytes;
use prost::Message;
use runtara_protocol::frame::{Frame, FrameError, HEADER_SIZE, MAX_FRAME_SIZE, MessageType};
use runtara_protocol::management_proto::HealthCheckRequest;

#[test]
fn test_message_type_conversions() {
    // Valid message types
    assert_eq!(MessageType::try_from(1u16).unwrap(), MessageType::Request);
    assert_eq!(MessageType::try_from(2u16).unwrap(), MessageType::Response);
    assert_eq!(
        MessageType::try_from(3u16).unwrap(),
        MessageType::StreamStart
    );
    assert_eq!(
        MessageType::try_from(4u16).unwrap(),
        MessageType::StreamData
    );
    assert_eq!(MessageType::try_from(5u16).unwrap(), MessageType::StreamEnd);
    assert_eq!(MessageType::try_from(6u16).unwrap(), MessageType::Error);

    // Invalid message types
    assert!(MessageType::try_from(0u16).is_err());
    assert!(MessageType::try_from(7u16).is_err());
    assert!(MessageType::try_from(100u16).is_err());
}

#[test]
fn test_frame_request_creation() {
    let msg = HealthCheckRequest {};
    let frame = Frame::request(&msg).unwrap();

    assert_eq!(frame.message_type, MessageType::Request);
    assert!(!frame.payload.is_empty() || msg.encoded_len() == 0);
}

#[test]
fn test_frame_response_creation() {
    let msg = HealthCheckRequest {};
    let frame = Frame::response(&msg).unwrap();

    assert_eq!(frame.message_type, MessageType::Response);
}

#[test]
fn test_frame_error_creation() {
    let msg = HealthCheckRequest {};
    let frame = Frame::error(&msg).unwrap();

    assert_eq!(frame.message_type, MessageType::Error);
}

#[test]
fn test_frame_stream_data_creation() {
    let msg = HealthCheckRequest {};
    let frame = Frame::stream_data(&msg).unwrap();

    assert_eq!(frame.message_type, MessageType::StreamData);
}

#[test]
fn test_frame_encode_decode_roundtrip() {
    let msg = HealthCheckRequest {};
    let original_frame = Frame::request(&msg).unwrap();

    let encoded = original_frame.encode();
    let decoded_frame = Frame::decode_from_bytes(encoded).unwrap();

    assert_eq!(original_frame.message_type, decoded_frame.message_type);
    assert_eq!(original_frame.payload, decoded_frame.payload);
}

#[test]
fn test_frame_header_format() {
    let msg = HealthCheckRequest {};
    let frame = Frame::request(&msg).unwrap();
    let encoded = frame.encode();

    // First 4 bytes are length (big-endian)
    let length = u32::from_be_bytes([encoded[0], encoded[1], encoded[2], encoded[3]]) as usize;
    assert_eq!(length, frame.payload.len());

    // Next 2 bytes are message type (big-endian)
    let msg_type = u16::from_be_bytes([encoded[4], encoded[5]]);
    assert_eq!(msg_type, MessageType::Request as u16);

    // Total size should be header + payload
    assert_eq!(encoded.len(), HEADER_SIZE + frame.payload.len());
}

#[test]
fn test_frame_decode_incomplete_header() {
    // Less than 6 bytes
    let incomplete = Bytes::from_static(&[0, 0, 0, 10, 0]); // only 5 bytes
    let result = Frame::decode_from_bytes(incomplete);

    assert!(matches!(result, Err(FrameError::Io(_))));
}

#[test]
fn test_frame_decode_incomplete_payload() {
    // Header says 100 bytes but only 10 provided
    let mut data = vec![0, 0, 0, 100]; // length = 100
    data.extend_from_slice(&[0, 1]); // type = Request
    data.extend_from_slice(&[0u8; 10]); // only 10 bytes of payload

    let result = Frame::decode_from_bytes(Bytes::from(data));
    assert!(matches!(result, Err(FrameError::Io(_))));
}

#[test]
fn test_frame_decode_invalid_message_type() {
    let mut data = vec![0, 0, 0, 0]; // length = 0
    data.extend_from_slice(&[0, 99]); // type = 99 (invalid)

    let result = Frame::decode_from_bytes(Bytes::from(data));
    assert!(matches!(result, Err(FrameError::InvalidMessageType(99))));
}

#[test]
fn test_frame_decode_empty_payload() {
    let mut data = vec![0, 0, 0, 0]; // length = 0
    data.extend_from_slice(&[0, 1]); // type = Request

    let frame = Frame::decode_from_bytes(Bytes::from(data)).unwrap();
    assert_eq!(frame.message_type, MessageType::Request);
    assert!(frame.payload.is_empty());
}

#[test]
fn test_frame_with_large_payload() {
    // Create a frame with 1KB payload
    let payload = vec![0u8; 1024];
    let frame = Frame {
        message_type: MessageType::StreamData,
        payload: Bytes::from(payload.clone()),
    };

    let encoded = frame.encode();
    let decoded = Frame::decode_from_bytes(encoded).unwrap();

    assert_eq!(decoded.payload.len(), 1024);
    assert_eq!(decoded.payload.as_ref(), payload.as_slice());
}

#[test]
fn test_max_frame_size_constant() {
    // Verify the constant is 64MB
    assert_eq!(MAX_FRAME_SIZE, 64 * 1024 * 1024);
}

#[test]
fn test_header_size_constant() {
    // Verify header is 6 bytes (4 length + 2 type)
    assert_eq!(HEADER_SIZE, 6);
}

#[test]
fn test_message_type_values() {
    assert_eq!(MessageType::Request as u16, 1);
    assert_eq!(MessageType::Response as u16, 2);
    assert_eq!(MessageType::StreamStart as u16, 3);
    assert_eq!(MessageType::StreamData as u16, 4);
    assert_eq!(MessageType::StreamEnd as u16, 5);
    assert_eq!(MessageType::Error as u16, 6);
}

#[tokio::test]
async fn test_read_write_frame() {
    use runtara_protocol::frame::{read_frame, write_frame};
    use tokio::io::BufWriter;

    let msg = HealthCheckRequest {};
    let original_frame = Frame::request(&msg).unwrap();

    // Write to a buffer
    let mut buffer = Vec::new();
    let mut writer = BufWriter::new(&mut buffer);
    write_frame(&mut writer, &original_frame).await.unwrap();

    // Flush to ensure all data is written
    tokio::io::AsyncWriteExt::flush(&mut writer).await.unwrap();
    drop(writer);

    // Read back
    let mut reader = buffer.as_slice();
    let read_back = read_frame(&mut reader).await.unwrap();

    assert_eq!(original_frame.message_type, read_back.message_type);
    assert_eq!(original_frame.payload, read_back.payload);
}

#[test]
fn test_frame_decode_protobuf() {
    // Create a frame with a known message
    let msg = HealthCheckRequest {};
    let frame = Frame::request(&msg).unwrap();

    // Decode the payload back to the message
    let decoded: HealthCheckRequest = frame.decode().unwrap();

    // HealthCheckRequest is empty, so just verify it decodes
    assert_eq!(decoded, msg);
}
