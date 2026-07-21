#[path = "../build_metadata.rs"]
mod build_metadata;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn git(repo: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .expect("git must run");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

fn commit(repo: &Path, contents: &str, message: &str) -> String {
    fs::write(repo.join("tracked.txt"), contents).unwrap();
    git(repo, &["add", "tracked.txt"]);
    git(repo, &["commit", "-m", message]);
    git(repo, &["rev-parse", "HEAD"])
}

fn init_repo() -> tempfile::TempDir {
    let repo = tempfile::tempdir().unwrap();
    git(repo.path(), &["init"]);
    git(
        repo.path(),
        &["config", "user.email", "build-test@example.com"],
    );
    git(repo.path(), &["config", "user.name", "Build Test"]);
    repo
}

fn canonical(path: impl AsRef<Path>) -> PathBuf {
    fs::canonicalize(path).unwrap()
}

fn contains_path(paths: &[PathBuf], expected: impl AsRef<Path>) -> bool {
    let expected = canonical(expected);
    paths
        .iter()
        .filter_map(|path| fs::canonicalize(path).ok())
        .any(|path| path == expected)
}

#[test]
fn watches_the_symbolic_ref_that_changes_on_same_branch_commits() {
    let repo = init_repo();
    let first_commit = commit(repo.path(), "one", "first");

    let symbolic_ref = git(repo.path(), &["symbolic-ref", "HEAD"]);
    let ref_path = repo.path().join(".git").join(&symbolic_ref);
    let paths = build_metadata::git_rerun_paths(repo.path());
    assert!(contains_path(&paths, repo.path().join(".git/HEAD")));
    assert!(contains_path(&paths, &ref_path));
    assert_eq!(fs::read_to_string(&ref_path).unwrap().trim(), first_commit);

    let second_commit = commit(repo.path(), "two", "second");
    assert_ne!(first_commit, second_commit);
    assert_eq!(fs::read_to_string(&ref_path).unwrap().trim(), second_commit);
}

#[test]
fn watches_head_for_detached_checkouts_and_packed_refs_for_packed_branches() {
    let repo = init_repo();
    let commit_id = commit(repo.path(), "one", "first");
    git(repo.path(), &["pack-refs", "--all"]);

    let paths = build_metadata::git_rerun_paths(repo.path());
    assert!(contains_path(&paths, repo.path().join(".git/packed-refs")));

    git(repo.path(), &["checkout", "--detach", &commit_id]);
    let detached_paths = build_metadata::git_rerun_paths(repo.path());
    assert!(contains_path(
        &detached_paths,
        repo.path().join(".git/HEAD")
    ));
}
