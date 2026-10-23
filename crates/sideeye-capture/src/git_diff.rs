//! Git evidence capture for review packets.
//!
//! A transcript-only review cannot verify claims about code. This module
//! produces part of the code artifact from the repository itself: a combined
//! diff against a base ref, plus untracked files the session touched.

use std::path::Path;
use std::process::Command;

use sideeye_core::Artifact;

use crate::CaptureError;

/// Per-artifact character cap. A bigger diff is truncated and marked
/// incomplete so the judge knows its view is partial.
pub const MAX_DIFF_CHARS: usize = 200_000;

pub fn collect_diff_artifacts(
    repo: &Path,
    diff_base: &str,
    touched: &[String],
) -> Result<Vec<Artifact>, CaptureError> {
    if diff_base.is_empty() || diff_base.starts_with('-') {
        return Err(CaptureError::Invalid(format!(
            "invalid git diff base: {diff_base:?}"
        )));
    }
    if touched.is_empty() {
        return Ok(Vec::new());
    }

    let mut content = String::new();
    let tracked: Vec<&String> = touched.iter().collect();
    let mut diff_args = vec![
        "-C",
        repo.to_str().unwrap_or("."),
        "diff",
        "--no-color",
        "-U20",
        diff_base,
        "--",
    ];
    diff_args.extend(tracked.iter().map(|path| path.as_str()));
    let diff = run_git(&diff_args)?;
    content.push_str(&diff);

    for path in touched {
        if is_tracked(repo, path)? {
            continue;
        }
        let full = repo.join(path);
        let text = std::fs::read_to_string(&full).unwrap_or_default();
        content.push_str(&format!("+++ {path} (new file)\n{text}\n"));
    }

    if content.trim().is_empty() {
        return Ok(Vec::new());
    }
    let complete = content.len() <= MAX_DIFF_CHARS;
    if !complete {
        content.truncate(MAX_DIFF_CHARS);
        content.push_str("\n... diff truncated to the review budget ...\n");
    }
    Ok(vec![Artifact {
        kind: "git_diff".to_owned(),
        path: None,
        content,
        complete,
    }])
}

fn run_git(args: &[&str]) -> Result<String, CaptureError> {
    let output = Command::new("git")
        .args(args)
        .output()
        .map_err(|error| CaptureError::Invalid(format!("cannot run git: {error}")))?;
    if !output.status.success() {
        return Err(CaptureError::Invalid(format!(
            "git failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn is_tracked(repo: &Path, path: &str) -> Result<bool, CaptureError> {
    let output = Command::new("git")
        .args([
            "-C",
            repo.to_str().unwrap_or("."),
            "ls-files",
            "--error-unmatch",
            "--",
            path,
        ])
        .output()
        .map_err(|error| CaptureError::Invalid(format!("cannot run git: {error}")))?;
    Ok(output.status.success())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(repo: &Path, args: &[&str]) {
        let status = Command::new("git")
            .args(["-C", repo.to_str().unwrap()])
            .args(args)
            .status()
            .expect("git");
        assert!(status.success(), "git {args:?}");
    }

    fn repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        git(dir.path(), &["init", "-q"]);
        git(
            dir.path(),
            &["config", "user.email", "test@example.invalid"],
        );
        git(dir.path(), &["config", "user.name", "Test"]);
        std::fs::write(dir.path().join("a.txt"), "before\n").unwrap();
        git(dir.path(), &["add", "a.txt"]);
        git(dir.path(), &["commit", "-qm", "initial"]);
        dir
    }

    #[test]
    fn captures_tracked_and_untracked_changes() {
        let dir = repo();
        std::fs::write(dir.path().join("a.txt"), "after\n").unwrap();
        std::fs::write(dir.path().join("new.txt"), "new file\n").unwrap();
        let artifacts = collect_diff_artifacts(
            dir.path(),
            "HEAD",
            &["a.txt".to_owned(), "new.txt".to_owned()],
        )
        .expect("artifacts");
        assert_eq!(artifacts.len(), 1);
        let artifact = &artifacts[0];
        assert_eq!(artifact.kind, "git_diff");
        assert!(artifact.complete);
        assert!(artifact.content.contains("+after"));
        assert!(artifact.content.contains("new file"));
    }

    #[test]
    fn rejects_option_injection_in_diff_base() {
        let dir = repo();
        assert!(
            collect_diff_artifacts(dir.path(), "--output=/tmp/x", &["a.txt".to_owned()]).is_err()
        );
    }

    #[test]
    fn no_changes_produce_no_artifact() {
        let dir = repo();
        assert!(
            collect_diff_artifacts(dir.path(), "HEAD", &["a.txt".to_owned()])
                .expect("artifacts")
                .is_empty()
        );
    }
}
