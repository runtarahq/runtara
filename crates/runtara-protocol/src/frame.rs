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
}
