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
use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Loads a known file's bytes the first time the compile reads it.
pub type Loader = Box<dyn Fn(&Path) -> io::Result<Vec<u8>> + Send + Sync>;

/// Files kept in memory, by path. Directories are implied by the files
/// under them. A file may be known before it is loaded: it exists, and is
/// loaded (and kept) when first read.
#[derive(Default)]
pub struct MemoryFs {
    files: Mutex<BTreeMap<PathBuf, Vec<u8>>>,
    known: BTreeSet<PathBuf>,
    loader: Option<Loader>,
}

impl std::fmt::Debug for MemoryFs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemoryFs")
            .field("known", &self.known.len())
            .finish_non_exhaustive()
    }
}

impl MemoryFs {
    /// An empty one.
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// One whose `known` files are loaded by `loader` when first read.
    pub fn with_loader(known: impl IntoIterator<Item = PathBuf>, loader: Loader) -> Arc<Self> {
        Arc::new(Self {
            files: Mutex::default(),
            known: known.into_iter().collect(),
            loader: Some(loader),
        })
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
        let under = |file: &PathBuf| file != path && file.starts_with(path);
        self.known.iter().any(under)
            || self
                .files
                .lock()
                .expect("memory filesystem poisoned")
                .keys()
                .any(under)
    }

    fn is_file(&self, path: &Path) -> bool {
        self.known.contains(path) || self.get(path).is_some()
    }

    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        if let Some(bytes) = self.get(path) {
            return Ok(bytes);
        }
        match &self.loader {
            Some(loader) if self.known.contains(path) => {
                let bytes = loader(path)?;
                self.insert(path, bytes.clone());
                Ok(bytes)
            }
            _ => Err(not_found(path)),
        }
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
        Some(fs) => fs.read(path),
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
        Some(fs) => fs.is_file(path) || fs.is_dir(path),
        None => path.exists(),
    }
}

/// The size of the file at `path`.
pub(crate) fn file_len(path: impl AsRef<Path>) -> io::Result<u64> {
    let path = path.as_ref();
    match current() {
        Some(fs) => fs.read(path).map(|bytes| bytes.len() as u64),
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

    #[test]
    fn a_known_file_is_loaded_once_when_first_read() {
        let loads = Arc::new(Mutex::new(Vec::new()));
        let seen = loads.clone();
        let fs = MemoryFs::with_loader(
            [
                PathBuf::from("/components/a.wasm"),
                PathBuf::from("/components/b.wasm"),
            ],
            Box::new(move |path| {
                seen.lock().unwrap().push(path.to_path_buf());
                Ok(path.to_string_lossy().into_owned().into_bytes())
            }),
        );
        in_memory(fs, || {
            assert!(exists("/components/b.wasm"), "known, before it is loaded");
            assert!(exists("/components"));
            assert!(!exists("/components/c.wasm"));
            assert_eq!(read("/components/a.wasm").unwrap(), b"/components/a.wasm");
            assert_eq!(read("/components/a.wasm").unwrap(), b"/components/a.wasm");
            assert!(
                read("/components/c.wasm").is_err(),
                "an unknown file is not loaded"
            );
        });
        assert_eq!(
            *loads.lock().unwrap(),
            [PathBuf::from("/components/a.wasm")]
        );
    }
}
