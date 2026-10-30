// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! `runtara-server publish-release <components-dir> [--release <name>]`
//!
//! Publishes a built components bundle (its agents and shared components) as
//! a release into the artifact store at `RUNTARA_ARTIFACT_STORE_URL`, as a
//! server does at boot, without a database or a server (SYN-779). Release CI
//! runs it once per release; a server with no components directory then runs
//! the release from that store. The release is named by `--release`, else
//! `RUNTARA_AGENT_RELEASE`, else this server's version.

use std::ffi::OsString;
use std::path::PathBuf;

/// The argv[1] that selects this command.
pub const COMMAND: &str = "publish-release";

const USAGE: &str = "Usage: runtara-server publish-release <components-dir> [--release <name>]";

#[derive(Debug, PartialEq, Eq)]
pub struct PublishArgs {
    pub components: PathBuf,
    pub release: Option<String>,
}

/// Parse the process arguments. Returns `None` when they don't select this
/// command, so the caller falls through.
pub fn parse_args(mut args: impl Iterator<Item = OsString>) -> Option<Result<PublishArgs, String>> {
    let _program = args.next();
    if args.next()?.to_str() != Some(COMMAND) {
        return None;
    }
    Some(parse_options(args))
}

fn parse_options(mut args: impl Iterator<Item = OsString>) -> Result<PublishArgs, String> {
    let mut components = None;
    let mut release = None;
    while let Some(arg) = args.next() {
        let arg = arg
            .into_string()
            .map_err(|_| format!("arguments must be valid UTF-8\n{USAGE}"))?;
        match arg.as_str() {
            "--release" => {
                let name = args
                    .next()
                    .and_then(|v| v.into_string().ok())
                    .filter(|v| !v.trim().is_empty())
                    .ok_or_else(|| format!("--release needs a value\n{USAGE}"))?;
                release = Some(name);
            }
            other if other.starts_with("--") => {
                return Err(format!("unexpected argument '{other}'\n{USAGE}"));
            }
            dir if components.is_none() => components = Some(PathBuf::from(dir)),
            other => return Err(format!("unexpected argument '{other}'\n{USAGE}")),
        }
    }
    Ok(PublishArgs {
        components: components.ok_or_else(|| USAGE.to_owned())?,
        release,
    })
}

/// Publish `args.components` into the store at `RUNTARA_ARTIFACT_STORE_URL`.
pub async fn run(args: PublishArgs) -> Result<(), String> {
    let url = std::env::var("RUNTARA_ARTIFACT_STORE_URL")
        .ok()
        .filter(|url| !url.trim().is_empty())
        .ok_or("RUNTARA_ARTIFACT_STORE_URL names the store to publish to")?;
    let release = args
        .release
        .unwrap_or_else(crate::agent_release::release_name);
    // Publishing reads nothing back, so the store's local cache is throwaway.
    let cache = std::env::temp_dir().join(format!("runtara-publish-{}", uuid::Uuid::new_v4()));
    let store =
        runtara_artifact_store::object::ObjectArtifactStore::from_url(url.trim(), cache.clone())
            .map_err(|error| format!("RUNTARA_ARTIFACT_STORE_URL: {error}"))?;
    let published =
        runtara_artifact_store::release::publish_dir(&store, &args.components, &release).await;
    let _ = std::fs::remove_dir_all(&cache);
    let index = published.map_err(|error| {
        format!(
            "publish {} as release {release}: {error}",
            args.components.display()
        )
    })?;
    println!(
        "published release {release}: {} agents, {} shared components, index {}",
        index.agents.len(),
        index.components.len(),
        index.digest()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Option<Result<PublishArgs, String>> {
        parse_args(
            std::iter::once("runtara-server")
                .chain(args.iter().copied())
                .map(OsString::from),
        )
    }

    #[test]
    fn the_command_takes_a_bundle_and_a_release_name() {
        assert_eq!(parse(&["serve"]), None);
        assert_eq!(
            parse(&["publish-release", "/bundle"]),
            Some(Ok(PublishArgs {
                components: "/bundle".into(),
                release: None
            }))
        );
        assert_eq!(
            parse(&["publish-release", "/bundle", "--release", "9.1.0"]),
            Some(Ok(PublishArgs {
                components: "/bundle".into(),
                release: Some("9.1.0".into())
            }))
        );
        assert!(matches!(parse(&["publish-release"]), Some(Err(_))));
        assert!(matches!(
            parse(&["publish-release", "/a", "/b"]),
            Some(Err(_))
        ));
        assert!(matches!(
            parse(&["publish-release", "/a", "--release"]),
            Some(Err(_))
        ));
    }
}
