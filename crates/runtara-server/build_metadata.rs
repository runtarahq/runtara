use std::path::{Path, PathBuf};
use std::process::Command;

/// Git administrative files whose contents can change the commit embedded in
/// the server binary.
///
/// `.git/HEAD` is not enough while a branch is checked out: it contains only
/// `ref: refs/heads/<branch>`, while commits update the referenced file. Ask
/// Git for the paths so linked worktrees and non-standard git directories work
/// too. `packed-refs` covers repositories where the symbolic ref is packed.
pub(crate) fn git_rerun_paths(workspace_root: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();

    push_git_path(workspace_root, "HEAD", &mut paths);

    if let Some(symbolic_ref) =
        git_output(workspace_root, &["symbolic-ref", "-q", "HEAD"]).and_then(clean_value)
    {
        push_git_path(workspace_root, &symbolic_ref, &mut paths);
    }

    push_git_path(workspace_root, "packed-refs", &mut paths);
    paths.sort();
    paths.dedup();
    paths
}

fn push_git_path(workspace_root: &Path, name: &str, paths: &mut Vec<PathBuf>) {
    let Some(path) = git_output(
        workspace_root,
        &["rev-parse", "--path-format=absolute", "--git-path", name],
    )
    .and_then(clean_value) else {
        return;
    };
    paths.push(PathBuf::from(path));
}

fn clean_value(value: String) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

fn git_output(workspace_root: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(workspace_root)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8(output.stdout).ok())
        .flatten()
}
