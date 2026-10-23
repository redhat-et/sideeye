use std::{
    env, fs,
    io::{self, Read},
    path::PathBuf,
    process::ExitCode,
};

use clap::{Parser, Subcommand};
use serde_json::Value;
use sideeye_core::ReviewPacket;
use sideeye_providers::{FakeJudgeProvider, JudgeProvider};

const AMBIENT_BASE_DEFAULT: &str = "https://api.anthropic.com";

#[derive(Debug, Parser)]
#[command(
    name = "sideeye-rs",
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
        /// Rubric file (anthropic provider).
        #[arg(long, default_value = "rubric/rubric_session_v2.md")]
        rubric: PathBuf,
        /// Output token ceiling (anthropic provider).
        #[arg(long, default_value_t = 4096)]
        max_tokens: u32,
    },
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("sideeye-rs: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    match Cli::parse().command {
        Command::Doctor { base_url } => doctor(base_url),
        Command::Validate { packet } => validate(packet),
        Command::Packet {
            opencode_export,
            redact,
        } => packet_from_opencode(opencode_export, redact),
        Command::Review {
            packet,
            provider,
            fixture,
            base_url,
            model,
            rubric,
            max_tokens,
        } => review(
            packet, provider, fixture, base_url, model, rubric, max_tokens,
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
    rubric: PathBuf,
    max_tokens: u32,
) -> Result<(), Box<dyn std::error::Error>> {
    let packet: ReviewPacket = serde_json::from_str(&read_input(path)?)?;
    let verdict = match provider_name.as_str() {
        "fake" => {
            let fixture = fixture.ok_or("--fixture is required for --provider fake")?;
            let fixture_verdict: sideeye_core::Verdict =
                serde_json::from_str(&fs::read_to_string(fixture)?)?;
            FakeJudgeProvider::new("fake", "fixture-judge", fixture_verdict).judge(&packet)?
        }
        "anthropic" => {
            let config = load_judge_config();
            let mut route = resolve_judge_route(&config);
            if base_url.is_some() {
                route.base = base_url;
                route.source = Some("flag");
            }
            validate_route(&route)?;
            let base = route.base.ok_or("no judge base URL configured")?;
            let key = route.key.ok_or("no judge credential configured")?;
            let rubric_text = fs::read_to_string(&rubric)
                .map_err(|error| format!("cannot read rubric {}: {error}", rubric.display()))?;
            let rubric_version = rubric
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or("unknown")
                .to_owned();
            sideeye_providers::AnthropicProvider::new(base, key, model, rubric_text, rubric_version)
                .with_max_tokens(max_tokens)
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

fn packet_from_opencode(
    export_path: PathBuf,
    redact: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let export: serde_json::Value = serde_json::from_str(&read_input(export_path)?)?;
    let packet = sideeye_capture::from_opencode_export(&export, Vec::new())?;
    let packet = if redact { packet.redacted() } else { packet };
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

fn doctor(base_url: Option<String>) -> Result<(), Box<dyn std::error::Error>> {
    let config = load_judge_config();
    let mut route = resolve_judge_route(&config);
    if base_url.is_some() {
        route.base = base_url;
        route.source = Some("flag");
    }

    println!("sideeye-rs doctor");
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

    ResolvedRoute {
        base,
        key: env_key
            .map(str::to_owned)
            .or_else(|| config.api_key.clone())
            .or_else(|| ambient_key.map(str::to_owned)),
        source,
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
    }

    #[test]
    fn key_only_defaults_to_direct_anthropic() {
        let route = resolve_judge_route_values(
            Some("https://explicit.example"),
            None,
            None,
            Some("ambient-key"),
            &JudgeConfig::default(),
        );
        assert_eq!(route.base.as_deref(), Some("https://explicit.example"));
        assert_eq!(route.key.as_deref(), Some("ambient-key"));
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
