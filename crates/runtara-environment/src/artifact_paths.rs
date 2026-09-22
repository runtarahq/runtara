// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Filesystem names are encodings of identities, never raw path components.

use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

pub(crate) fn identity_component(identity: &str) -> String {
    format!("{:x}", Sha256::digest(identity.as_bytes()))
}

pub(crate) fn tenant_root(data_dir: &Path, tenant: &str) -> PathBuf {
    data_dir.join("tenants").join(identity_component(tenant))
}

pub(crate) fn images_dir(data_dir: &Path, tenant: &str) -> PathBuf {
    tenant_root(data_dir, tenant).join("images")
}

pub(crate) fn image_dir(data_dir: &Path, tenant: &str, image: &str) -> PathBuf {
    images_dir(data_dir, tenant).join(identity_component(image))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identities_cannot_escape_or_alias_another_tenants_artifacts() {
        let root = Path::new("/isolated-data");
        for tenant in ["../b", "/b", "a/b", "a\\b", "..", "*", "é", "A", "a"] {
            let path = image_dir(root, tenant, "../../binary");
            assert!(path.starts_with(root.join("tenants")));
            assert_eq!(path.strip_prefix(root).unwrap().components().count(), 4);
            assert_ne!(path, image_dir(root, "b", "../../binary"));
        }
        assert_ne!(image_dir(root, "A", "id"), image_dir(root, "a", "id"));
        assert_ne!(image_dir(root, "a", "id"), image_dir(root, "a", "ID"));
    }
}
