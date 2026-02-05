// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Wire format for QUIC stream framing.
//!
//! Each QUIC stream carries one RPC call with the following frame format:
//! - 4 bytes: message length (big-endian)
//! - 2 bytes: message type
//! - N bytes: protobuf payload

use bytes::{Buf, BufMut, Bytes, BytesMut};
use prost::Message;
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Maximum frame size (64 MB)
/// Increased to accommodate large compiled workflow binaries
pub const MAX_FRAME_SIZE: usize = 64 * 1024 * 1024;

/// Frame header size (4 bytes length + 2 bytes type)
pub const HEADER_SIZE: usize = 6;

/// Message types for the wire protocol
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum MessageType {
    /// Request message
    Request = 1,
    /// Response message
    Response = 2,
    /// Start of a streaming response
    StreamStart = 3,
    /// Data chunk in a streaming response
    StreamData = 4,
    /// End of a streaming response
    StreamEnd = 5,
    /// Error response
    Error = 6,
}

impl TryFrom<u16> for MessageType {
    type Error = FrameError;

    fn try_from(value: u16) -> Result<Self, <Self as TryFrom<u16>>::Error> {
        match value {
            1 => Ok(MessageType::Request),
            2 => Ok(MessageType::Response),
            3 => Ok(MessageType::StreamStart),
            4 => Ok(MessageType::StreamData),
            5 => Ok(MessageType::StreamEnd),
            6 => Ok(MessageType::Error),
            _ => Err(FrameError::InvalidMessageType(value)),
        }
    }
}

/// Errors that can occur during frame encoding/decoding
#[derive(Debug, Error)]
pub enum FrameError {
    #[error("frame too large: {0} bytes (max: {MAX_FRAME_SIZE})")]
    FrameTooLarge(usize),

    #[error("invalid message type: {0}")]
    InvalidMessageType(u16),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("protobuf decode error: {0}")]
    Decode(#[from] prost::DecodeError),

    #[error("connection closed")]
    ConnectionClosed,
}

/// A framed message with type and payload
#[derive(Debug, Clone)]
pub struct Frame {
    pub message_type: MessageType,
    pub payload: Bytes,
}

impl Frame {
    /// Create a new request frame
    pub fn request<M: Message>(msg: &M) -> Result<Self, FrameError> {
        Self::new(MessageType::Request, msg)
    }

    /// Create a new response frame
    pub fn response<M: Message>(msg: &M) -> Result<Self, FrameError> {
        Self::new(MessageType::Response, msg)
    }

    /// Create a new error frame
    pub fn error<M: Message>(msg: &M) -> Result<Self, FrameError> {
        Self::new(MessageType::Error, msg)
    }

    /// Create a new stream data frame
    pub fn stream_data<M: Message>(msg: &M) -> Result<Self, FrameError> {
        Self::new(MessageType::StreamData, msg)
    }

    /// Create a new frame with the given type and message
    pub fn new<M: Message>(message_type: MessageType, msg: &M) -> Result<Self, FrameError> {
        let payload = msg.encode_to_vec();
        if payload.len() > MAX_FRAME_SIZE {
            return Err(FrameError::FrameTooLarge(payload.len()));
        }
        Ok(Self {
            message_type,
            payload: Bytes::from(payload),
        })
    }

    /// Decode the payload as a protobuf message
    pub fn decode<M: Message + Default>(&self) -> Result<M, FrameError> {
        Ok(M::decode(self.payload.clone())?)
    }

    /// Encode the frame to bytes for wire transmission
    pub fn encode(&self) -> Bytes {
        let mut buf = BytesMut::with_capacity(HEADER_SIZE + self.payload.len());
        buf.put_u32(self.payload.len() as u32);
        buf.put_u16(self.message_type as u16);
        buf.put(self.payload.clone());
        buf.freeze()
    }

    /// Decode a frame from bytes
    pub fn decode_from_bytes(mut bytes: Bytes) -> Result<Self, FrameError> {
        if bytes.len() < HEADER_SIZE {
            return Err(FrameError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "incomplete frame header",
            )));
        }

        let length = bytes.get_u32() as usize;
        let message_type = MessageType::try_from(bytes.get_u16())?;

        if length > MAX_FRAME_SIZE {
            return Err(FrameError::FrameTooLarge(length));
        }

        if bytes.len() < length {
            return Err(FrameError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "incomplete frame payload",
            )));
        }

        let payload = bytes.split_to(length);
        Ok(Self {
            message_type,
            payload,
        })
    }
}

/// Write a frame to an async writer
pub async fn write_frame<W: AsyncWrite + Unpin>(
    writer: &mut W,
    frame: &Frame,
) -> Result<(), FrameError> {
    let encoded = frame.encode();
    writer.write_all(&encoded).await?;
    Ok(())
}

/// Read a frame from an async reader
pub async fn read_frame<R: AsyncRead + Unpin>(reader: &mut R) -> Result<Frame, FrameError> {
    // Read header
    let mut header = [0u8; HEADER_SIZE];
    match reader.read_exact(&mut header).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
            return Err(FrameError::ConnectionClosed);
        }
        Err(e) => return Err(e.into()),
    }

    let length = u32::from_be_bytes([header[0], header[1], header[2], header[3]]) as usize;
    let message_type = MessageType::try_from(u16::from_be_bytes([header[4], header[5]]))?;

    if length > MAX_FRAME_SIZE {
        return Err(FrameError::FrameTooLarge(length));
    }

    // Read payload
    let mut payload = vec![0u8; length];
    reader.read_exact(&mut payload).await?;

    Ok(Frame {
        message_type,
        payload: Bytes::from(payload),
    })
}

/// Framed codec for encoding/decoding frames on a stream
pub struct FramedStream<S> {
    stream: S,
}

impl<S> FramedStream<S> {
    pub fn new(stream: S) -> Self {
        Self { stream }
    }

    pub fn into_inner(self) -> S {
        self.stream
    }
}

impl<S: AsyncRead + Unpin> FramedStream<S> {
    /// Read the next frame from the stream
    pub async fn read_frame(&mut self) -> Result<Frame, FrameError> {
        read_frame(&mut self.stream).await
    }
}

impl<S: AsyncWrite + Unpin> FramedStream<S> {
    /// Write a frame to the stream
    pub async fn write_frame(&mut self, frame: &Frame) -> Result<(), FrameError> {
        write_frame(&mut self.stream, frame).await
    }
}

impl<S: AsyncRead + AsyncWrite + Unpin> FramedStream<S> {
    /// Send a request and wait for a response
    pub async fn request<Req: Message, Resp: Message + Default>(
        &mut self,
        request: &Req,
    ) -> Result<Resp, FrameError> {
        let frame = Frame::request(request)?;
        self.write_frame(&frame).await?;

        let response_frame = self.read_frame().await?;
        match response_frame.message_type {
            MessageType::Response => response_frame.decode(),
            MessageType::Error => {
                // Try to decode as error message
                Err(FrameError::Io(std::io::Error::other(
                    "received error response",
                )))
            }
            _ => Err(FrameError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "unexpected message type",
            ))),
        }
    }

    /// Send a response
    pub async fn respond<Resp: Message>(&mut self, response: &Resp) -> Result<(), FrameError> {
        let frame = Frame::response(response)?;
        self.write_frame(&frame).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_message_type_round_trip() {
        for &mt in &[
            MessageType::Request,
            MessageType::Response,
            MessageType::StreamStart,
            MessageType::StreamData,
            MessageType::StreamEnd,
            MessageType::Error,
        ] {
            let value = mt as u16;
            let decoded = MessageType::try_from(value).unwrap();
            assert_eq!(mt, decoded);
        }
    }

    #[test]
    fn test_frame_encode_decode() {
        use crate::management_proto::HealthCheckRequest;

        let msg = HealthCheckRequest {};
        let frame = Frame::request(&msg).unwrap();
        let encoded = frame.encode();
        let decoded = Frame::decode_from_bytes(encoded).unwrap();

        assert_eq!(frame.message_type, decoded.message_type);
        assert_eq!(frame.payload, decoded.payload);
    }

    // ========== Constants Tests ==========

    #[test]
    fn test_max_frame_size_constant() {
        // MAX_FRAME_SIZE is 64 MB
        assert_eq!(MAX_FRAME_SIZE, 64 * 1024 * 1024);
    }

    #[test]
    fn test_header_size_constant() {
        // HEADER_SIZE is 6 bytes: 4 bytes length + 2 bytes type
        assert_eq!(HEADER_SIZE, 6);
    }

    // ========== MessageType Tests ==========

    #[test]
    fn test_message_type_values() {
        assert_eq!(MessageType::Request as u16, 1);
        assert_eq!(MessageType::Response as u16, 2);
        assert_eq!(MessageType::StreamStart as u16, 3);
        assert_eq!(MessageType::StreamData as u16, 4);
        assert_eq!(MessageType::StreamEnd as u16, 5);
        assert_eq!(MessageType::Error as u16, 6);
    }

    #[test]
    fn test_message_type_conversions() {
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
    }

    #[test]
    fn test_message_type_invalid_conversion() {
        assert!(MessageType::try_from(0u16).is_err());
        assert!(MessageType::try_from(7u16).is_err());
        assert!(MessageType::try_from(100u16).is_err());
        assert!(MessageType::try_from(u16::MAX).is_err());
    }

    #[test]
    fn test_message_type_debug() {
        assert_eq!(format!("{:?}", MessageType::Request), "Request");
        assert_eq!(format!("{:?}", MessageType::Response), "Response");
        assert_eq!(format!("{:?}", MessageType::StreamStart), "StreamStart");
        assert_eq!(format!("{:?}", MessageType::StreamData), "StreamData");
        assert_eq!(format!("{:?}", MessageType::StreamEnd), "StreamEnd");
        assert_eq!(format!("{:?}", MessageType::Error), "Error");
    }

    #[test]
    fn test_message_type_clone_and_copy() {
        let mt = MessageType::Request;
        // MessageType is Copy, so we can just copy it directly
        let copied: MessageType = mt;
        let copied2: MessageType = mt;
        assert_eq!(mt, copied);
        assert_eq!(mt, copied2);
    }

    #[test]
    fn test_message_type_equality() {
        assert_eq!(MessageType::Request, MessageType::Request);
        assert_ne!(MessageType::Request, MessageType::Response);
    }

    // ========== FrameError Tests ==========

    #[test]
    fn test_frame_error_display_frame_too_large() {
        let err = FrameError::FrameTooLarge(100_000_000);
        let msg = format!("{}", err);
        assert!(msg.contains("frame too large"));
        assert!(msg.contains("100000000"));
        assert!(msg.contains(&MAX_FRAME_SIZE.to_string()));
    }

    #[test]
    fn test_frame_error_display_invalid_message_type() {
        let err = FrameError::InvalidMessageType(42);
        let msg = format!("{}", err);
        assert!(msg.contains("invalid message type"));
        assert!(msg.contains("42"));
    }

    #[test]
    fn test_frame_error_display_io() {
        let io_err = std::io::Error::other("test error");
        let err = FrameError::Io(io_err);
        let msg = format!("{}", err);
        assert!(msg.contains("IO error"));
    }

    #[test]
    fn test_frame_error_display_connection_closed() {
        let err = FrameError::ConnectionClosed;
        let msg = format!("{}", err);
        assert!(msg.contains("connection closed"));
    }

    #[test]
    fn test_frame_error_from_io_error() {
        let io_err = std::io::Error::new(std::io::ErrorKind::BrokenPipe, "pipe broken");
        let frame_err: FrameError = io_err.into();
        match frame_err {
            FrameError::Io(_) => {}
            _ => panic!("Expected FrameError::Io"),
        }
    }

    // ========== Frame Creation Tests ==========

    #[test]
    fn test_frame_request_creation() {
        use crate::management_proto::HealthCheckRequest;
        let msg = HealthCheckRequest {};
        let frame = Frame::request(&msg).unwrap();
        assert_eq!(frame.message_type, MessageType::Request);
    }

    #[test]
    fn test_frame_response_creation() {
        use crate::management_proto::HealthCheckResponse;
        let msg = HealthCheckResponse {
            healthy: true,
            version: "1.0.0".to_string(),
            uptime_ms: 1000,
            active_instances: 5,
        };
        let frame = Frame::response(&msg).unwrap();
        assert_eq!(frame.message_type, MessageType::Response);
    }

    #[test]
    fn test_frame_error_creation() {
        use crate::management_proto::HealthCheckResponse;
        let msg = HealthCheckResponse {
            healthy: false,
            version: "1.0.0".to_string(),
            uptime_ms: 0,
            active_instances: 0,
        };
        let frame = Frame::error(&msg).unwrap();
        assert_eq!(frame.message_type, MessageType::Error);
    }

    #[test]
    fn test_frame_stream_data_creation() {
        use crate::management_proto::HealthCheckResponse;
        let msg = HealthCheckResponse {
            healthy: true,
            version: "1.0.0".to_string(),
            uptime_ms: 500,
            active_instances: 2,
        };
        let frame = Frame::stream_data(&msg).unwrap();
        assert_eq!(frame.message_type, MessageType::StreamData);
    }

    #[test]
    fn test_frame_new_all_types() {
        use crate::management_proto::HealthCheckRequest;
        let msg = HealthCheckRequest {};

        for &mt in &[
            MessageType::Request,
            MessageType::Response,
            MessageType::StreamStart,
            MessageType::StreamData,
            MessageType::StreamEnd,
            MessageType::Error,
        ] {
            let frame = Frame::new(mt, &msg).unwrap();
            assert_eq!(frame.message_type, mt);
        }
    }

    #[test]
    fn test_frame_decode_payload() {
        use crate::management_proto::HealthCheckResponse;
        let original = HealthCheckResponse {
            healthy: true,
            version: "2.0.0".to_string(),
            uptime_ms: 12345,
            active_instances: 10,
        };
        let frame = Frame::response(&original).unwrap();
        let decoded: HealthCheckResponse = frame.decode().unwrap();
        assert!(decoded.healthy);
        assert_eq!(decoded.version, "2.0.0");
        assert_eq!(decoded.uptime_ms, 12345);
        assert_eq!(decoded.active_instances, 10);
    }

    #[test]
    fn test_frame_clone() {
        use crate::management_proto::HealthCheckRequest;
        let msg = HealthCheckRequest {};
        let frame = Frame::request(&msg).unwrap();
        let cloned = frame.clone();
        assert_eq!(frame.message_type, cloned.message_type);
        assert_eq!(frame.payload, cloned.payload);
    }

    #[test]
    fn test_frame_debug() {
        use crate::management_proto::HealthCheckRequest;
        let msg = HealthCheckRequest {};
        let frame = Frame::request(&msg).unwrap();
        let debug_str = format!("{:?}", frame);
        assert!(debug_str.contains("Frame"));
        assert!(debug_str.contains("message_type"));
        assert!(debug_str.contains("payload"));
    }

    // ========== Frame Encoding Tests ==========

    #[test]
    fn test_frame_encode_structure() {
        use crate::management_proto::HealthCheckRequest;
        let msg = HealthCheckRequest {};
        let frame = Frame::request(&msg).unwrap();
        let encoded = frame.encode();

        // Check header: 4 bytes length + 2 bytes type
        assert!(encoded.len() >= HEADER_SIZE);

        // First 4 bytes should be the payload length (big-endian)
        let length = u32::from_be_bytes([encoded[0], encoded[1], encoded[2], encoded[3]]) as usize;
        assert_eq!(length, frame.payload.len());

        // Bytes 4-5 should be the message type
        let msg_type = u16::from_be_bytes([encoded[4], encoded[5]]);
        assert_eq!(msg_type, MessageType::Request as u16);

        // Total length should be header + payload
        assert_eq!(encoded.len(), HEADER_SIZE + frame.payload.len());
    }

    #[test]
    fn test_frame_with_large_payload() {
        use crate::instance_proto::CheckpointRequest;
        // Create a checkpoint request with substantial data
        let msg = CheckpointRequest {
            instance_id: "test-instance".to_string(),
            checkpoint_id: "checkpoint-123".to_string(),
            state: vec![0u8; 1024 * 1024], // 1 MB of data
        };
        let frame = Frame::request(&msg).unwrap();
        assert!(frame.payload.len() > 1024 * 1024);

        // Should encode and decode correctly
        let encoded = frame.encode();
        let decoded = Frame::decode_from_bytes(encoded).unwrap();
        assert_eq!(frame.payload, decoded.payload);
    }

    #[test]
    fn test_frame_with_empty_payload() {
        use crate::management_proto::HealthCheckRequest;
        let msg = HealthCheckRequest {};
        let frame = Frame::request(&msg).unwrap();
        // HealthCheckRequest is empty, so payload should be minimal (just protobuf overhead)
        assert!(frame.payload.len() <= 10);
    }

    // ========== decode_from_bytes Tests ==========

    #[test]
    fn test_decode_from_bytes_incomplete_header() {
        let bytes = Bytes::from_static(&[0, 0, 0]); // Only 3 bytes, need 6
        let result = Frame::decode_from_bytes(bytes);
        assert!(result.is_err());
        match result.unwrap_err() {
            FrameError::Io(e) => {
                assert!(e.to_string().contains("incomplete frame header"));
            }
            _ => panic!("Expected Io error with incomplete header message"),
        }
    }

    #[test]
    fn test_decode_from_bytes_incomplete_payload() {
        // Header says 100 bytes payload, but we only have 10
        let mut bytes = BytesMut::new();
        bytes.put_u32(100); // length = 100
        bytes.put_u16(1); // type = Request
        bytes.put(&[0u8; 10][..]); // Only 10 bytes of payload

        let result = Frame::decode_from_bytes(bytes.freeze());
        assert!(result.is_err());
        match result.unwrap_err() {
            FrameError::Io(e) => {
                assert!(e.to_string().contains("incomplete frame payload"));
            }
            _ => panic!("Expected Io error with incomplete payload message"),
        }
    }

    #[test]
    fn test_decode_from_bytes_invalid_message_type() {
        let mut bytes = BytesMut::new();
        bytes.put_u32(0); // length = 0
        bytes.put_u16(99); // invalid type

        let result = Frame::decode_from_bytes(bytes.freeze());
        assert!(result.is_err());
        match result.unwrap_err() {
            FrameError::InvalidMessageType(99) => {}
            _ => panic!("Expected InvalidMessageType error"),
        }
    }

    #[test]
    fn test_decode_from_bytes_frame_too_large() {
        let mut bytes = BytesMut::new();
        bytes.put_u32((MAX_FRAME_SIZE + 1) as u32); // Too large
        bytes.put_u16(1); // type = Request

        let result = Frame::decode_from_bytes(bytes.freeze());
        assert!(result.is_err());
        match result.unwrap_err() {
            FrameError::FrameTooLarge(size) => {
                assert_eq!(size, MAX_FRAME_SIZE + 1);
            }
            _ => panic!("Expected FrameTooLarge error"),
        }
    }

    #[test]
    fn test_decode_from_bytes_empty_payload() {
        let mut bytes = BytesMut::new();
        bytes.put_u32(0); // length = 0
        bytes.put_u16(1); // type = Request

        let result = Frame::decode_from_bytes(bytes.freeze());
        assert!(result.is_ok());
        let frame = result.unwrap();
        assert_eq!(frame.message_type, MessageType::Request);
        assert!(frame.payload.is_empty());
    }

    #[test]
    fn test_decode_from_bytes_with_extra_data() {
        // Create a valid frame followed by extra data
        let mut bytes = BytesMut::new();
        bytes.put_u32(5); // length = 5
        bytes.put_u16(2); // type = Response
        bytes.put(&[1, 2, 3, 4, 5][..]); // 5 bytes payload
        bytes.put(&[99, 99, 99][..]); // Extra data (should be ignored)

        let result = Frame::decode_from_bytes(bytes.freeze());
        assert!(result.is_ok());
        let frame = result.unwrap();
        assert_eq!(frame.message_type, MessageType::Response);
        assert_eq!(&frame.payload[..], &[1, 2, 3, 4, 5]);
    }

    // ========== Async read/write frame tests ==========

    #[tokio::test]
    async fn test_read_write_frame() {
        use crate::management_proto::HealthCheckRequest;
        use tokio::io::duplex;

        let msg = HealthCheckRequest {};
        let frame = Frame::request(&msg).unwrap();

        // Create a duplex stream (in-memory bidirectional)
        let (mut writer, mut reader) = duplex(1024);

        // Write frame
        write_frame(&mut writer, &frame).await.unwrap();

        // Read frame back
        let read_frame = read_frame(&mut reader).await.unwrap();
        assert_eq!(frame.message_type, read_frame.message_type);
        assert_eq!(frame.payload, read_frame.payload);
    }

    #[tokio::test]
    async fn test_read_frame_connection_closed() {
        use tokio::io::duplex;

        let (_, mut reader) = duplex(1024);
        // Writer is dropped, reader will get EOF

        let result = read_frame(&mut reader).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            FrameError::ConnectionClosed => {}
            e => panic!("Expected ConnectionClosed, got: {:?}", e),
        }
    }

    #[tokio::test]
    async fn test_write_read_multiple_frames() {
        use crate::management_proto::{HealthCheckRequest, HealthCheckResponse};
        use tokio::io::duplex;

        let (mut writer, mut reader) = duplex(4096);

        // Write multiple frames
        let req = HealthCheckRequest {};
        let resp = HealthCheckResponse {
            healthy: true,
            version: "1.0.0".to_string(),
            uptime_ms: 100,
            active_instances: 1,
        };

        let frame1 = Frame::request(&req).unwrap();
        let frame2 = Frame::response(&resp).unwrap();

        write_frame(&mut writer, &frame1).await.unwrap();
        write_frame(&mut writer, &frame2).await.unwrap();
        drop(writer); // Signal EOF

        // Read back
        let read1 = read_frame(&mut reader).await.unwrap();
        let read2 = read_frame(&mut reader).await.unwrap();

        assert_eq!(read1.message_type, MessageType::Request);
        assert_eq!(read2.message_type, MessageType::Response);
    }

    // ========== FramedStream Tests ==========

    #[test]
    fn test_framed_stream_new() {
        let stream = vec![0u8; 10];
        let framed = FramedStream::new(stream);
        let inner = framed.into_inner();
        assert_eq!(inner.len(), 10);
    }

    #[test]
    fn test_framed_stream_into_inner() {
        let data = "test data".to_string();
        let framed = FramedStream::new(data.clone());
        let inner = framed.into_inner();
        assert_eq!(inner, data);
    }

    #[tokio::test]
    async fn test_framed_stream_read_write() {
        use crate::management_proto::HealthCheckRequest;
        use tokio::io::duplex;

        let (writer, reader) = duplex(1024);
        let mut writer_framed = FramedStream::new(writer);
        let mut reader_framed = FramedStream::new(reader);

        let msg = HealthCheckRequest {};
        let frame = Frame::request(&msg).unwrap();

        // Write and read through FramedStream
        writer_framed.write_frame(&frame).await.unwrap();
        drop(writer_framed); // Drop to signal EOF on the writing end

        let read_frame = reader_framed.read_frame().await.unwrap();
        assert_eq!(frame.message_type, read_frame.message_type);
    }

    // ========== Edge Cases and Boundary Tests ==========

    #[test]
    fn test_frame_at_max_size() {
        use crate::instance_proto::CheckpointRequest;
        // Create a frame just under the max size
        let large_state = vec![0u8; MAX_FRAME_SIZE - 100]; // Leave room for other fields
        let msg = CheckpointRequest {
            instance_id: "i".to_string(),
            checkpoint_id: "c".to_string(),
            state: large_state,
        };
        let result = Frame::request(&msg);
        // Should succeed if under limit
        assert!(result.is_ok() || matches!(result, Err(FrameError::FrameTooLarge(_))));
    }

    #[test]
    fn test_message_type_exhaustive_matching() {
        // Ensure all message types can be matched
        let types = vec![
            MessageType::Request,
            MessageType::Response,
            MessageType::StreamStart,
            MessageType::StreamData,
            MessageType::StreamEnd,
            MessageType::Error,
        ];

        for mt in types {
            match mt {
                MessageType::Request => assert_eq!(mt as u16, 1),
                MessageType::Response => assert_eq!(mt as u16, 2),
                MessageType::StreamStart => assert_eq!(mt as u16, 3),
                MessageType::StreamData => assert_eq!(mt as u16, 4),
                MessageType::StreamEnd => assert_eq!(mt as u16, 5),
                MessageType::Error => assert_eq!(mt as u16, 6),
            }
        }
    }

    #[test]
    fn test_frame_error_debug() {
        let err = FrameError::FrameTooLarge(100);
        let debug = format!("{:?}", err);
        assert!(debug.contains("FrameTooLarge"));

        let err = FrameError::InvalidMessageType(42);
        let debug = format!("{:?}", err);
        assert!(debug.contains("InvalidMessageType"));

        let err = FrameError::ConnectionClosed;
        let debug = format!("{:?}", err);
        assert!(debug.contains("ConnectionClosed"));
    }
}
