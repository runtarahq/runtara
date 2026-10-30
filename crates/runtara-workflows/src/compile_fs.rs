// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Where the compiler reads its components and writes its build: the real
//! filesystem, unless the compile runs inside [`in_memory`], when the same
//! paths name files of a [`MemoryFs`] instead.
//!
//! A compiler running where there is no filesystem (a WASI command in a VM
//! that grants none) is handed its components as bytes and hands back its
//! build the same way, with the compile itself unchanged.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Files kept in memory, by path. Directories are implied by the files
/// under them.
#[derive(Debug, Default)]
pub struct MemoryFs {
    files: Mutex<BTreeMap<PathBuf, Vec<u8>>>,
}

impl MemoryFs {
    /// An empty one.
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Puts `bytes` at `path`, replacing what was there.
    pub fn insert(&self, path: impl Into<PathBuf>, bytes: Vec<u8>) {
        self.files
            .lock()
            .expect("memory filesystem poisoned")
            .insert(path.into(), bytes);
    }

    /// The bytes at `path`, if a file is there.
    pub fn get(&self, path: impl AsRef<Path>) -> Option<Vec<u8>> {
        self.files
            .lock()
            .expect("memory filesystem poisoned")
            .get(path.as_ref())
            .cloned()
    }

    /// Every file under `dir`, by its path relative to `dir`.
    pub fn under(&self, dir: impl AsRef<Path>) -> BTreeMap<PathBuf, Vec<u8>> {
        let dir = dir.as_ref();
        self.files
            .lock()
            .expect("memory filesystem poisoned")
            .iter()
            .filter_map(|(path, bytes)| {
                path.strip_prefix(dir)
                    .ok()
                    .map(|relative| (relative.to_path_buf(), bytes.clone()))
            })
            .collect()
    }

    fn is_dir(&self, path: &Path) -> bool {
        self.files
            .lock()
            .expect("memory filesystem poisoned")
            .keys()
            .any(|file| file != path && file.starts_with(path))
    }
}

thread_local! {
    static CURRENT: RefCell<Option<Arc<MemoryFs>>> = const { RefCell::new(None) };
}

/// Runs `compile` with `fs` as the compiler's filesystem.
pub fn in_memory<R>(fs: Arc<MemoryFs>, compile: impl FnOnce() -> R) -> R {
    let previous = CURRENT.with(|current| current.replace(Some(fs)));
    let result = compile();
    CURRENT.with(|current| *current.borrow_mut() = previous);
    result
}

/// The memory filesystem of the current compile, if it has one: work moved
/// to another thread takes it along.
pub(crate) fn current() -> Option<Arc<MemoryFs>> {
    CURRENT.with(|current| current.borrow().clone())
}

fn not_found(path: &Path) -> io::Error {
    io::Error::new(
        io::ErrorKind::NotFound,
        format!("{} is not there", path.display()),
    )
}

pub(crate) fn read(path: impl AsRef<Path>) -> io::Result<Vec<u8>> {
    let path = path.as_ref();
    match current() {
        Some(fs) => fs.get(path).ok_or_else(|| not_found(path)),
        None => std::fs::read(path),
    }
}

pub(crate) fn write(path: impl AsRef<Path>, bytes: impl AsRef<[u8]>) -> io::Result<()> {
    match current() {
        Some(fs) => {
            fs.insert(path.as_ref(), bytes.as_ref().to_vec());
            Ok(())
        }
        None => std::fs::write(path, bytes),
    }
}

pub(crate) fn create_dir_all(path: impl AsRef<Path>) -> io::Result<()> {
    match current() {
        Some(_) => Ok(()),
        None => std::fs::create_dir_all(path),
    }
}

pub(crate) fn exists(path: impl AsRef<Path>) -> bool {
    let path = path.as_ref();
    match current() {
        Some(fs) => fs.get(path).is_some() || fs.is_dir(path),
        None => path.exists(),
    }
}

/// The size of the file at `path`.
pub(crate) fn file_len(path: impl AsRef<Path>) -> io::Result<u64> {
    let path = path.as_ref();
    match current() {
        Some(fs) => fs
            .get(path)
            .map(|bytes| bytes.len() as u64)
            .ok_or_else(|| not_found(path)),
        None => std::fs::metadata(path).map(|metadata| metadata.len()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_compile_in_memory_reads_and_writes_only_memory() {
        let dir = tempfile::tempdir().unwrap();
        let on_disk = dir.path().join("on-disk");
        std::fs::write(&on_disk, b"disk").unwrap();
        let fs = MemoryFs::new();
        fs.insert("/components/a.wasm", b"component".to_vec());
        in_memory(fs.clone(), || {
            assert_eq!(read("/components/a.wasm").unwrap(), b"component");
            assert!(exists("/components"), "a directory is implied by its files");
            assert!(!exists(&on_disk), "the real filesystem is out of reach");
            assert!(read(&on_disk).is_err());
            write("/build/workflow.wasm", b"output").unwrap();
            assert_eq!(file_len("/build/workflow.wasm").unwrap(), 6);
        });
        assert_eq!(
            fs.under("/build"),
            BTreeMap::from([(PathBuf::from("workflow.wasm"), b"output".to_vec())])
        );
        assert!(!dir.path().join("build").exists());
        // Outside it, the real filesystem again.
        assert_eq!(read(&on_disk).unwrap(), b"disk");
        assert!(current().is_none());
    }
}
