//! OpenCode plugin installation for `sideeye setup`.
//!
//! The plugin package is embedded in the binary so `sideeye setup` works from a
//! released artifact with no Python checkout. The installer builds it with npm,
//! then updates `opencode.json(c)` and `cli.json` without dropping JSONC
//! comments or unrelated settings.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use serde_json::{Value, json};

const OPENCODE_FILES: &[(&str, &str)] = &[
    (
        "package.json",
        include_str!("../../../integrations/opencode/package.json"),
    ),
    (
        "package-lock.json",
        include_str!("../../../integrations/opencode/package-lock.json"),
    ),
    (
        "server.js",
        include_str!("../../../integrations/opencode/server.js"),
    ),
    (
        "tui.js",
        include_str!("../../../integrations/opencode/tui.js"),
    ),
    (
        "tsconfig.json",
        include_str!("../../../integrations/opencode/tsconfig.json"),
    ),
    (
        "tsconfig.build.json",
        include_str!("../../../integrations/opencode/tsconfig.build.json"),
    ),
    (
        "src/index.ts",
        include_str!("../../../integrations/opencode/src/index.ts"),
    ),
    (
        "src/tui.tsx",
        include_str!("../../../integrations/opencode/src/tui.tsx"),
    ),
];

pub(crate) fn install_opencode_plugin(
    repo: Option<&Path>,
    model: &str,
    max_tokens: u32,
) -> Result<PathBuf, String> {
    let destination = data_root().join("opencode-plugin");
    fs::create_dir_all(&destination)
        .map_err(|error| format!("cannot create plugin dir: {error}"))?;
    for (relative, content) in OPENCODE_FILES {
        let path = destination.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
        }
        fs::write(&path, content)
            .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
    }

    let npm = which("npm").ok_or("npm is required to build the OpenCode plugin")?;
    for args in [["ci", "--ignore-scripts"], ["run", "build"]] {
        let output = Command::new(&npm)
            .args(args)
            .current_dir(&destination)
            .output()
            .map_err(|error| format!("cannot run npm {}: {error}", args.join(" ")))?;
        if !output.status.success() {
            return Err(format!(
                "OpenCode plugin build failed: npm {}\n{}",
                args.join(" "),
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
    }

    let python = which("sideeye-py").unwrap_or_else(|| PathBuf::from("sideeye-py"));
    let rust = which("sideeye").filter(|path| is_rust_sideeye(path));
    let mut options = json!({
        "command": [python],
        "model": model,
        "maxTokens": max_tokens,
    });
    if let Some(rust) = rust {
        options["engine"] = json!("rust");
        options["rust"] = json!([rust]);
    }
    if let Some(repo) = repo.map(Path::to_path_buf).or_else(git_root) {
        options["repo"] = json!(repo);
    }
    let entry = json!({"package": destination, "options": options});

    write_plugin_config(
        &opencode_config_file(),
        &entry,
        "https://opencode.ai/config.json",
    )?;
    write_plugin_config(
        &cli_config_file(),
        &entry,
        "https://opencode.ai/v2/cli.json",
    )?;
    Ok(destination)
}

fn data_root() -> PathBuf {
    env::var_os("SIDEEYE_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".local/share/sideeye"))
}

fn home() -> PathBuf {
    env::var_os("HOME").map(PathBuf::from).unwrap_or_default()
}

pub(crate) fn opencode_config_file() -> PathBuf {
    if let Some(path) = env::var_os("OPENCODE_CONFIG") {
        return PathBuf::from(path);
    }
    let directory = opencode_config_dir();
    for name in ["opencode.jsonc", "opencode.json"] {
        let candidate = directory.join(name);
        if candidate.exists() {
            return candidate;
        }
    }
    directory.join("opencode.json")
}

fn cli_config_file() -> PathBuf {
    opencode_config_file()
        .parent()
        .map(|parent| parent.join("cli.json"))
        .unwrap_or_else(|| PathBuf::from("cli.json"))
}

fn opencode_config_dir() -> PathBuf {
    if let Some(path) = env::var_os("OPENCODE_CONFIG_DIR") {
        return PathBuf::from(path);
    }
    if let Some(command) = which("opencode") {
        if let Some(real) = real_opencode(&command) {
            let config_home = env::var_os("ENMAAS_OPENCODE_CONFIG_HOME")
                .map(PathBuf::from)
                .or_else(|| wrapper_default(&command, "ENMAAS_OPENCODE_CONFIG_HOME"))
                .unwrap_or_else(|| home().join(".config/opencode"));
            if let Some(directory) = probe_config_dir(&real, Some(&config_home)) {
                return directory;
            }
        }
        if let Some(directory) = probe_config_dir(&command, None) {
            return directory;
        }
    }
    env::var_os("XDG_CONFIG_HOME")
        .map(|value| PathBuf::from(value).join("opencode"))
        .unwrap_or_else(|| home().join(".config/opencode"))
}

fn probe_config_dir(command: &Path, config_home: Option<&Path>) -> Option<PathBuf> {
    let mut command = Command::new(command);
    command.args(["debug", "paths", "config"]);
    if let Some(config_home) = config_home {
        command.env("XDG_CONFIG_HOME", config_home);
    }
    let output = command.output().ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    (!value.is_empty()).then(|| PathBuf::from(value))
}

fn real_opencode(command: &Path) -> Option<PathBuf> {
    if let Some(value) = env::var_os("OPENCODE_REAL_BIN") {
        let path = PathBuf::from(value);
        if path.is_file() {
            return Some(path);
        }
    }
    let text = fs::read_to_string(command).ok()?;
    if !text.contains("--standalone") {
        return None;
    }
    let value = wrapper_value(&text, "OPENCODE_REAL_BIN")?;
    let path = PathBuf::from(value);
    path.is_file().then_some(path)
}

fn wrapper_default(command: &Path, name: &str) -> Option<PathBuf> {
    let text = fs::read_to_string(command).ok()?;
    let value = wrapper_value(&text, name)?;
    let expanded = value.replace("$HOME", &home().to_string_lossy());
    Some(PathBuf::from(expanded))
}

fn wrapper_value(text: &str, name: &str) -> Option<String> {
    let marker = format!("{name}:-");
    let start = text.find(&marker)? + marker.len();
    let rest = &text[start..];
    let end = rest.find(['}', '"', ' ']).unwrap_or(rest.len());
    let value = rest[..end].trim();
    (!value.is_empty()).then(|| value.to_owned())
}

fn which(name: &str) -> Option<PathBuf> {
    env::var_os("PATH").and_then(|paths| {
        env::split_paths(&paths)
            .map(|directory| directory.join(name))
            .find(|candidate| candidate.is_file())
    })
}

fn is_rust_sideeye(path: &Path) -> bool {
    Command::new(path)
        .arg("--version")
        .output()
        .map(|output| {
            output.status.success()
                && String::from_utf8_lossy(&output.stdout)
                    .trim()
                    .to_lowercase()
                    .starts_with("sideeye")
        })
        .unwrap_or(false)
}

fn git_root() -> Option<PathBuf> {
    let output = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    (!value.is_empty()).then(|| PathBuf::from(value))
}

fn write_plugin_config(path: &Path, entry: &Value, schema: &str) -> Result<(), String> {
    if path.exists() {
        let text = fs::read_to_string(path)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        if let Ok(mut value) = serde_json::from_str::<Value>(&text)
            && update_plugins_value(&mut value, entry)
        {
            return write_text_secure(path, &format!("{value:#}\n"));
        }
        let package = entry.get("package").and_then(Value::as_str).unwrap_or("");
        if text.contains(package) {
            return Ok(());
        }
        let updated = insert_plugin_jsonc(&text, entry)?;
        return write_text_secure(path, &updated);
    }
    let value = json!({"$schema": schema, "plugins": [entry]});
    write_text_secure(path, &format!("{value:#}\n"))
}

fn update_plugins_value(value: &mut Value, entry: &Value) -> bool {
    let Some(object) = value.as_object_mut() else {
        return false;
    };
    let Some(array) = object.entry("plugins").or_insert(json!([])).as_array_mut() else {
        return false;
    };
    let package = entry.get("package").and_then(Value::as_str);
    if let Some(existing) = array
        .iter_mut()
        .find(|item| item.get("package").and_then(Value::as_str) == package)
    {
        *existing = entry.clone();
    } else {
        array.push(entry.clone());
    }
    true
}

fn write_text_secure(path: &Path, text: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
    }
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("config");
    let temporary = path.with_file_name(format!(".{name}.{}.tmp", std::process::id()));
    fs::write(&temporary, text)
        .map_err(|error| format!("cannot write {}: {error}", temporary.display()))?;
    #[cfg(unix)]
    fs::set_permissions(&temporary, fs::Permissions::from_mode(0o600))
        .map_err(|error| format!("cannot set mode on {}: {error}", temporary.display()))?;
    fs::rename(&temporary, path)
        .map_err(|error| format!("cannot replace {}: {error}", path.display()))
}

fn insert_plugin_jsonc(text: &str, entry: &Value) -> Result<String, String> {
    let entry_text =
        serde_json::to_string_pretty(entry).map_err(|error| format!("invalid entry: {error}"))?;
    if let Some(key) = find_key(text, "plugins") {
        let open = text[key..]
            .find('[')
            .map(|offset| key + offset)
            .ok_or("plugins entry is not an array")?;
        let close = matching_close(text, open, b'[', b']').ok_or("unclosed plugins array")?;
        let before = text[open + 1..close].trim_end();
        let comma = if before.is_empty() || before.ends_with(',') {
            ""
        } else {
            ","
        };
        return Ok(format!(
            "{}{}{}\n{}\n{}",
            &text[..open + 1],
            before,
            comma,
            indent(&entry_text, 2),
            &text[close..]
        ));
    }
    let close = last_root_close(text).ok_or("config has no root object")?;
    let before = text[..close].trim_end();
    let comma = if before.ends_with('{') || before.ends_with(',') {
        ""
    } else {
        ","
    };
    Ok(format!(
        "{}{}\n  \"plugins\": [\n{}\n  ]\n{}",
        before,
        comma,
        indent(&entry_text, 4),
        &text[close..]
    ))
}

fn indent(text: &str, spaces: usize) -> String {
    let prefix = " ".repeat(spaces);
    text.lines()
        .map(|line| format!("{prefix}{line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn find_key(text: &str, key: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'/' && bytes.get(index + 1) == Some(&b'/') {
            index = text[index + 2..]
                .find('\n')
                .map(|offset| index + 2 + offset + 1)
                .unwrap_or(bytes.len());
            continue;
        }
        if bytes[index] == b'/' && bytes.get(index + 1) == Some(&b'*') {
            index = text[index + 2..]
                .find("*/")
                .map(|offset| index + 2 + offset + 2)
                .unwrap_or(bytes.len());
            continue;
        }
        if bytes[index] != b'"' {
            index += 1;
            continue;
        }
        let start = index;
        index += 1;
        let mut value = String::new();
        while index < bytes.len() {
            match bytes[index] {
                b'\\' if index + 1 < bytes.len() => {
                    value.push(bytes[index + 1] as char);
                    index += 2;
                }
                b'"' => {
                    index += 1;
                    break;
                }
                byte => {
                    value.push(byte as char);
                    index += 1;
                }
            }
        }
        if value != key {
            continue;
        }
        let mut rest = index;
        while rest < bytes.len() && bytes[rest].is_ascii_whitespace() {
            rest += 1;
        }
        if rest < bytes.len() && bytes[rest] == b':' {
            return Some(start);
        }
    }
    None
}

fn matching_close(text: &str, open: usize, left: u8, right: u8) -> Option<usize> {
    scan(text, open, Some((left, right)))
}

fn last_root_close(text: &str) -> Option<usize> {
    scan(text, 0, None)
}

fn scan(text: &str, start: usize, pair: Option<(u8, u8)>) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut index = start;
    let mut depth = 0usize;
    let mut last = None;
    let mut in_string = false;
    let mut escaped = false;
    while index < bytes.len() {
        let byte = bytes[index];
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            index += 1;
            continue;
        }
        if byte == b'/' && bytes.get(index + 1) == Some(&b'/') {
            index = text[index + 2..]
                .find('\n')
                .map(|offset| index + 2 + offset + 1)
                .unwrap_or(bytes.len());
            continue;
        }
        if byte == b'/' && bytes.get(index + 1) == Some(&b'*') {
            index = text[index + 2..]
                .find("*/")
                .map(|offset| index + 2 + offset + 2)
                .unwrap_or(bytes.len());
            continue;
        }
        if byte == b'"' {
            in_string = true;
            index += 1;
            continue;
        }
        match pair {
            Some((left, right)) => {
                if byte == left {
                    depth += 1;
                } else if byte == right {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        return Some(index);
                    }
                }
            }
            None => {
                if byte == b'{' {
                    depth += 1;
                } else if byte == b'}' {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        last = Some(index);
                    }
                }
            }
        }
        index += 1;
    }
    pair.is_none().then_some(last).flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inserts_plugins_into_commented_jsonc() {
        let text = "{\n  // } stray and \"plugins\": [1]\n  \"model\": \"m\"\n}\n";
        let entry = json!({"package": "/plugin", "options": {}});
        let updated = insert_plugin_jsonc(text, &entry).expect("insert");
        assert!(updated.contains("// } stray and \"plugins\": [1]"));
        assert!(updated.contains("\"plugins\": ["));
        assert!(updated.contains("\"package\": \"/plugin\""));
        let stripped = updated
            .lines()
            .map(|line| line.split("//").next().unwrap_or(""))
            .collect::<Vec<_>>()
            .join("\n");
        let parsed: Value = serde_json::from_str(&stripped).expect("valid JSON after comments");
        assert_eq!(parsed["model"], "m");
        assert_eq!(parsed["plugins"][0]["package"], "/plugin");
    }

    #[test]
    fn inserts_into_existing_plugins_array() {
        let text = "{\n  \"plugins\": [\"/one\"]\n}\n";
        let entry = json!({"package": "/two"});
        let updated = insert_plugin_jsonc(text, &entry).expect("insert");
        let parsed: Value = serde_json::from_str(&updated).expect("valid JSON");
        assert_eq!(parsed["plugins"].as_array().unwrap().len(), 2);
        assert_eq!(parsed["plugins"][1]["package"], "/two");
    }

    #[test]
    fn wrapper_setting_is_read() {
        let script = "export XDG_CONFIG_HOME=\"${ENMAAS_OPENCODE_CONFIG_HOME:-$HOME/.config/opencode-enmaas}\"\n";
        assert_eq!(
            wrapper_value(script, "ENMAAS_OPENCODE_CONFIG_HOME").as_deref(),
            Some("$HOME/.config/opencode-enmaas")
        );
    }
}
