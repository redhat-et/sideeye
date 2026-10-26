//! Locate and parse Claude Code and Codex session files.

use std::{
    env, fs,
    path::{Path, PathBuf},
    time::SystemTime,
};

use crate::{CaptureError, CapturedSession, parse_claude_session, parse_codex_rollout};

#[derive(Debug, Clone, Default)]
pub struct SessionRequest {
    /// `claude` or `codex`.
    pub client: String,
    /// Explicit rollout/transcript path.
    pub rollout: Option<PathBuf>,
    /// Prefer the current harness session (Claude Code exports its session ID).
    pub current: bool,
    /// Search every project instead of the current project/ancestors.
    pub all_projects: bool,
    /// A specific Claude project directory or Codex path.
    pub project: Option<String>,
    /// Override the home directory for tests.
    pub home: Option<PathBuf>,
    /// Override the working directory for tests.
    pub cwd: Option<PathBuf>,
}

pub fn resolve_session(request: &SessionRequest) -> Result<PathBuf, CaptureError> {
    if let Some(path) = &request.rollout {
        if path.is_file() {
            return Ok(path.clone());
        }
        return Err(CaptureError::Io(format!(
            "session file does not exist: {}",
            path.display()
        )));
    }
    let client = request.client.trim().to_lowercase();
    match client.as_str() {
        "claude" | "claude-code" => resolve_claude(request),
        "codex" => resolve_codex(request),
        other => Err(CaptureError::Io(format!(
            "unknown client {other:?}; use claude or codex"
        ))),
    }
}

pub fn parse_session_file(path: &Path) -> Result<CapturedSession, CaptureError> {
    let text = fs::read_to_string(path)
        .map_err(|error| CaptureError::Io(format!("{}: {error}", path.display())))?;
    for line in text.lines().take(20) {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(object) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        match object.get("type").and_then(serde_json::Value::as_str) {
            Some("session_meta" | "turn_context" | "response_item") => {
                return parse_codex_rollout(path);
            }
            Some("user" | "assistant") if object.get("message").is_some() => {
                return parse_claude_session(path);
            }
            _ => {}
        }
    }
    Err(CaptureError::Io(format!(
        "could not detect a Claude Code or Codex session in {}",
        path.display()
    )))
}

fn resolve_claude(request: &SessionRequest) -> Result<PathBuf, CaptureError> {
    let home =
        request.home.clone().or_else(home_dir).ok_or_else(|| {
            CaptureError::Io("HOME is not set; pass --rollout explicitly".to_owned())
        })?;
    let projects = home.join(".claude").join("projects");

    if let Ok(session_id) = env::var("CLAUDE_CODE_SESSION_ID")
        && !session_id.trim().is_empty()
    {
        let mut hits = Vec::new();
        collect_matching(&projects, &mut hits, &mut |path| {
            path.file_stem().and_then(|stem| stem.to_str()) == Some(session_id.as_str())
        });
        if let Some(path) = newest(hits) {
            return Ok(path);
        }
    }

    if request.all_projects {
        return newest_session(&projects, "claude")?.ok_or_else(|| {
            CaptureError::Io(format!(
                "no Claude Code session found in {}",
                projects.display()
            ))
        });
    }

    if let Some(project) = &request.project {
        let directory = {
            let path = Path::new(project);
            if path.is_absolute() {
                path.to_path_buf()
            } else {
                projects.join(project)
            }
        };
        if let Some(path) = newest_claude_sessions(&directory) {
            return Ok(path);
        }
        return Err(CaptureError::Io(format!(
            "no Claude Code session found in {}",
            directory.display()
        )));
    }

    let cwd = request.cwd.clone().or_else(|| env::current_dir().ok());
    if let Some(cwd) = cwd {
        let mut candidates = Vec::new();
        candidates.push(cwd.clone());
        candidates.extend(cwd.ancestors().skip(1).map(Path::to_path_buf));
        for candidate in candidates {
            let directory = projects.join(mangle_cwd(&candidate));
            if let Some(path) = newest_claude_sessions(&directory) {
                return Ok(path);
            }
        }
    }

    newest_session(&projects, "claude")?.ok_or_else(|| {
        CaptureError::Io(
            "no Claude Code session found; run from the project directory or pass --rollout"
                .to_owned(),
        )
    })
}

fn resolve_codex(request: &SessionRequest) -> Result<PathBuf, CaptureError> {
    let home =
        request.home.clone().or_else(home_dir).ok_or_else(|| {
            CaptureError::Io("HOME is not set; pass --rollout explicitly".to_owned())
        })?;
    let sessions = home.join(".codex").join("sessions");
    if let Some(project) = &request.project {
        let path = Path::new(project);
        if path.is_dir()
            && let Some(path) = newest_session(path, "codex")?
        {
            return Ok(path);
        }
        if path.is_file() {
            return Ok(path.to_path_buf());
        }
    }
    newest_session(&sessions, "codex")?.ok_or_else(|| {
        CaptureError::Io(
            "no Codex rollout found; run from the project directory or pass --rollout".to_owned(),
        )
    })
}

fn home_dir() -> Option<PathBuf> {
    env::var_os("HOME").map(PathBuf::from)
}

fn mangle_cwd(path: &Path) -> String {
    path.to_string_lossy().replace('/', "-")
}

fn newest_session(root: &Path, kind: &str) -> Result<Option<PathBuf>, CaptureError> {
    if kind == "claude" {
        let mut files = Vec::new();
        collect_claude_sessions(root, &mut files);
        return Ok(newest(files));
    }
    let mut files = Vec::new();
    collect_codex_rollouts(root, &mut files);
    Ok(newest(files))
}

fn newest_claude_sessions(directory: &Path) -> Option<PathBuf> {
    if !directory.is_dir() {
        return None;
    }
    let mut files = Vec::new();
    collect_claude_sessions(directory, &mut files);
    newest(files)
}

fn collect_claude_sessions(directory: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().and_then(|name| name.to_str()) == Some("subagents") {
                continue;
            }
            collect_claude_sessions(&path, files);
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("jsonl")
            && !path
                .components()
                .any(|part| part.as_os_str() == "subagents")
        {
            files.push(path);
        }
    }
}

fn collect_codex_rollouts(directory: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_codex_rollouts(&path, files);
        } else if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("rollout-") && name.ends_with(".jsonl"))
        {
            files.push(path);
        }
    }
}

fn collect_matching(
    directory: &Path,
    files: &mut Vec<PathBuf>,
    predicate: &mut dyn FnMut(&Path) -> bool,
) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().and_then(|name| name.to_str()) == Some("subagents") {
                continue;
            }
            collect_matching(&path, files, predicate);
        } else if predicate(&path) {
            files.push(path);
        }
    }
}

fn newest(mut files: Vec<PathBuf>) -> Option<PathBuf> {
    files.sort_by_key(|path| {
        fs::metadata(path)
            .and_then(|metadata| metadata.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH)
    });
    files.pop()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_session(path: &Path, content: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut file = fs::File::create(path).unwrap();
        file.write_all(content.as_bytes()).unwrap();
    }

    #[test]
    fn resolves_claude_current_project_and_explicit_rollout() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let project = root.path().join("project");
        let session = home
            .join(".claude/projects")
            .join(mangle_cwd(&project))
            .join("session-a.jsonl");
        write_session(
            &session,
            "{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"hi\"}}\n",
        );
        let request = SessionRequest {
            client: "claude".to_owned(),
            home: Some(home.clone()),
            cwd: Some(project.clone()),
            ..SessionRequest::default()
        };
        assert_eq!(resolve_session(&request).unwrap(), session);

        let explicit = SessionRequest {
            client: "claude".to_owned(),
            rollout: Some(session.clone()),
            ..SessionRequest::default()
        };
        assert_eq!(resolve_session(&explicit).unwrap(), session);
    }

    #[test]
    fn resolves_latest_codex_rollout() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let session = home
            .join(".codex/sessions/2026/08/24")
            .join("rollout-1.jsonl");
        write_session(
            &session,
            "{\"type\":\"session_meta\",\"payload\":{\"session_id\":\"s\"}}\n",
        );
        let request = SessionRequest {
            client: "codex".to_owned(),
            home: Some(home),
            ..SessionRequest::default()
        };
        assert_eq!(resolve_session(&request).unwrap(), session);
    }
}
