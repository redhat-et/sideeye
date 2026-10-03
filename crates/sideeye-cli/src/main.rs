use std::{
    env, fs,
    io::{self, Read},
    path::PathBuf,
    process::ExitCode,
};

use clap::{Parser, Subcommand};
use sideeye_core::ReviewPacket;

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
    Doctor,
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
        Command::Doctor => doctor(),
        Command::Validate { packet } => validate(packet),
    }
}

fn doctor() -> Result<(), Box<dyn std::error::Error>> {
    let base = env::var("SIDEEYE_JUDGE_BASE_URL")
        .or_else(|_| env::var("ANTHROPIC_BASE_URL"))
        .unwrap_or_else(|_| "https://api.anthropic.com".to_owned());
    let key_set = env::var("SIDEEYE_JUDGE_API_KEY").is_ok()
        || env::var("ANTHROPIC_API_KEY").is_ok()
        || env::var("ANTHROPIC_AUTH_TOKEN").is_ok();

    println!("sideeye-rs doctor");
    println!("  judge base URL: {base}");
    println!(
        "  judge key:     {}",
        if key_set { "set" } else { "missing" }
    );
    if !key_set {
        return Err(
            "no judge credential configured; set SIDEEYE_JUDGE_API_KEY or ANTHROPIC_API_KEY".into(),
        );
    }
    println!("  verdict:       ready for provider adapter configuration");
    Ok(())
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
