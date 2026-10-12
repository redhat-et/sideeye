use std::{
    env, fs,
    io::{self, Read},
    path::PathBuf,
    process::ExitCode,
};

use clap::{Parser, Subcommand};
use serde_json::Value;
use sideeye_core::ReviewPacket;

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
