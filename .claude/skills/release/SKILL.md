---
name: release
description: Use to cut a new release — bumps the workspace version, commits, tags, and pushes. The tag push triggers the release CI workflow which publishes to crates.io. Wraps scripts/release.sh with a pre-flight checklist so a release doesn't ship broken.
---

# Cut a release

The release flow is a single script: [scripts/release.sh](../../../scripts/release.sh) `<patch|minor|major>`.

## Pre-flight checklist

Run **before** the script. It refuses to run with a dirty tree, but it won't catch logic regressions.

1. **On `main`, up to date with origin.**
   ```bash
   git checkout main && git pull --ff-only
   ```

2. **Working tree clean.**
   ```bash
   git status
   ```
   No staged or unstaged changes. The script will abort otherwise.

3. **CI green on `main`.** Check [.github/workflows](../../../.github/workflows). If CI is red, fix before tagging — the release workflow runs against the tagged commit.

4. **`e2e-verify` passes locally.** A clean tree doesn't mean the tag will work end-to-end. Run the `e2e-verify` skill against the commit you're about to tag.

5. **Pick the bump type.** Follow semver:
   - `patch` — bug fixes, no API changes
   - `minor` — new features, backwards-compatible
   - `major` — breaking changes (rare; coordinate with downstream consumers)

## Run the release

```bash
./scripts/release.sh patch    # or minor, or major
```

The script:

1. Reads the latest `v*.*.*` tag, computes the next version.
2. Runs [scripts/update-version.sh](../../../scripts/update-version.sh) to bump versions in all `Cargo.toml` files.
3. `git add -A`, commits as `Release <version>`, tags as `v<version>`.
4. Prompts before pushing. Answer `y` to push commit + tag to `origin`.

## After the push

The tag push triggers [.github/workflows/release.yml](../../../.github/workflows/release.yml), which publishes crates.io artifacts. Watch the workflow run — if publish fails partway through (e.g. a sub-crate already published), the recovery is **not** to delete and re-tag (`v<X>` is permanent on crates.io). Cut a new patch release with the fix instead.

## If the script aborted partway

- Tag created locally but not pushed: `git tag -d v<version>` is safe.
- Commit created but you don't want to push it: `git reset --hard HEAD~1` (only if you haven't pushed).
- Versions bumped in `Cargo.toml` but no commit yet: `git checkout -- .` to revert.

Per repo policy: **never force-push** a release. If a tagged release is broken, ship a patch on top.

## Files touched (by the script)

- All `Cargo.toml` files with workspace versions
- New commit `Release <version>`
- New tag `v<version>`
