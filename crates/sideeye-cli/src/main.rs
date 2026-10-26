use std::{
    env, fs,
    io::{self, IsTerminal, Read, Write},
    path::{Path, PathBuf},
    process::ExitCode,
};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

mod setup_ext;

use clap::{Parser, Subcommand};
use serde_json::Value;
use sideeye_core::ReviewPacket;
use sideeye_providers::{FakeJudgeProvider, JudgeProvider};

const AMBIENT_BASE_DEFAULT: &str = "https://api.anthropic.com";

#[derive(Debug, Parser)]
#[command(
    name = "sideeye",
    version,
    about = "Provider-neutral Side-Eye core (experimental)"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Show judge-route readiness without making a network request.
    Doctor {
        /// Judge base URL override; useful for an explicit local/gateway route.
        #[arg(long)]
        base_url: Option<String>,
    },
    /// Configure the judge route and detect harnesses.
    Setup {
        /// Judge base URL override.
        #[arg(long)]
        judge_base_url: Option<String>,
        /// Judge API key (prefer the environment or the hidden prompt).
        #[arg(long, hide = true)]
        judge_api_key: Option<String>,
        /// Do not configure the OpenCode plugin.
        #[arg(long)]
        no_opencode: bool,
        /// Git repository root the OpenCode session should review.
        #[arg(long)]
        repo: Option<PathBuf>,
    },
    /// Validate a normalized review packet from a file or stdin.
    Validate {
        /// JSON packet path, or `-` for stdin.
        packet: PathBuf,
    },
    /// Convert an OpenCode V2 export into a ReviewPacket.
    Packet {
        /// OpenCode export JSON, or `-` for stdin.
        #[arg(long)]
        opencode_export: PathBuf,
        /// Redact credential-shaped values before writing the packet.
        #[arg(long)]
        redact: bool,
        /// Git repository root used to collect code evidence.
        #[arg(long)]
        repo: Option<PathBuf>,
        /// Git diff base for tracked files (default: HEAD).
        #[arg(long, default_value = "HEAD")]
        diff_base: String,
        /// File touched by the session; repeat for each path.
        #[arg(long = "touched-file")]
        touched_files: Vec<String>,
        /// Character budget for the packet; 0 disables budgeting.
        #[arg(long, default_value_t = 400_000)]
        max_packet_chars: usize,
    },
    /// Estimate input tokens and cost without calling the judge.
    Estimate {
        /// JSON packet path, or `-` for stdin.
        packet: PathBuf,
        /// Judge base URL override.
        #[arg(long)]
        base_url: Option<String>,
        /// Judge model id.
        #[arg(long, default_value = "claude-opus-4-8")]
        model: String,
        /// Rubric file (defaults to the embedded session rubric).
        #[arg(long)]
        rubric: Option<PathBuf>,
        /// Output token ceiling used for the cost estimate.
        #[arg(long, default_value_t = 4096)]
        max_tokens: u32,
        /// Extra focus for the judge, e.g. "security vulnerabilities".
        #[arg(long)]
        instructions: Option<String>,
    },
    /// Run a packet through a provider and emit a validated verdict.
    Review {
        /// JSON packet path, or `-` for stdin.
        packet: PathBuf,
        /// Provider to use: `fake` or `anthropic`.
        #[arg(long, default_value = "fake")]
        provider: String,
        /// Fixture verdict JSON. Required by the fake provider.
        #[arg(long)]
        fixture: Option<PathBuf>,
        /// Judge base URL override (anthropic provider).
        #[arg(long)]
        base_url: Option<String>,
        /// Judge model id (anthropic provider).
        #[arg(long, default_value = "claude-opus-4-8")]
        model: String,
        /// Rubric file (defaults to the embedded session rubric).
        #[arg(long)]
        rubric: Option<PathBuf>,
        /// Output token ceiling (anthropic provider).
        #[arg(long, default_value_t = 4096)]
        max_tokens: u32,
        /// Extra focus for the judge, e.g. "security vulnerabilities".
        #[arg(long)]
        instructions: Option<String>,
    },
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("sideeye: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    match Cli::parse().command {
        Command::Doctor { base_url } => doctor(base_url),
        Command::Setup {
            judge_base_url,
            judge_api_key,
            no_opencode,
            repo,
        } => setup(judge_base_url, judge_api_key, no_opencode, repo),
        Command::Validate { packet } => validate(packet),
        Command::Packet {
            opencode_export,
            redact,
            repo,
            diff_base,
            touched_files,
            max_packet_chars,
        } => packet_from_opencode(
            opencode_export,
            redact,
            repo,
            diff_base,
            touched_files,
            max_packet_chars,
        ),
        Command::Estimate {
            packet,
            base_url,
            model,
            rubric,
            max_tokens,
            instructions,
        } => estimate(packet, base_url, model, rubric, max_tokens, instructions),
        Command::Review {
            packet,
            provider,
            fixture,
            base_url,
            model,
            rubric,
            max_tokens,
            instructions,
        } => review(
            packet,
            provider,
            fixture,
            base_url,
            model,
            rubric,
            max_tokens,
            instructions,
        ),
    }
}

#[allow(clippy::too_many_arguments)]
fn review(
    path: PathBuf,
    provider_name: String,
    fixture: Option<PathBuf>,
    base_url: Option<String>,
    model: String,
    rubric: Option<PathBuf>,
    max_tokens: u32,
    instructions: Option<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let packet: ReviewPacket = serde_json::from_str(&read_input(path)?)?;
    let packet = packet.redacted();
    let verdict = match provider_name.as_str() {
        "fake" => {
            let fixture = fixture.ok_or("--fixture is required for --provider fake")?;
            let fixture_verdict: sideeye_core::Verdict =
                serde_json::from_str(&fs::read_to_string(fixture)?)?;
            FakeJudgeProvider::new("fake", "fixture-judge", fixture_verdict).judge(&packet)?
        }
        "anthropic" => {
            guard_self_grading(&packet, &model)?;
            anthropic_provider(base_url, model, rubric.as_ref(), max_tokens, instructions)?
                .judge(&packet)?
        }
        other => {
            return Err(format!(
                "provider {other:?} is not implemented; use --provider fake or anthropic"
            )
            .into());
        }
    };
    verdict.validate()?;
    println!("{}", serde_json::to_string(&verdict)?);
    Ok(())
}

fn estimate(
    path: PathBuf,
    base_url: Option<String>,
    model: String,
    rubric: Option<PathBuf>,
    max_tokens: u32,
    instructions: Option<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let packet: ReviewPacket = serde_json::from_str(&read_input(path)?)?;
    let packet = packet.redacted();
    guard_self_grading(&packet, &model)?;
    let estimate = anthropic_provider(base_url, model, rubric.as_ref(), max_tokens, instructions)?
        .estimate(&packet)?;
    println!("{}", serde_json::to_string(&estimate)?);
    Ok(())
}

const EMBEDDED_RUBRIC: &str = include_str!("../../../rubric/rubric_session_v2.md");

fn anthropic_provider(
    base_url: Option<String>,
    model: String,
    rubric: Option<&PathBuf>,
    max_tokens: u32,
    instructions: Option<String>,
) -> Result<sideeye_providers::AnthropicProvider, Box<dyn std::error::Error>> {
    let config = load_judge_config();
    let mut route = resolve_judge_route(&config);
    if base_url.is_some() {
        route.base = base_url;
        route.source = Some("flag");
    }
    validate_route(&route)?;
    let base = route.base.ok_or("no judge base URL configured")?;
    let key = route.key.ok_or("no judge credential configured")?;
    let (rubric_text, rubric_version) = match rubric {
        Some(path) => {
            let text = fs::read_to_string(path)
                .map_err(|error| format!("cannot read rubric {}: {error}", path.display()))?;
            let version = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or("unknown")
                .to_owned();
            (text, version)
        }
        None => (EMBEDDED_RUBRIC.to_owned(), "rubric_session_v2".to_owned()),
    };
    Ok(
        sideeye_providers::AnthropicProvider::new(base, key, model, rubric_text, rubric_version)
            .with_max_tokens(max_tokens)
            .with_instructions(instructions.unwrap_or_default()),
    )
}

/// Refuse a judge that generated any turn in the packet. A model grading its
/// own output is the failure Side-Eye exists to catch.
fn guard_self_grading(packet: &ReviewPacket, judge_model: &str) -> Result<(), String> {
    let family = |model: &str| model.rsplit('/').next().unwrap_or(model).to_lowercase();
    let judge = family(judge_model);
    if let Some(generator) = packet.generator_models.iter().find(|model| {
        let model = family(model);
        model == judge
            || model.starts_with(&format!("{judge}-"))
            || judge.starts_with(&format!("{model}-"))
    }) {
        return Err(format!(
            "judge model {judge_model:?} also generated turns in this packet ({generator}); choose a different judge to avoid self-grading"
        ));
    }
    Ok(())
}

fn packet_from_opencode(
    export_path: PathBuf,
    redact: bool,
    repo: Option<PathBuf>,
    diff_base: String,
    touched_files: Vec<String>,
    max_packet_chars: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    let export: serde_json::Value = serde_json::from_str(&read_input(export_path)?)?;
    let artifacts = match repo {
        Some(repo) => sideeye_capture::collect_diff_artifacts(&repo, &diff_base, &touched_files)?,
        None => Vec::new(),
    };
    let packet = sideeye_capture::from_opencode_export(&export, artifacts)?;
    let packet = if redact { packet.redacted() } else { packet };
    let packet = packet.budgeted(max_packet_chars);
    packet.validate()?;
    println!("{}", serde_json::to_string(&packet)?);
    Ok(())
}

fn read_input(path: PathBuf) -> Result<String, Box<dyn std::error::Error>> {
    if path.as_os_str() == "-" {
        let mut input = String::new();
        io::stdin().read_to_string(&mut input)?;
        Ok(input)
    } else {
        Ok(fs::read_to_string(path)?)
    }
}

fn normalize_base_url(value: &str) -> String {
    let value = value.trim().trim_end_matches('/');
    value.strip_suffix("/v1").unwrap_or(value).to_owned()
}

fn on_path(name: &str) -> bool {
    env::var_os("PATH")
        .map(|paths| env::split_paths(&paths).any(|dir| dir.join(name).is_file()))
        .unwrap_or(false)
}

fn detect_harnesses() -> [(&'static str, bool); 3] {
    let home = env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    [
        (
            "Claude Code",
            on_path("claude") || home.join(".claude").exists(),
        ),
        ("Codex", on_path("codex") || home.join(".codex").exists()),
        ("OpenCode", on_path("opencode")),
    ]
}

fn write_json_secure(path: &Path, value: &serde_json::Value) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("config.json");
    let temporary = path.with_file_name(format!(".{file_name}.{}.tmp", std::process::id()));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&temporary)?;
    #[cfg(unix)]
    file.set_permissions(fs::Permissions::from_mode(0o600))?;
    serde_json::to_writer_pretty(&mut file, value)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    fs::rename(&temporary, path)?;
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(())
}

fn setup(
    base_url: Option<String>,
    api_key: Option<String>,
    no_opencode: bool,
    repo: Option<PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    let path = config_path();
    let mut config: serde_json::Value = if path.exists() {
        serde_json::from_str(&fs::read_to_string(&path)?).unwrap_or_else(|_| serde_json::json!({}))
    } else {
        serde_json::json!({})
    };
    if !config.is_object() {
        config = serde_json::json!({});
    }
    let mut judge = config
        .get("judge")
        .cloned()
        .filter(serde_json::Value::is_object)
        .unwrap_or_else(|| serde_json::json!({}));

    let existing = judge
        .get("api_key")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
    let mut key = api_key
        .or_else(|| env::var("SIDEEYE_JUDGE_API_KEY").ok())
        .or(existing);
    if key.as_deref().map(str::trim).unwrap_or("").is_empty() {
        if !io::stdin().is_terminal() {
            return Err(
                "no judge credential configured; set SIDEEYE_JUDGE_API_KEY or run sideeye setup in an interactive terminal".into(),
            );
        }
        key = Some(rpassword::prompt_password(
            "Side-Eye judge API key (hidden): ",
        )?);
    }
    let key = key.unwrap_or_default();
    if key.trim().is_empty() {
        return Err("judge API key cannot be empty".into());
    }
    judge["api_key"] = serde_json::json!(key.trim());

    let selected = base_url
        .or_else(|| env::var("SIDEEYE_JUDGE_BASE_URL").ok())
        .or_else(|| {
            judge
                .get("base_url")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        });
    if let Some(value) = selected {
        judge["base_url"] = serde_json::json!(normalize_base_url(&value));
    } else if let Some(object) = judge.as_object_mut() {
        object.remove("base_url");
    }
    config["judge"] = judge;
    write_json_secure(&path, &config)?;

    println!("judge route: configured ({})", path.display());
    let harnesses = detect_harnesses();
    for (name, detected) in harnesses {
        println!(
            "{name}: {}",
            if detected { "detected" } else { "not detected" }
        );
    }
    let opencode = harnesses
        .iter()
        .find(|(name, _)| *name == "OpenCode")
        .map(|(_, detected)| *detected)
        .unwrap_or(false);
    if no_opencode || !opencode {
        println!("OpenCode plugin: skipped");
    } else {
        match setup_ext::install_opencode_plugin(repo.as_deref()) {
            Ok(path) => println!("OpenCode plugin: configured ({})", path.display()),
            Err(error) => return Err(error.into()),
        }
    }
    println!("Run `sideeye doctor` to verify the route without spending.");
    Ok(())
}

fn doctor(base_url: Option<String>) -> Result<(), Box<dyn std::error::Error>> {
    let config = load_judge_config();
    let mut route = resolve_judge_route(&config);
    if base_url.is_some() {
        route.base = base_url;
        route.source = Some("flag");
    }

    println!("sideeye doctor");
    println!(
        "  judge base URL: {}",
        route.base.as_deref().unwrap_or("not configured")
    );
    println!(
        "  judge key:     {}",
        if route.key.is_some() {
            "set"
        } else {
            "missing"
        }
    );
    println!("  route source:  {}", route.source.unwrap_or("none"));
    validate_route(&route)?;
    println!("  route guard:   passed");
    println!("  verdict:       ready for provider adapter configuration");
    Ok(())
}

#[derive(Debug, Default, PartialEq, Eq)]
struct JudgeConfig {
    base_url: Option<String>,
    api_key: Option<String>,
    trust_ambient_route: bool,
}

#[derive(Debug, Default, PartialEq, Eq)]
struct ResolvedRoute {
    base: Option<String>,
    key: Option<String>,
    source: Option<&'static str>,
    key_source: Option<&'static str>,
    trust_ambient_route: bool,
}

fn config_path() -> PathBuf {
    if let Ok(path) = env::var("SIDEEYE_CONFIG") {
        return PathBuf::from(path);
    }
    env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("~"))
        .join(".config/sideeye/config.json")
}

fn load_judge_config() -> JudgeConfig {
    // Match the Python oracle: missing, unreadable, malformed, and non-object
    // configs are treated as empty so route errors remain actionable.
    let Ok(text) = fs::read_to_string(config_path()) else {
        return JudgeConfig::default();
    };
    let Ok(root) = serde_json::from_str::<Value>(&text) else {
        return JudgeConfig::default();
    };
    let Some(judge) = root.get("judge").and_then(Value::as_object) else {
        return JudgeConfig::default();
    };
    JudgeConfig {
        base_url: judge
            .get("base_url")
            .and_then(Value::as_str)
            .map(str::to_owned),
        api_key: judge
            .get("api_key")
            .and_then(Value::as_str)
            .map(str::to_owned),
        trust_ambient_route: judge
            .get("trust_ambient_route")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    }
}

fn resolve_judge_route(config: &JudgeConfig) -> ResolvedRoute {
    let env_base = nonempty_env("SIDEEYE_JUDGE_BASE_URL");
    let env_key = nonempty_env("SIDEEYE_JUDGE_API_KEY");
    let ambient_base = nonempty_env("ANTHROPIC_BASE_URL");
    let ambient_key =
        nonempty_env("ANTHROPIC_AUTH_TOKEN").or_else(|| nonempty_env("ANTHROPIC_API_KEY"));

    resolve_judge_route_values(
        env_base.as_deref(),
        env_key.as_deref(),
        ambient_base.as_deref(),
        ambient_key.as_deref(),
        config,
    )
}

fn resolve_judge_route_values(
    env_base: Option<&str>,
    env_key: Option<&str>,
    ambient_base: Option<&str>,
    ambient_key: Option<&str>,
    config: &JudgeConfig,
) -> ResolvedRoute {
    let (base, source) = if let Some(base) = env_base {
        (Some(base.to_owned()), Some("env"))
    } else if let Some(base) = config.base_url.as_deref() {
        (Some(base.to_owned()), Some("config"))
    } else if let Some(base) = ambient_base {
        (Some(base.to_owned()), Some("ambient"))
    } else if env_key.is_some() || config.api_key.is_some() {
        (Some(AMBIENT_BASE_DEFAULT.to_owned()), Some("env"))
    } else if ambient_key.is_some() {
        (Some(AMBIENT_BASE_DEFAULT.to_owned()), Some("ambient"))
    } else {
        (None, None)
    };

    let (key, key_source) = if let Some(key) = env_key {
        (Some(key.to_owned()), Some("env"))
    } else if let Some(key) = config.api_key.as_deref() {
        (Some(key.to_owned()), Some("config"))
    } else if let Some(key) = ambient_key {
        (Some(key.to_owned()), Some("ambient"))
    } else {
        (None, None)
    };
    ResolvedRoute {
        base,
        key,
        source,
        key_source,
        trust_ambient_route: config.trust_ambient_route,
    }
}

fn nonempty_env(name: &str) -> Option<String> {
    env::var(name).ok().filter(|value| !value.trim().is_empty())
}

fn validate_route(route: &ResolvedRoute) -> Result<(), Box<dyn std::error::Error>> {
    let Some(base) = route.base.as_deref() else {
        return Err("no judge route configured; set SIDEEYE_JUDGE_BASE_URL and SIDEEYE_JUDGE_API_KEY, configure SIDEEYE_CONFIG, or set ANTHROPIC_API_KEY".into());
    };
    if route.key.is_none() {
        return Err("judge base URL is configured but no credential is set; set SIDEEYE_JUDGE_API_KEY or configure judge.api_key".into());
    }

    for (needle, description) in [
        ("127.0.0.1:818", "the local GLM gateway"),
        ("localhost:818", "the local GLM gateway"),
        ("ai-gateway-qwen", "the dogfood Qwen route"),
    ] {
        if base.contains(needle) {
            return Err(format!(
                "judge route points at {description} ({base}); the judge must use the real Claude route"
            )
            .into());
        }
    }

    if route.source == Some("ambient")
        && !route.trust_ambient_route
        && !is_anthropic_host(host_from_url(base).as_deref())
    {
        return Err(format!(
            "the ambient ANTHROPIC route ({base}) is not an Anthropic host and may be this session's cheap model; set SIDEEYE_JUDGE_BASE_URL or configure judge.trust_ambient_route=true"
        )
        .into());
    }
    // An explicit base can be a gateway. Sending an ambient Anthropic
    // credential to it would leak the key to an unrelated host unless the
    // user explicitly trusts that route.
    if route.key_source == Some("ambient")
        && route.source != Some("ambient")
        && !route.trust_ambient_route
        && !is_anthropic_host(host_from_url(base).as_deref())
    {
        return Err(format!(
            "refusing to send the ambient ANTHROPIC credential to {base}; set SIDEEYE_JUDGE_API_KEY for this route or configure judge.trust_ambient_route=true"
        )
        .into());
    }
    Ok(())
}

fn host_from_url(url: &str) -> Option<String> {
    let authority = url
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(url)
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default();
    let host = authority.rsplit('@').next().unwrap_or(authority);
    let host = host.strip_prefix('[').unwrap_or(host);
    let host = host.split(']').next().unwrap_or(host);
    let host = host.split(':').next().unwrap_or(host);
    (!host.is_empty()).then(|| host.to_owned())
}

fn is_anthropic_host(host: Option<&str>) -> bool {
    host == Some("api.anthropic.com") || host.is_some_and(|h| h.ends_with(".anthropic.com"))
}

fn validate(path: PathBuf) -> Result<(), Box<dyn std::error::Error>> {
    let mut input = String::new();
    if path.as_os_str() == "-" {
        io::stdin().read_to_string(&mut input)?;
    } else {
        input = fs::read_to_string(path)?;
    }
    let packet: ReviewPacket = serde_json::from_str(&input)?;
    packet.validate()?;
    println!(
        "valid review packet: session={} source={} turns={} artifacts={}",
        packet.session_id,
        packet.source,
        packet.turns.len(),
        packet.artifacts.len()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_precedence_matches_python_oracle() {
        let config = JudgeConfig {
            base_url: Some("https://config.example".to_owned()),
            api_key: Some("config-key".to_owned()),
            trust_ambient_route: false,
        };
        let route = resolve_judge_route_values(
            Some("https://env.example"),
            Some("env-key"),
            Some("https://ambient.example"),
            Some("ambient-key"),
            &config,
        );
        assert_eq!(route.base.as_deref(), Some("https://env.example"));
        assert_eq!(route.key.as_deref(), Some("env-key"));
        assert_eq!(route.source, Some("env"));
        assert_eq!(route.key_source, Some("env"));
    }

    #[test]
    fn ambient_key_is_not_sent_to_an_explicit_foreign_base() {
        let route = resolve_judge_route_values(
            Some("https://explicit.example"),
            None,
            None,
            Some("ambient-key"),
            &JudgeConfig::default(),
        );
        assert_eq!(route.base.as_deref(), Some("https://explicit.example"));
        assert!(validate_route(&route).is_err());
    }

    #[test]
    fn ambient_private_route_is_blocked_without_explicit_trust() {
        let route = resolve_judge_route_values(
            None,
            None,
            Some("http://127.0.0.1:8787"),
            Some("ambient-key"),
            &JudgeConfig::default(),
        );
        assert!(validate_route(&route).is_err());
    }

    #[test]
    fn explicitly_configured_private_route_is_allowed() {
        let route = resolve_judge_route_values(
            Some("http://127.0.0.1:8787"),
            Some("judge-key"),
            None,
            None,
            &JudgeConfig::default(),
        );
        assert!(validate_route(&route).is_ok());
    }

    #[test]
    fn self_grading_guard_rejects_same_model_family() {
        let packet = ReviewPacket {
            schema_version: sideeye_core::REVIEW_PACKET_SCHEMA.to_owned(),
            packet_id: "p".to_owned(),
            session_id: "s".to_owned(),
            source: "opencode".to_owned(),
            client: "opencode".to_owned(),
            generator_model: Some("anthropic/claude-opus-4-8".to_owned()),
            task: "task".to_owned(),
            turns: vec![sideeye_core::Turn {
                role: sideeye_core::Role::User,
                text: "task".to_owned(),
            }],
            artifacts: vec![],
            generation_usage: None,
            generator_models: vec!["anthropic/claude-opus-4-8".to_owned()],
        };
        assert!(guard_self_grading(&packet, "claude-opus-4-8").is_err());
        assert!(guard_self_grading(&packet, "claude-sonnet-5").is_ok());
    }

    #[test]
    fn anthropic_host_parser_handles_ports() {
        assert_eq!(
            host_from_url("https://api.anthropic.com/v1"),
            Some("api.anthropic.com".to_owned())
        );
        assert!(is_anthropic_host(
            host_from_url("https://gateway.anthropic.com:443").as_deref()
        ));
    }
}
