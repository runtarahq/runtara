//! Versioned, self-contained child-component catalog shared by compiler and host.
//!
//! A final root custom section contains a bounded JSON index followed by unique
//! component bytes, sorted by SHA-256. Entry bindings reference those digests.
//! Parsing borrows artifact bodies without copying them. This checks framing and
//! integrity, not component validity/types: the precompiler validates every
//! component before any Store may invoke it. Native serialized code is not accepted.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

mod invocation_manifest;
mod invocation_path;
pub use invocation_manifest::{AgentCallSite, InvocationCallSite, InvocationManifest};
pub use invocation_path::{
    AgentInvocationPath, InvocationPathError, InvocationSelector, LoopFrame, LoopKind,
    NamespaceFrame,
};

pub const SECTION_NAME: &str = "runtara:isolated-package@1";
const COMPONENT_HEADER: &[u8; 8] = b"\0asm\x0d\0\x01\0";

#[derive(Clone, Copy, Debug)]
pub struct PackageLimits {
    pub total_bytes: usize,
    pub manifest_bytes: usize,
    pub artifacts: usize,
    pub bindings: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    pub id: String,
    pub artifact: String,
    pub interface: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Artifact {
    digest: String,
    offset: usize,
    length: usize,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    version: u32,
    artifacts: Vec<Artifact>,
    bindings: Vec<Binding>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    invocations: Option<InvocationManifest>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PackageError {
    NotComponent,
    InvalidFraming,
    AlreadyPackaged,
    InvalidManifest,
    UnsupportedVersion,
    LimitExceeded,
    InvalidLayout,
    DigestMismatch,
    DuplicateBinding,
    MissingArtifact,
}

impl std::fmt::Display for PackageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "isolated component package: {self:?}")
    }
}
impl std::error::Error for PackageError {}

pub fn artifact_digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub struct ParsedPackage<'a> {
    /// Root component without the catalog; legacy artifacts are never rewritten.
    pub root: &'a [u8],
    bindings: BTreeMap<String, Binding>,
    artifacts: BTreeMap<String, &'a [u8]>,
    invocations: Option<InvocationManifest>,
}

impl<'a> ParsedPackage<'a> {
    /// Compiler call-site authority covered by the enclosing raw artifact digest.
    pub fn invocations(&self) -> Option<&InvocationManifest> {
        self.invocations.as_ref()
    }
    pub fn bindings(&self) -> &BTreeMap<String, Binding> {
        &self.bindings
    }

    pub fn artifacts(&self) -> &BTreeMap<String, &'a [u8]> {
        &self.artifacts
    }

    pub fn resolve(&self, id: &str) -> Option<(&Binding, &'a [u8])> {
        let binding = self.bindings.get(id)?;
        Some((binding, *self.artifacts.get(&binding.artifact)?))
    }
}

fn read_u32(bytes: &[u8], cursor: &mut usize) -> Result<u32, PackageError> {
    let mut value = 0;
    for shift in (0..35).step_by(7) {
        let byte = *bytes.get(*cursor).ok_or(PackageError::InvalidFraming)?;
        *cursor += 1;
        if shift == 28 && byte > 15 {
            return Err(PackageError::InvalidFraming);
        }
        value |= u32::from(byte & 127) << shift;
        if byte < 128 {
            return Ok(value);
        }
    }
    Err(PackageError::InvalidFraming)
}

fn write_u32(mut value: u32, output: &mut Vec<u8>) {
    loop {
        let byte = (value & 127) as u8;
        value >>= 7;
        output.push(byte | if value == 0 { 0 } else { 128 });
        if value == 0 {
            return;
        }
    }
}

/// Only top-level sections are scanned. A child component's custom section can
/// never masquerade as the root catalog. The v1 catalog must be the final section.
fn catalog_section(bytes: &[u8]) -> Result<Option<(usize, &[u8])>, PackageError> {
    if !bytes.starts_with(COMPONENT_HEADER) {
        return Err(PackageError::NotComponent);
    }
    let mut cursor = COMPONENT_HEADER.len();
    while cursor < bytes.len() {
        let start = cursor;
        let id = bytes[cursor];
        cursor += 1;
        let len = read_u32(bytes, &mut cursor)? as usize;
        let end = cursor
            .checked_add(len)
            .filter(|end| *end <= bytes.len())
            .ok_or(PackageError::InvalidFraming)?;
        if id == 0 {
            let section = &bytes[cursor..end];
            let mut name_start = 0;
            let name_len = read_u32(section, &mut name_start)? as usize;
            let name_end = name_start
                .checked_add(name_len)
                .filter(|end| *end <= section.len())
                .ok_or(PackageError::InvalidFraming)?;
            if &section[name_start..name_end] == SECTION_NAME.as_bytes() {
                if end != bytes.len() {
                    return Err(PackageError::InvalidLayout);
                }
                return Ok(Some((start, &section[name_end..])));
            }
        }
        cursor = end;
    }
    Ok(None)
}

fn checked_bindings(
    bindings: Vec<Binding>,
    artifacts: &BTreeMap<String, &[u8]>,
) -> Result<BTreeMap<String, Binding>, PackageError> {
    let mut indexed = BTreeMap::new();
    for binding in bindings {
        if binding.id.is_empty() || binding.interface.is_empty() {
            return Err(PackageError::InvalidManifest);
        }
        if !artifacts.contains_key(&binding.artifact) {
            return Err(PackageError::MissingArtifact);
        }
        if indexed.insert(binding.id.clone(), binding).is_some() {
            return Err(PackageError::DuplicateBinding);
        }
    }
    Ok(indexed)
}

pub fn parse(
    bytes: &[u8],
    limits: PackageLimits,
) -> Result<Option<ParsedPackage<'_>>, PackageError> {
    if bytes.len() > limits.total_bytes {
        return Err(PackageError::LimitExceeded);
    }
    let Some((root_end, section)) = catalog_section(bytes)? else {
        return Ok(None);
    };
    let length_bytes: [u8; 4] = section
        .get(..4)
        .ok_or(PackageError::InvalidFraming)?
        .try_into()
        .unwrap();
    let manifest_len = u32::from_le_bytes(length_bytes) as usize;
    if manifest_len > limits.manifest_bytes {
        return Err(PackageError::LimitExceeded);
    }
    let manifest_end = 4usize
        .checked_add(manifest_len)
        .filter(|end| *end <= section.len())
        .ok_or(PackageError::InvalidFraming)?;
    let manifest: Manifest = serde_json::from_slice(&section[4..manifest_end])
        .map_err(|_| PackageError::InvalidManifest)?;
    if !matches!(
        (manifest.version, manifest.invocations.is_some()),
        (1, false) | (2, true)
    ) {
        return Err(PackageError::UnsupportedVersion);
    }
    if manifest.artifacts.len() > limits.artifacts || manifest.bindings.len() > limits.bindings {
        return Err(PackageError::LimitExceeded);
    }
    let bodies = &section[manifest_end..];
    let mut artifacts = BTreeMap::new();
    let mut next_offset = 0;
    let mut previous = None;
    for artifact in manifest.artifacts {
        if artifact.offset != next_offset
            || previous
                .as_ref()
                .is_some_and(|digest| digest >= &artifact.digest)
        {
            return Err(PackageError::InvalidLayout);
        }
        let end = artifact
            .offset
            .checked_add(artifact.length)
            .filter(|end| *end <= bodies.len())
            .ok_or(PackageError::InvalidLayout)?;
        let body = &bodies[artifact.offset..end];
        if !body.starts_with(COMPONENT_HEADER) {
            return Err(PackageError::NotComponent);
        }
        if artifact_digest(body) != artifact.digest {
            return Err(PackageError::DigestMismatch);
        }
        // Catalogs are flat. Children reference the enclosing catalog at runtime;
        // recursively embedded catalogs would defeat size/admission accounting.
        if catalog_section(body)?.is_some() {
            return Err(PackageError::AlreadyPackaged);
        }
        previous = Some(artifact.digest.clone());
        artifacts.insert(artifact.digest, body);
        next_offset = end;
    }
    if next_offset != bodies.len() {
        return Err(PackageError::InvalidLayout);
    }
    let bindings = checked_bindings(manifest.bindings, &artifacts)?;
    if let Some(invocations) = &manifest.invocations {
        invocations.validate(&bindings)?;
    }
    if bindings
        .values()
        .map(|binding| &binding.artifact)
        .collect::<BTreeSet<_>>()
        .len()
        != artifacts.len()
    {
        return Err(PackageError::MissingArtifact);
    }
    Ok(Some(ParsedPackage {
        root: &bytes[..root_end],
        artifacts,
        bindings,
        invocations: manifest.invocations,
    }))
}

/// Build deterministically. Call sites reference a binding; content-identical
/// artifacts are stored once even if supplied repeatedly or under many bindings.
pub fn append(
    root: &[u8],
    components: &[&[u8]],
    bindings: Vec<Binding>,
    limits: PackageLimits,
) -> Result<Vec<u8>, PackageError> {
    append_inner(root, components, bindings, None, limits)
}

/// Version 2 carries compiler invocation authority. Legacy `append` keeps the
/// exact v1 encoding; older readers reject v2 instead of discarding authority.
pub fn append_with_invocations(
    root: &[u8],
    components: &[&[u8]],
    bindings: Vec<Binding>,
    invocations: InvocationManifest,
    limits: PackageLimits,
) -> Result<Vec<u8>, PackageError> {
    append_inner(root, components, bindings, Some(invocations), limits)
}

fn append_inner(
    root: &[u8],
    components: &[&[u8]],
    bindings: Vec<Binding>,
    invocations: Option<InvocationManifest>,
    limits: PackageLimits,
) -> Result<Vec<u8>, PackageError> {
    if root.len() > limits.total_bytes || bindings.len() > limits.bindings {
        return Err(PackageError::LimitExceeded);
    }
    if catalog_section(root)?.is_some() {
        return Err(PackageError::AlreadyPackaged);
    }
    let mut unique = BTreeMap::new();
    let mut total = root.len();
    for body in components {
        if body.len() > limits.total_bytes {
            return Err(PackageError::LimitExceeded);
        }
        if catalog_section(body)?.is_some() {
            return Err(PackageError::AlreadyPackaged);
        }
        let digest = artifact_digest(body);
        if unique.insert(digest, *body).is_none() {
            total = total
                .checked_add(body.len())
                .filter(|n| *n <= limits.total_bytes)
                .ok_or(PackageError::LimitExceeded)?;
        }
        if unique.len() > limits.artifacts {
            return Err(PackageError::LimitExceeded);
        }
    }
    let bindings = checked_bindings(bindings, &unique)?;
    if let Some(invocations) = &invocations {
        invocations.validate(&bindings)?;
    }
    let used: BTreeSet<_> = bindings.values().map(|binding| &binding.artifact).collect();
    if used.len() != unique.len() {
        return Err(PackageError::MissingArtifact);
    }
    let mut offset = 0;
    let artifacts = unique
        .iter()
        .map(|(digest, body)| {
            let artifact = Artifact {
                digest: digest.clone(),
                offset,
                length: body.len(),
            };
            offset += body.len();
            artifact
        })
        .collect();
    let manifest = serde_json::to_vec(&Manifest {
        version: if invocations.is_some() { 2 } else { 1 },
        artifacts,
        bindings: bindings.into_values().collect(),
        invocations,
    })
    .map_err(|_| PackageError::InvalidManifest)?;
    if manifest.len() > limits.manifest_bytes {
        return Err(PackageError::LimitExceeded);
    }
    let manifest_len = u32::try_from(manifest.len()).map_err(|_| PackageError::LimitExceeded)?;
    let mut prefix = Vec::new();
    write_u32(SECTION_NAME.len() as u32, &mut prefix);
    prefix.extend_from_slice(SECTION_NAME.as_bytes());
    prefix.extend_from_slice(&manifest_len.to_le_bytes());
    let payload_len = prefix
        .len()
        .checked_add(manifest.len())
        .and_then(|n| n.checked_add(offset))
        .ok_or(PackageError::LimitExceeded)?;
    let mut section_header = vec![0];
    write_u32(
        u32::try_from(payload_len).map_err(|_| PackageError::LimitExceeded)?,
        &mut section_header,
    );
    let size = root
        .len()
        .checked_add(section_header.len())
        .and_then(|n| n.checked_add(payload_len))
        .filter(|n| *n <= limits.total_bytes)
        .ok_or(PackageError::LimitExceeded)?;
    let mut output = Vec::with_capacity(size);
    output.extend_from_slice(root);
    output.extend_from_slice(&section_header);
    output.extend_from_slice(&prefix);
    output.extend_from_slice(&manifest);
    for body in unique.values() {
        output.extend_from_slice(body);
    }
    Ok(output)
}

#[cfg(test)]
mod tests;
