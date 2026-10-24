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
    let mut escaped_path = false;
    let mut safe: Vec<&String> = Vec::new();
    for path in touched {
        if safe_join(repo, path)?.is_some() {
            safe.push(path);
        } else {
            escaped_path = true;
        }
    }
    if safe.is_empty() {
        return Ok(Vec::new());
    }
    let mut diff_args = vec![
        "-C",
        repo.to_str().unwrap_or("."),
        "diff",
        "--no-color",
        "-U20",
        diff_base,
        "--",
    ];
    diff_args.extend(safe.iter().map(|path| path.as_str()));
    let diff = run_git(&diff_args)?;
    content.push_str(&diff);

    for path in safe {
        if is_tracked(repo, path)? {
            continue;
        }
        let Some(full) = safe_join(repo, path)? else {
            continue;
        };
        let text = std::fs::read_to_string(&full).unwrap_or_default();
        content.push_str(&format!("+++ {path} (new file)\n{text}\n"));
    }

    if content.trim().is_empty() {
        return Ok(Vec::new());
    }
    let complete = content.len() <= MAX_DIFF_CHARS && !escaped_path;
    if content.len() > MAX_DIFF_CHARS {
        let cut = floor_char_boundary(&content, MAX_DIFF_CHARS);
        content.truncate(cut);
        content.push_str("\n... diff truncated to the review budget ...\n");
    }
    if escaped_path {
        content.push_str("\n... paths outside the repository were skipped ...\n");
    }
    Ok(vec![Artifact {
        kind: "git_diff".to_owned(),
        path: None,
        content,
        complete,
    }])
}

/// Resolve a touched path inside `repo`, refusing absolute paths and `..`
/// escapes. Session data is untrusted; a malicious path must never read files
/// outside the reviewed repository.
fn safe_join(repo: &Path, path: &str) -> Result<Option<std::path::PathBuf>, CaptureError> {
    if Path::new(path).is_absolute() {
        return Ok(None);
    }
    let root = std::fs::canonicalize(repo)
        .map_err(|error| CaptureError::Invalid(format!("cannot resolve repository: {error}")))?;
    let joined = repo.join(path);
    let resolved = std::fs::canonicalize(&joined).unwrap_or(joined);
    Ok(resolved.starts_with(&root).then_some(resolved))
}

fn floor_char_boundary(text: &str, index: usize) -> usize {
    let mut index = index.min(text.len());
    while index > 0 && !text.is_char_boundary(index) {
        index -= 1;
    }
    index
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
    fn truncation_is_utf8_safe() {
        let mut text = "é".repeat(MAX_DIFF_CHARS);
        let cut = floor_char_boundary(&text, MAX_DIFF_CHARS);
        text.truncate(cut);
        assert!(text.is_char_boundary(text.len()));
    }

    #[test]
    fn refuses_paths_outside_the_repository() {
        let dir = repo();
        let outside = dir.path().parent().unwrap().join("outside-sideeye.txt");
        std::fs::write(&outside, "outside secret\n").unwrap();
        let artifacts =
            collect_diff_artifacts(dir.path(), "HEAD", &["../outside-sideeye.txt".to_owned()])
                .expect("artifacts");
        let content = artifacts
            .iter()
            .map(|artifact| artifact.content.as_str())
            .collect::<String>();
        assert!(!content.contains("outside secret"));
        let _ = std::fs::remove_file(outside);
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
