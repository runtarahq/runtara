//! Fixed-size protocol for killable component precompilation workers.
//!
//! Wasmtime component compilation is synchronous and cannot be cancelled by
//! dropping a `spawn_blocking` join handle. The Environment can instead run
//! this protocol in a short-lived child process: killing that child stops its
//! filesystem read and compiler work without touching an active guest runner.
//!
//! This module deliberately uses a small binary frame rather than a
//! deserialize-to-`Vec` format. Lengths are checked before allocation on both
//! stdin and stdout, so a broken child cannot turn a preparation timeout into
//! an unbounded parent-memory allocation.

use std::hash::{Hash, Hasher};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use wasmtime::{Engine, component::Component};

use crate::{EngineConfig, build_engine};

/// Exact byte length of a request nonce and source digest.
pub const PRECOMPILE_NONCE_BYTES: usize = 32;

/// Exact private argv switch that puts the server binary into precompile-worker
/// mode. Both the Environment parent and server child use this shared value so
/// the internal protocol cannot drift into a normal server startup.
pub const PRECOMPILE_WORKER_ARGUMENT: &str = "--internal-precompile-component";

/// Largest UTF-8 artifact path accepted on the worker protocol.
pub const MAX_PRECOMPILE_ARTIFACT_PATH_BYTES: usize = 16 * 1024;

/// Largest raw component the precompiler may read.
///
/// This protects the child from an unbounded filesystem allocation. A small
/// preparation pool can have more than one child, and each child also needs
/// Wasmtime compiler memory, so keep this deliberately conservative for the
/// 4 GiB development/production hosts rather than sizing it to an arbitrary
/// uploaded artifact.
pub const MAX_PRECOMPILE_COMPONENT_BYTES: usize = 64 * 1024 * 1024;

/// Largest serialized Wasmtime component the protocol may return.
///
/// Native code can expand beyond its input component, so it has a separate cap
/// from [`MAX_PRECOMPILE_COMPONENT_BYTES`]. The parent receives this payload
/// too, so a two-child preparation pool has a bounded protocol-buffer ceiling
/// instead of allowing a few malformed/large images to consume the host.
pub const MAX_PRECOMPILED_COMPONENT_BYTES: usize = 128 * 1024 * 1024;

/// Largest diagnostic returned by a failed child.
pub const MAX_PRECOMPILE_FAILURE_BYTES: usize = 8 * 1024;

const MAGIC: [u8; 8] = *b"RTRPC001";
const VERSION: u16 = 1;
const HEADER_BYTES: usize = MAGIC.len() + 2 + 1 + 1 + 8;
const REQUEST_PREFIX_BYTES: usize = PRECOMPILE_NONCE_BYTES;
const SUCCESS_PREFIX_BYTES: usize = PRECOMPILE_NONCE_BYTES * 4;
const FAILURE_PREFIX_BYTES: usize = PRECOMPILE_NONCE_BYTES;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
enum FrameKind {
    Request = 1,
    Success = 2,
    Failure = 3,
}

impl TryFrom<u8> for FrameKind {
    type Error = anyhow::Error;

    fn try_from(value: u8) -> Result<Self> {
        match value {
            1 => Ok(Self::Request),
            2 => Ok(Self::Success),
            3 => Ok(Self::Failure),
            _ => bail!("unknown precompile protocol frame kind {value}"),
        }
    }
}

#[derive(Debug)]
struct FrameHeader {
    kind: FrameKind,
    payload_len: usize,
}

/// An artifact path and launch nonce sent to a precompile child.
///
/// The child, rather than the parent runner, reads the artifact, computes its
/// SHA-256 digest, and performs Wasmtime compilation. This keeps all potentially
/// blocking filesystem and compiler work inside a killable process boundary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrecompileRequest {
    nonce: [u8; PRECOMPILE_NONCE_BYTES],
    artifact_path: PathBuf,
}

impl PrecompileRequest {
    /// Build a request for the image artifact at `artifact_path`.
    ///
    /// Paths are deliberately carried as UTF-8 because this is a local
    /// process protocol, not a persistence format, and workflow image paths
    /// are rendered/logged as UTF-8 throughout Environment already.
    pub fn for_artifact(
        nonce: [u8; PRECOMPILE_NONCE_BYTES],
        artifact_path: impl AsRef<Path>,
    ) -> Result<Self> {
        let artifact_path = artifact_path.as_ref();
        let path = artifact_path
            .to_str()
            .context("precompile worker artifact path is not valid UTF-8")?;
        ensure!(
            !path.is_empty(),
            "precompile worker artifact path must not be empty"
        );
        ensure!(
            path.len() <= MAX_PRECOMPILE_ARTIFACT_PATH_BYTES,
            "precompile worker artifact path exceeds the configured limit"
        );
        Ok(Self {
            nonce,
            artifact_path: artifact_path.to_path_buf(),
        })
    }

    /// Per-launch nonce echoed by the child response.
    pub const fn nonce(&self) -> [u8; PRECOMPILE_NONCE_BYTES] {
        self.nonce
    }

    /// Image artifact path the child reads and compiles.
    pub fn artifact_path(&self) -> &Path {
        &self.artifact_path
    }

    fn path_bytes(&self) -> Result<&[u8]> {
        let path = self
            .artifact_path
            .to_str()
            .context("precompile worker artifact path is not valid UTF-8")?;
        ensure!(
            !path.is_empty(),
            "precompile worker artifact path must not be empty"
        );
        ensure!(
            path.len() <= MAX_PRECOMPILE_ARTIFACT_PATH_BYTES,
            "precompile worker artifact path exceeds the configured limit"
        );
        Ok(path.as_bytes())
    }
}

/// Successful response returned by a precompile child.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrecompiledComponent {
    nonce: [u8; PRECOMPILE_NONCE_BYTES],
    source_digest: [u8; PRECOMPILE_NONCE_BYTES],
    engine_fingerprint: [u8; PRECOMPILE_NONCE_BYTES],
    serialized_digest: [u8; PRECOMPILE_NONCE_BYTES],
    serialized_component: Vec<u8>,
}

impl PrecompiledComponent {
    /// Nonce echoed from the request this artifact belongs to.
    pub const fn nonce(&self) -> [u8; PRECOMPILE_NONCE_BYTES] {
        self.nonce
    }

    /// SHA-256 of the exact source artifact the child read and compiled.
    ///
    /// The durable dispatcher compares this to `workflow.binaryChecksum` for
    /// generated immutable workflow images before accepting the result.
    pub const fn source_digest(&self) -> [u8; PRECOMPILE_NONCE_BYTES] {
        self.source_digest
    }

    /// Fingerprint of the Wasmtime precompile-compatible engine configuration.
    ///
    /// This is an early, explicit configuration fence. Wasmtime also validates
    /// compatibility while deserializing the artifact.
    pub const fn engine_fingerprint(&self) -> [u8; PRECOMPILE_NONCE_BYTES] {
        self.engine_fingerprint
    }

    /// SHA-256 of the serialized component bytes for transport-integrity checks.
    pub const fn serialized_digest(&self) -> [u8; PRECOMPILE_NONCE_BYTES] {
        self.serialized_digest
    }

    /// Serialized Wasmtime component bytes produced by the child.
    pub fn serialized_component(&self) -> &[u8] {
        &self.serialized_component
    }

    fn validate(&self) -> Result<()> {
        ensure!(
            !self.serialized_component.is_empty(),
            "precompile response serialized component must not be empty"
        );
        ensure!(
            self.serialized_component.len() <= MAX_PRECOMPILED_COMPONENT_BYTES,
            "precompile response serialized component exceeds the configured limit"
        );
        ensure!(
            digest(&self.serialized_component) == self.serialized_digest,
            "precompile response serialized component digest does not match its bytes"
        );
        Ok(())
    }
}

/// A bounded failure response from a child.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrecompileFailure {
    nonce: [u8; PRECOMPILE_NONCE_BYTES],
    message: String,
}

impl PrecompileFailure {
    /// Nonce echoed from the request this failure belongs to.
    pub const fn nonce(&self) -> [u8; PRECOMPILE_NONCE_BYTES] {
        self.nonce
    }

    /// Child diagnostic, capped by [`MAX_PRECOMPILE_FAILURE_BYTES`].
    pub fn message(&self) -> &str {
        &self.message
    }
}

/// One framed child response.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PrecompileResponse {
    /// The native serialized component is ready for trusted deserialization.
    Success(PrecompiledComponent),
    /// Reading, hashing, validation, or compilation failed before a result existed.
    Failure(PrecompileFailure),
}

/// Write one bounded precompile request to a child stdin pipe.
pub fn write_precompile_request<W: Write>(
    writer: &mut W,
    request: &PrecompileRequest,
) -> Result<()> {
    let path = request.path_bytes()?;
    let payload_len = checked_payload_len(REQUEST_PREFIX_BYTES, path.len())?;
    write_header(writer, FrameKind::Request, payload_len)?;
    writer.write_all(&request.nonce)?;
    writer.write_all(path)?;
    writer.flush()?;
    Ok(())
}

/// Read one bounded precompile request from a child stdin pipe.
pub fn read_precompile_request<R: Read>(reader: &mut R) -> Result<PrecompileRequest> {
    let header = read_header(reader)?;
    ensure!(
        header.kind == FrameKind::Request,
        "expected precompile request frame, got {:?}",
        header.kind
    );
    let path_len = checked_body_len(
        header.payload_len,
        REQUEST_PREFIX_BYTES,
        MAX_PRECOMPILE_ARTIFACT_PATH_BYTES,
        "artifact path",
    )?;
    let nonce = read_array(reader)?;
    let path = String::from_utf8(read_vec(reader, path_len)?)
        .context("precompile worker artifact path is not valid UTF-8")?;
    PrecompileRequest::for_artifact(nonce, path)
}

/// Asynchronously write one bounded precompile request to a child stdin pipe.
///
/// Environment uses this directly with `tokio::process::ChildStdin`, so the
/// parent never needs a blocking adapter for pre-run filesystem or compiler
/// preparation.
pub async fn write_precompile_request_async<W: AsyncWrite + Unpin>(
    writer: &mut W,
    request: &PrecompileRequest,
) -> Result<()> {
    let path = request.path_bytes()?;
    let payload_len = checked_payload_len(REQUEST_PREFIX_BYTES, path.len())?;
    write_header_async(writer, FrameKind::Request, payload_len).await?;
    writer.write_all(&request.nonce).await?;
    writer.write_all(path).await?;
    writer.flush().await?;
    Ok(())
}

/// Asynchronously read one bounded precompile request from a pipe.
pub async fn read_precompile_request_async<R: AsyncRead + Unpin>(
    reader: &mut R,
) -> Result<PrecompileRequest> {
    let header = read_header_async(reader).await?;
    ensure!(
        header.kind == FrameKind::Request,
        "expected precompile request frame, got {:?}",
        header.kind
    );
    let path_len = checked_body_len(
        header.payload_len,
        REQUEST_PREFIX_BYTES,
        MAX_PRECOMPILE_ARTIFACT_PATH_BYTES,
        "artifact path",
    )?;
    let nonce = read_array_async(reader).await?;
    let path = String::from_utf8(read_vec_async(reader, path_len).await?)
        .context("precompile worker artifact path is not valid UTF-8")?;
    PrecompileRequest::for_artifact(nonce, path)
}

/// Write one bounded precompile response to a parent stdout pipe.
pub fn write_precompile_response<W: Write>(
    writer: &mut W,
    response: &PrecompileResponse,
) -> Result<()> {
    match response {
        PrecompileResponse::Success(success) => {
            success.validate()?;
            let payload_len =
                checked_payload_len(SUCCESS_PREFIX_BYTES, success.serialized_component.len())?;
            write_header(writer, FrameKind::Success, payload_len)?;
            writer.write_all(&success.nonce)?;
            writer.write_all(&success.source_digest)?;
            writer.write_all(&success.engine_fingerprint)?;
            writer.write_all(&success.serialized_digest)?;
            writer.write_all(&success.serialized_component)?;
        }
        PrecompileResponse::Failure(failure) => {
            let message = failure.message.as_bytes();
            ensure!(
                message.len() <= MAX_PRECOMPILE_FAILURE_BYTES,
                "precompile failure message exceeds the configured limit"
            );
            let payload_len = checked_payload_len(FAILURE_PREFIX_BYTES, message.len())?;
            write_header(writer, FrameKind::Failure, payload_len)?;
            writer.write_all(&failure.nonce)?;
            writer.write_all(message)?;
        }
    }
    writer.flush()?;
    Ok(())
}

/// Read one bounded precompile response from a child stdout pipe.
pub fn read_precompile_response<R: Read>(reader: &mut R) -> Result<PrecompileResponse> {
    let header = read_header(reader)?;
    match header.kind {
        FrameKind::Success => {
            let component_len = checked_body_len(
                header.payload_len,
                SUCCESS_PREFIX_BYTES,
                MAX_PRECOMPILED_COMPONENT_BYTES,
                "serialized component",
            )?;
            let nonce = read_array(reader)?;
            let source_digest = read_array(reader)?;
            let engine_fingerprint = read_array(reader)?;
            let serialized_digest = read_array(reader)?;
            let serialized_component = read_vec(reader, component_len)?;
            let success = PrecompiledComponent {
                nonce,
                source_digest,
                engine_fingerprint,
                serialized_digest,
                serialized_component,
            };
            success.validate()?;
            Ok(PrecompileResponse::Success(success))
        }
        FrameKind::Failure => {
            let message_len = checked_body_len(
                header.payload_len,
                FAILURE_PREFIX_BYTES,
                MAX_PRECOMPILE_FAILURE_BYTES,
                "failure message",
            )?;
            let nonce = read_array(reader)?;
            let message = String::from_utf8(read_vec(reader, message_len)?)
                .context("precompile failure message is not valid UTF-8")?;
            Ok(PrecompileResponse::Failure(PrecompileFailure {
                nonce,
                message,
            }))
        }
        FrameKind::Request => bail!("expected precompile response frame, got request"),
    }
}

/// Asynchronously write one bounded response to a parent stdout pipe.
pub async fn write_precompile_response_async<W: AsyncWrite + Unpin>(
    writer: &mut W,
    response: &PrecompileResponse,
) -> Result<()> {
    match response {
        PrecompileResponse::Success(success) => {
            success.validate()?;
            let payload_len =
                checked_payload_len(SUCCESS_PREFIX_BYTES, success.serialized_component.len())?;
            write_header_async(writer, FrameKind::Success, payload_len).await?;
            writer.write_all(&success.nonce).await?;
            writer.write_all(&success.source_digest).await?;
            writer.write_all(&success.engine_fingerprint).await?;
            writer.write_all(&success.serialized_digest).await?;
            writer.write_all(&success.serialized_component).await?;
        }
        PrecompileResponse::Failure(failure) => {
            let message = failure.message.as_bytes();
            ensure!(
                message.len() <= MAX_PRECOMPILE_FAILURE_BYTES,
                "precompile failure message exceeds the configured limit"
            );
            let payload_len = checked_payload_len(FAILURE_PREFIX_BYTES, message.len())?;
            write_header_async(writer, FrameKind::Failure, payload_len).await?;
            writer.write_all(&failure.nonce).await?;
            writer.write_all(message).await?;
        }
    }
    writer.flush().await?;
    Ok(())
}

/// Asynchronously read one bounded precompile response from a child stdout pipe.
///
/// This streams the frame into at most the configured serialized-artifact cap;
/// it never uses `wait_with_output`, which would grow without a protocol bound.
pub async fn read_precompile_response_async<R: AsyncRead + Unpin>(
    reader: &mut R,
) -> Result<PrecompileResponse> {
    let header = read_header_async(reader).await?;
    match header.kind {
        FrameKind::Success => {
            let component_len = checked_body_len(
                header.payload_len,
                SUCCESS_PREFIX_BYTES,
                MAX_PRECOMPILED_COMPONENT_BYTES,
                "serialized component",
            )?;
            let nonce = read_array_async(reader).await?;
            let source_digest = read_array_async(reader).await?;
            let engine_fingerprint = read_array_async(reader).await?;
            let serialized_digest = read_array_async(reader).await?;
            let serialized_component = read_vec_async(reader, component_len).await?;
            let success = PrecompiledComponent {
                nonce,
                source_digest,
                engine_fingerprint,
                serialized_digest,
                serialized_component,
            };
            success.validate()?;
            Ok(PrecompileResponse::Success(success))
        }
        FrameKind::Failure => {
            let message_len = checked_body_len(
                header.payload_len,
                FAILURE_PREFIX_BYTES,
                MAX_PRECOMPILE_FAILURE_BYTES,
                "failure message",
            )?;
            let nonce = read_array_async(reader).await?;
            let message = String::from_utf8(read_vec_async(reader, message_len).await?)
                .context("precompile failure message is not valid UTF-8")?;
            Ok(PrecompileResponse::Failure(PrecompileFailure {
                nonce,
                message,
            }))
        }
        FrameKind::Request => bail!("expected precompile response frame, got request"),
    }
}

/// Read, hash, and compile the requested artifact in a killable child process.
///
/// The child creates the same default engine configuration as the long-lived
/// server. Wasmtime records configuration compatibility inside serialized
/// artifacts; the parent must still validate the nonce and deserialize only
/// from this process-private protocol before use.
pub fn precompile_artifact(request: &PrecompileRequest) -> Result<PrecompiledComponent> {
    let component = read_bounded_artifact(request.artifact_path())?;
    let source_digest = digest(&component);
    let engine =
        build_engine(&EngineConfig::default()).context("build precompile worker engine")?;
    let serialized_component = engine
        .precompile_component(&component)
        .map_err(|error| anyhow::anyhow!("precompile workflow component: {error:#}"))?;
    ensure!(
        !serialized_component.is_empty(),
        "wasmtime returned an empty serialized component"
    );
    ensure!(
        serialized_component.len() <= MAX_PRECOMPILED_COMPONENT_BYTES,
        "serialized component is {} bytes; limit is {} bytes",
        serialized_component.len(),
        MAX_PRECOMPILED_COMPONENT_BYTES
    );
    Ok(PrecompiledComponent {
        nonce: request.nonce(),
        source_digest,
        engine_fingerprint: precompile_engine_fingerprint(&engine),
        serialized_digest: digest(&serialized_component),
        serialized_component,
    })
}

/// Run the child-side half of the protocol once.
///
/// A valid request that fails reading or compilation receives a framed
/// [`PrecompileResponse::Failure`] before this function returns the same
/// error. A malformed request has no trustworthy nonce to echo, so it simply
/// returns an error and the child exits nonzero.
pub fn run_precompile_worker<R: Read, W: Write>(reader: &mut R, writer: &mut W) -> Result<()> {
    let request = read_precompile_request(reader)?;
    match precompile_artifact(&request) {
        Ok(success) => write_precompile_response(writer, &PrecompileResponse::Success(success)),
        Err(error) => {
            let response = PrecompileResponse::Failure(PrecompileFailure {
                nonce: request.nonce(),
                message: bounded_error_message(&error),
            });
            write_precompile_response(writer, &response)
                .context("write precompile worker failure response")?;
            Err(error)
        }
    }
}

/// Validate that a child reply belongs to exactly this request.
///
/// This is deliberately safe but does not make the serialized bytes trusted:
/// a nonce establishes launch identity and the serialized digest establishes
/// transport integrity, not the provenance required by Wasmtime's unsafe
/// deserialize API.
pub fn validate_precompile_response<'a>(
    request: &PrecompileRequest,
    response: &'a PrecompileResponse,
) -> Result<&'a PrecompiledComponent> {
    match response {
        PrecompileResponse::Success(success) => {
            success.validate()?;
            ensure!(
                success.nonce == request.nonce(),
                "precompile response nonce does not match its request"
            );
            Ok(success)
        }
        PrecompileResponse::Failure(failure) => {
            ensure!(
                failure.nonce == request.nonce(),
                "precompile failure nonce does not match its request"
            );
            bail!("precompile worker failed: {}", failure.message)
        }
    }
}

/// Verify that a child artifact was precompiled for this engine configuration.
///
/// This provides an inexpensive diagnostic before the narrow unsafe
/// deserialization boundary. Wasmtime repeats the authoritative compatibility
/// validation during deserialize, so an engine upgrade/configuration drift is
/// rejected even if this fingerprint scheme changes in a later release.
pub fn ensure_precompile_engine_compatible(
    engine: &Engine,
    component: &PrecompiledComponent,
) -> Result<()> {
    ensure!(
        component.engine_fingerprint == precompile_engine_fingerprint(engine),
        "precompile response engine configuration does not match the parent engine"
    );
    Ok(())
}

/// Deserialize a serialized component received from a trusted child process.
///
/// # Safety
///
/// `response` must have arrived unchanged over a process-private pipe from
/// this crate's [`run_precompile_worker`], or otherwise consist exactly of
/// bytes previously emitted by Wasmtime's `Engine::precompile_component` or
/// `Component::serialize`. The nonce and serialized-digest checks below are
/// necessary for launch identity and transport integrity, but they cannot
/// prove arbitrary bytes are a safe Wasmtime serialized artifact.
pub unsafe fn deserialize_trusted_precompiled_component(
    engine: &Engine,
    request: &PrecompileRequest,
    response: &PrecompileResponse,
) -> Result<Component> {
    let success = validate_precompile_response(request, response)?;
    ensure_precompile_engine_compatible(engine, success)?;
    // SAFETY: upheld by this function's caller contract. The child side only
    // returns `Engine::precompile_component` output and the parent must keep
    // the stdout pipe private from untrusted writers.
    unsafe { Component::deserialize(engine, success.serialized_component()) }.map_err(|error| {
        anyhow::anyhow!("deserialize trusted precompiled workflow component: {error:#}")
    })
}

fn read_bounded_artifact(path: &Path) -> Result<Vec<u8>> {
    let file = std::fs::File::open(path)
        .with_context(|| format!("open workflow artifact {}", path.display()))?;
    let metadata = file
        .metadata()
        .with_context(|| format!("stat workflow artifact {}", path.display()))?;
    ensure!(
        metadata.is_file(),
        "workflow artifact {} is not a regular file",
        path.display()
    );
    ensure!(
        metadata.len() <= u64::try_from(MAX_PRECOMPILE_COMPONENT_BYTES).unwrap_or(u64::MAX),
        "workflow artifact {} is {} bytes; limit is {} bytes",
        path.display(),
        metadata.len(),
        MAX_PRECOMPILE_COMPONENT_BYTES
    );
    let mut reader = file.take(
        u64::try_from(MAX_PRECOMPILE_COMPONENT_BYTES)
            .unwrap_or(u64::MAX)
            .saturating_add(1),
    );
    let mut component = Vec::with_capacity(
        usize::try_from(metadata.len())
            .unwrap_or(MAX_PRECOMPILE_COMPONENT_BYTES)
            .min(MAX_PRECOMPILE_COMPONENT_BYTES),
    );
    reader
        .read_to_end(&mut component)
        .with_context(|| format!("read workflow artifact {}", path.display()))?;
    ensure!(
        !component.is_empty(),
        "workflow artifact {} is empty",
        path.display()
    );
    ensure!(
        component.len() <= MAX_PRECOMPILE_COMPONENT_BYTES,
        "workflow artifact {} exceeds the configured limit while being read",
        path.display()
    );
    Ok(component)
}

fn digest(bytes: &[u8]) -> [u8; PRECOMPILE_NONCE_BYTES] {
    Sha256::digest(bytes).into()
}

/// Hash adapter used to turn Wasmtime's opaque compatibility hash into a
/// fixed-width protocol field without relying on a non-contractual standard
/// library hasher implementation.
#[derive(Default)]
struct CompatibilityHasher(Sha256);

impl Hasher for CompatibilityHasher {
    fn finish(&self) -> u64 {
        let digest = self.0.clone().finalize();
        u64::from_be_bytes(digest[..8].try_into().expect("SHA-256 prefix"))
    }

    fn write(&mut self, bytes: &[u8]) {
        self.0.update(bytes);
    }
}

fn precompile_engine_fingerprint(engine: &Engine) -> [u8; PRECOMPILE_NONCE_BYTES] {
    let mut hasher = CompatibilityHasher::default();
    engine.precompile_compatibility_hash().hash(&mut hasher);
    hasher.0.finalize().into()
}

fn bounded_error_message(error: &anyhow::Error) -> String {
    let mut message = format!("{error:#}");
    if message.len() > MAX_PRECOMPILE_FAILURE_BYTES {
        let mut end = MAX_PRECOMPILE_FAILURE_BYTES;
        while !message.is_char_boundary(end) {
            end -= 1;
        }
        message.truncate(end);
    }
    message
}

fn checked_payload_len(prefix_len: usize, body_len: usize) -> Result<usize> {
    prefix_len
        .checked_add(body_len)
        .context("precompile protocol payload length overflow")
}

fn checked_body_len(
    payload_len: usize,
    prefix_len: usize,
    max_body_len: usize,
    label: &str,
) -> Result<usize> {
    let body_len = payload_len
        .checked_sub(prefix_len)
        .with_context(|| format!("precompile {label} payload is too short"))?;
    ensure!(
        body_len <= max_body_len,
        "precompile {label} is {body_len} bytes; limit is {max_body_len} bytes"
    );
    Ok(body_len)
}

fn write_header<W: Write>(writer: &mut W, kind: FrameKind, payload_len: usize) -> Result<()> {
    let payload_len = u64::try_from(payload_len).context("precompile payload does not fit u64")?;
    writer.write_all(&MAGIC)?;
    writer.write_all(&VERSION.to_be_bytes())?;
    writer.write_all(&[kind as u8, 0])?;
    writer.write_all(&payload_len.to_be_bytes())?;
    Ok(())
}

fn read_header<R: Read>(reader: &mut R) -> Result<FrameHeader> {
    let mut header = [0_u8; HEADER_BYTES];
    reader.read_exact(&mut header)?;
    ensure!(
        header[..MAGIC.len()] == MAGIC,
        "invalid precompile protocol magic"
    );
    let version_offset = MAGIC.len();
    let version = u16::from_be_bytes([header[version_offset], header[version_offset + 1]]);
    ensure!(
        version == VERSION,
        "unsupported precompile protocol version {version}"
    );
    let kind_offset = version_offset + 2;
    ensure!(
        header[kind_offset + 1] == 0,
        "invalid nonzero precompile protocol reserved byte"
    );
    let length_offset = kind_offset + 2;
    let mut length_bytes = [0_u8; 8];
    length_bytes.copy_from_slice(&header[length_offset..]);
    let payload_len = usize::try_from(u64::from_be_bytes(length_bytes))
        .context("precompile payload length does not fit this platform")?;
    Ok(FrameHeader {
        kind: FrameKind::try_from(header[kind_offset])?,
        payload_len,
    })
}

async fn write_header_async<W: AsyncWrite + Unpin>(
    writer: &mut W,
    kind: FrameKind,
    payload_len: usize,
) -> Result<()> {
    let payload_len = u64::try_from(payload_len).context("precompile payload does not fit u64")?;
    writer.write_all(&MAGIC).await?;
    writer.write_all(&VERSION.to_be_bytes()).await?;
    writer.write_all(&[kind as u8, 0]).await?;
    writer.write_all(&payload_len.to_be_bytes()).await?;
    Ok(())
}

async fn read_header_async<R: AsyncRead + Unpin>(reader: &mut R) -> Result<FrameHeader> {
    let mut header = [0_u8; HEADER_BYTES];
    reader.read_exact(&mut header).await?;
    ensure!(
        header[..MAGIC.len()] == MAGIC,
        "invalid precompile protocol magic"
    );
    let version_offset = MAGIC.len();
    let version = u16::from_be_bytes([header[version_offset], header[version_offset + 1]]);
    ensure!(
        version == VERSION,
        "unsupported precompile protocol version {version}"
    );
    let kind_offset = version_offset + 2;
    ensure!(
        header[kind_offset + 1] == 0,
        "invalid nonzero precompile protocol reserved byte"
    );
    let length_offset = kind_offset + 2;
    let mut length_bytes = [0_u8; 8];
    length_bytes.copy_from_slice(&header[length_offset..]);
    let payload_len = usize::try_from(u64::from_be_bytes(length_bytes))
        .context("precompile payload length does not fit this platform")?;
    Ok(FrameHeader {
        kind: FrameKind::try_from(header[kind_offset])?,
        payload_len,
    })
}

fn read_array<R: Read>(reader: &mut R) -> Result<[u8; PRECOMPILE_NONCE_BYTES]> {
    let mut bytes = [0_u8; PRECOMPILE_NONCE_BYTES];
    reader.read_exact(&mut bytes)?;
    Ok(bytes)
}

fn read_vec<R: Read>(reader: &mut R, len: usize) -> Result<Vec<u8>> {
    let mut bytes = vec![0_u8; len];
    reader.read_exact(&mut bytes)?;
    Ok(bytes)
}

async fn read_array_async<R: AsyncRead + Unpin>(
    reader: &mut R,
) -> Result<[u8; PRECOMPILE_NONCE_BYTES]> {
    let mut bytes = [0_u8; PRECOMPILE_NONCE_BYTES];
    reader.read_exact(&mut bytes).await?;
    Ok(bytes)
}

async fn read_vec_async<R: AsyncRead + Unpin>(reader: &mut R, len: usize) -> Result<Vec<u8>> {
    let mut bytes = vec![0_u8; len];
    reader.read_exact(&mut bytes).await?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn component_file() -> (tempfile::TempDir, PathBuf, Vec<u8>) {
        let dir = tempfile::tempdir().expect("temporary artifact directory");
        let path = dir.path().join("workflow.wasm");
        let bytes = wat::parse_str("(component)").expect("minimal component WAT parses");
        std::fs::write(&path, &bytes).expect("write component");
        (dir, path, bytes)
    }

    #[test]
    fn request_round_trip_preserves_path_and_nonce() {
        let request = PrecompileRequest::for_artifact(
            [7; PRECOMPILE_NONCE_BYTES],
            "/tmp/runtara-workflow.wasm",
        )
        .expect("request");
        let mut wire = Vec::new();
        write_precompile_request(&mut wire, &request).expect("write request");
        let actual = read_precompile_request(&mut wire.as_slice()).expect("read request");
        assert_eq!(actual, request);
    }

    #[test]
    fn frame_lengths_are_checked_before_allocating() {
        let mut wire = Vec::new();
        write_header(
            &mut wire,
            FrameKind::Request,
            REQUEST_PREFIX_BYTES + MAX_PRECOMPILE_ARTIFACT_PATH_BYTES + 1,
        )
        .expect("header");
        assert!(read_precompile_request(&mut wire.as_slice()).is_err());

        let mut wire = Vec::new();
        write_header(
            &mut wire,
            FrameKind::Success,
            SUCCESS_PREFIX_BYTES + MAX_PRECOMPILED_COMPONENT_BYTES + 1,
        )
        .expect("header");
        assert!(read_precompile_response(&mut wire.as_slice()).is_err());
    }

    #[test]
    fn artifact_above_operational_cap_is_rejected_before_reading() {
        let dir = tempfile::tempdir().expect("temporary artifact directory");
        let path = dir.path().join("too-large.wasm");
        let file = std::fs::File::create(&path).expect("create sparse artifact");
        file.set_len(u64::try_from(MAX_PRECOMPILE_COMPONENT_BYTES + 1).expect("u64 size"))
            .expect("make sparse artifact larger than cap");

        let error = read_bounded_artifact(&path).expect_err("oversized artifact is rejected");
        assert!(error.to_string().contains("limit"));
    }

    #[tokio::test]
    async fn async_protocol_round_trip_uses_bounded_pipe_io() {
        let request = PrecompileRequest::for_artifact(
            [5; PRECOMPILE_NONCE_BYTES],
            "/tmp/runtara-workflow.wasm",
        )
        .expect("request");
        let (mut writer, mut reader) = tokio::io::duplex(1024);
        let expected = request.clone();
        let writer = tokio::spawn(async move {
            write_precompile_request_async(&mut writer, &request)
                .await
                .expect("write request")
        });
        let actual = read_precompile_request_async(&mut reader)
            .await
            .expect("read request");
        writer.await.expect("writer task");
        assert_eq!(actual, expected);
    }

    #[test]
    fn worker_reads_hashes_precompiles_and_deserializes_a_component() {
        let (_dir, path, source) = component_file();
        let request =
            PrecompileRequest::for_artifact([9; PRECOMPILE_NONCE_BYTES], &path).expect("request");
        let mut input = Vec::new();
        write_precompile_request(&mut input, &request).expect("write request");
        let mut output = Vec::new();
        run_precompile_worker(&mut input.as_slice(), &mut output).expect("worker succeeds");
        let response = read_precompile_response(&mut output.as_slice()).expect("read response");
        let success = validate_precompile_response(&request, &response).expect("response matches");
        assert_eq!(success.source_digest(), digest(&source));
        let engine = build_engine(&EngineConfig::default()).expect("parent engine");
        // SAFETY: this test routes the response directly from the local
        // `run_precompile_worker` through in-memory private buffers.
        unsafe { deserialize_trusted_precompiled_component(&engine, &request, &response) }
            .expect("deserialize trusted precompile output");
    }

    #[test]
    fn worker_returns_a_bounded_failure_frame_for_invalid_components() {
        let dir = tempfile::tempdir().expect("temporary artifact directory");
        let path = dir.path().join("invalid.wasm");
        std::fs::write(&path, [0, 1, 2]).expect("write invalid component");
        let request =
            PrecompileRequest::for_artifact([11; PRECOMPILE_NONCE_BYTES], &path).expect("request");
        let mut input = Vec::new();
        write_precompile_request(&mut input, &request).expect("write request");
        let mut output = Vec::new();
        assert!(run_precompile_worker(&mut input.as_slice(), &mut output).is_err());
        let response = read_precompile_response(&mut output.as_slice()).expect("read failure");
        assert!(matches!(response, PrecompileResponse::Failure(_)));
        assert!(validate_precompile_response(&request, &response).is_err());
    }
}
