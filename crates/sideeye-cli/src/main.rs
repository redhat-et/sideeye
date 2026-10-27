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
use dialoguer::{Confirm, Input, Select};
use serde_json::Value;
use sideeye_capture::SessionRequest;
use sideeye_core::ReviewPacket;
use sideeye_providers::{FakeJudgeProvider, JudgeAdvisor, JudgeProvider};

const AMBIENT_BASE_DEFAULT: &str = "https://api.anthropic.com";

#[derive(Debug, Parser)]
#[command(
    name = "sideeye",
    version,
    about = "Ask a strong judge to review an AI coding session",
    long_about = "Side-Eye is an independent quality-control judge for AI coding sessions.\n\nRun `sideeye setup` once, then ask for a review from your harness. The judge is a different model from the one that wrote the code.",
    after_help = "Examples:\n  sideeye setup\n  sideeye setup --repo ~/code/project\n  sideeye doctor\n  sideeye doctor --verify\n\nOpenCode:\n  run /sideeye after setup"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Show judge-route readiness and safety guards.
    #[command(
        long_about = "Show which judge route and credential Side-Eye resolves, and whether the safety guards pass. This is offline by default. Add --verify for a free count_tokens check that proves the route, key, and model work without spending.",
        after_help = "Examples:\n  sideeye doctor\n  sideeye doctor --verify\n  sideeye doctor --base-url http://127.0.0.1:1234"
    )]
    Doctor {
        /// Judge base URL override; useful for an explicit local/gateway route.
        #[arg(long)]
        base_url: Option<String>,
        /// Make a free count_tokens request to verify the route and model.
        #[arg(long)]
        verify: bool,
        /// Judge model to check with --verify.
        #[arg(long, default_value = "claude-opus-4-8")]
        model: String,
        /// Provider to check: `anthropic` or `openai-compatible`.
        #[arg(long)]
        provider: Option<String>,
    },
    /// Configure the judge route and install the harness plugin.
    #[command(
        long_about = "Configure Side-Eye in one guided flow.\n\nIn a terminal, setup asks only for what it needs, installs the detected harness plugin, and can verify the judge route with a free check (`count_tokens` or `/v1/models`). The API key prompt is hidden and the key is never printed. Use --yes for scripts.",
        after_help = "Examples:\n  sideeye setup\n  sideeye setup --repo ~/code/project\n  sideeye setup --model claude-opus-4-8 --max-tokens 4096\n  sideeye setup --provider openai-compatible --judge-base-url http://127.0.0.1:11434 --judge-api-key local --model qwen2.5-coder\n  sideeye setup --judge-base-url https://gateway.example --no-opencode --yes\n\nThe key can also come from SIDEEYE_JUDGE_API_KEY; the base URL from SIDEEYE_JUDGE_BASE_URL."
    )]
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
        /// Do not install the Claude Code or Codex skill.
        #[arg(long)]
        no_skills: bool,
        /// Git repository root the OpenCode session should review.
        #[arg(long)]
        repo: Option<PathBuf>,
        /// Judge provider: `anthropic` or `openai-compatible`.
        #[arg(long, default_value = "anthropic")]
        provider: String,
        /// Judge model id written to the plugin options.
        #[arg(long, default_value = "claude-opus-4-8")]
        model: String,
        /// Judge output token ceiling written to the plugin options.
        /// Defaults to 8192 for Anthropic and 16384 for OpenAI-compatible routes.
        #[arg(long)]
        max_tokens: Option<u32>,
        /// Accept the supplied/default answers without interactive prompts.
        #[arg(long)]
        yes: bool,
        /// Run a free route check after writing the config.
        #[arg(long)]
        verify: bool,
        /// Skip the free route check in the interactive wizard.
        #[arg(long)]
        no_verify: bool,
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
        /// Review scope: `session` for all context, `last-exchange` for the last request.
        #[arg(long, default_value = "session")]
        scope: String,
    },
    /// Review the current Claude Code or Codex session.
    Session {
        /// Harness: `claude` or `codex`.
        #[arg(long, default_value = "claude")]
        client: String,
        /// Explicit transcript/rollout path.
        #[arg(long)]
        rollout: Option<PathBuf>,
        /// Prefer the current harness session.
        #[arg(long)]
        current: bool,
        /// Search every project instead of the current project and ancestors.
        #[arg(long)]
        all_projects: bool,
        /// Specific Claude project directory or Codex path.
        #[arg(long)]
        project: Option<String>,
        /// Skip the git diff and review narrative only.
        #[arg(long)]
        no_code: bool,
        /// Print the free estimate instead of judging.
        #[arg(long)]
        estimate_only: bool,
        /// Provider override; defaults to the saved config.
        #[arg(long)]
        provider: Option<String>,
        /// Judge base URL override.
        #[arg(long)]
        base_url: Option<String>,
        /// Judge model override; defaults to the saved config.
        #[arg(long)]
        model: Option<String>,
        /// Output token ceiling override.
        #[arg(long)]
        max_tokens: Option<u32>,
        /// Extra focus for the judge.
        #[arg(long)]
        instructions: Option<String>,
        /// Git repository root for the diff (default: current checkout).
        #[arg(long)]
        repo: Option<PathBuf>,
        /// Git diff base (default: merge-base with origin/main, main, or master).
        #[arg(long)]
        diff_base: Option<String>,
        /// Append the raw verdict record to this JSONL file.
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Ask for a free-form second opinion on recent Claude Code or Codex work.
    Advise {
        /// Harness: `claude` or `codex`.
        #[arg(long, default_value = "claude")]
        client: String,
        /// Explicit transcript/rollout path.
        #[arg(long)]
        rollout: Option<PathBuf>,
        /// Prefer the current harness session.
        #[arg(long)]
        current: bool,
        /// Search every project instead of the current project and ancestors.
        #[arg(long)]
        all_projects: bool,
        /// Specific Claude project directory or Codex path.
        #[arg(long)]
        project: Option<String>,
        /// Number of recent user/assistant exchanges to include.
        #[arg(long, default_value_t = 1)]
        turns: usize,
        /// Skip the git diff.
        #[arg(long)]
        no_code: bool,
        /// The judgement call you want an opinion on.
        #[arg(long, default_value = "")]
        question: String,
        /// Provider override; defaults to the saved config.
        #[arg(long)]
        provider: Option<String>,
        /// Judge base URL override.
        #[arg(long)]
        base_url: Option<String>,
        /// Judge model override; defaults to the saved config.
        #[arg(long)]
        model: Option<String>,
        /// Output token ceiling override.
        #[arg(long)]
        max_tokens: Option<u32>,
        /// Git repository root for the diff (default: current checkout).
        #[arg(long)]
        repo: Option<PathBuf>,
        /// Git diff base.
        #[arg(long)]
        diff_base: Option<String>,
        /// Append the advice record to this JSONL file.
        #[arg(long)]
        out: Option<PathBuf>,
        /// Print one JSON record instead of human-readable text.
        #[arg(long)]
        json: bool,
    },
    /// Label a stored verdict for local judge-success metrics.
    Label {
        /// Label: `confirmed`, `false-positive`, or `neutral`.
        label: String,
        /// Record id to label; defaults to the newest verdict in the ledger.
        #[arg(long)]
        id: Option<String>,
        /// Verdict ledger to read.
        #[arg(long)]
        ledger: Option<PathBuf>,
        /// Label ledger to append to.
        #[arg(long)]
        labels: Option<PathBuf>,
    },
    /// Show local judge-success metrics.
    Stats {
        /// Verdict ledger to read.
        #[arg(long)]
        ledger: Option<PathBuf>,
        /// Label ledger to read.
        #[arg(long)]
        labels: Option<PathBuf>,
        /// Print one JSON object.
        #[arg(long)]
        json: bool,
    },
    /// Estimate input tokens and cost without calling the judge.
    Estimate {
        /// JSON packet path, or `-` for stdin.
        packet: PathBuf,
        /// Provider: `anthropic` or `openai-compatible`.
        #[arg(long, default_value = "anthropic")]
        provider: String,
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
        /// Provider: `fake`, `anthropic`, or `openai-compatible`.
        #[arg(long, default_value = "fake")]
        provider: String,
        /// Fixture verdict JSON. Required by the fake provider.
        #[arg(long)]
        fixture: Option<PathBuf>,
        /// Judge base URL override.
        #[arg(long)]
        base_url: Option<String>,
        /// Judge model id.
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
        /// Append the raw verdict record to this JSONL file.
        #[arg(long)]
        out: Option<PathBuf>,
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
        Command::Doctor {
            base_url,
            verify,
            model,
            provider,
        } => doctor(base_url, verify, model, provider),
        Command::Setup {
            judge_base_url,
            judge_api_key,
            no_opencode,
            no_skills,
            repo,
            provider,
            model,
            max_tokens,
            yes,
            verify,
            no_verify,
        } => setup(
            judge_base_url,
            judge_api_key,
            no_opencode,
            no_skills,
            repo,
            provider,
            model,
            max_tokens,
            yes,
            verify,
            no_verify,
        ),
        Command::Validate { packet } => validate(packet),
        Command::Packet {
            opencode_export,
            redact,
            repo,
            diff_base,
            touched_files,
            max_packet_chars,
            scope,
        } => packet_from_opencode(
            opencode_export,
            redact,
            repo,
            diff_base,
            touched_files,
            max_packet_chars,
            scope,
        ),
        Command::Session {
            client,
            rollout,
            current,
            all_projects,
            project,
            no_code,
            estimate_only,
            provider,
            base_url,
            model,
            max_tokens,
            instructions,
            repo,
            diff_base,
            out,
        } => session_review(
            client,
            rollout,
            current,
            all_projects,
            project,
            no_code,
            estimate_only,
            provider,
            base_url,
            model,
            max_tokens,
            instructions,
            repo,
            diff_base,
            out,
        ),
        Command::Advise {
            client,
            rollout,
            current,
            all_projects,
            project,
            turns,
            no_code,
            question,
            provider,
            base_url,
            model,
            max_tokens,
            repo,
            diff_base,
            out,
            json,
        } => advise_command(
            client,
            rollout,
            current,
            all_projects,
            project,
            turns,
            no_code,
            question,
            provider,
            base_url,
            model,
            max_tokens,
            repo,
            diff_base,
            out,
            json,
        ),
        Command::Label {
            label,
            id,
            ledger,
            labels,
        } => label_command(label, id, ledger, labels),
        Command::Stats {
            ledger,
            labels,
            json,
        } => stats_command(ledger, labels, json),
        Command::Estimate {
            packet,
            provider,
            base_url,
            model,
            rubric,
            max_tokens,
            instructions,
        } => estimate(
            packet,
            provider,
            base_url,
            model,
            rubric,
            max_tokens,
            instructions,
        ),
        Command::Review {
            packet,
            provider,
            fixture,
            base_url,
            model,
            rubric,
            max_tokens,
            instructions,
            out,
        } => review(
            packet,
            provider,
            fixture,
            base_url,
            model,
            rubric,
            max_tokens,
            instructions,
            out,
        ),
    }
}

#[allow(clippy::too_many_arguments)]
fn advise_command(
    client: String,
    rollout: Option<PathBuf>,
    current: bool,
    all_projects: bool,
    project: Option<String>,
    turns: usize,
    no_code: bool,
    question: String,
    provider: Option<String>,
    base_url: Option<String>,
    model: Option<String>,
    max_tokens: Option<u32>,
    repo: Option<PathBuf>,
    diff_base: Option<String>,
    out: Option<PathBuf>,
    json: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let config = load_judge_config();
    let provider_name = provider
        .or(config.provider.clone())
        .unwrap_or_else(|| "anthropic".to_owned());
    let model = model
        .or(config.model.clone())
        .unwrap_or_else(|| "claude-opus-4-8".to_owned());
    let max_tokens = max_tokens.or(config.max_tokens).unwrap_or(4096);
    let request = SessionRequest {
        client,
        rollout,
        current,
        all_projects,
        project,
        home: None,
        cwd: None,
    };
    let session_path = sideeye_capture::resolve_session(&request)?;
    let captured = sideeye_capture::parse_session_file(&session_path)?;
    let selected = recent_turns(&captured.turns, turns);
    let mut context = render_turns(&selected);

    if !no_code && let Some(repo) = repo.or_else(current_git_root) {
        let base = diff_base.unwrap_or_else(|| default_diff_base(&repo));
        let touched = captured
            .touched_files
            .iter()
            .filter(|file| context.contains(&file.path))
            .map(|file| file.path.clone())
            .collect::<Vec<_>>();
        let artifacts = if touched.is_empty() {
            sideeye_capture::collect_diff_artifacts_all(&repo, &base)?
        } else {
            sideeye_capture::collect_diff_artifacts(&repo, &base, &touched)?
        };
        for artifact in artifacts {
            context.push_str(&format!(
                "\n\n## {} evidence\n{}",
                artifact.kind, artifact.content
            ));
        }
    }

    let (base, key, rubric, rubric_version) =
        provider_material(base_url, None, EMBEDDED_ADVICE_RUBRIC, "rubric_advice_v1")?;
    let advice = match provider_name.as_str() {
        "anthropic" => sideeye_providers::AnthropicProvider::new(
            base,
            key,
            model.clone(),
            rubric,
            rubric_version,
        )
        .with_max_tokens(max_tokens)
        .advise(&question, &context)?,
        "openai" | "openai-compatible" => {
            sideeye_providers::OpenAiProvider::new(base, key, model.clone(), rubric, rubric_version)
                .with_max_tokens(max_tokens)
                .advise(&question, &context)?
        }
        other => {
            return Err(format!(
                "provider {other:?} is not implemented; use anthropic or openai-compatible"
            )
            .into());
        }
    };

    let record = serde_json::json!({
        "session_id": captured.session_id,
        "source": "advice",
        "question": question,
        "advice": advice.text,
        "judge_provider": advice.provider,
        "judge_model": advice.model,
        "input_tokens": advice.input_tokens,
        "output_tokens": advice.output_tokens,
        "cost_usd": advice.cost_usd,
    });
    if let Some(path) = out {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        writeln!(file, "{}", serde_json::to_string(&record)?)?;
    }
    if json {
        println!("{}", serde_json::to_string(&record)?);
    } else {
        println!("sideeye · advice");
        println!("  session: {}", captured.session_id);
        println!(
            "  scope:   {}",
            if turns <= 1 {
                "last exchange".to_owned()
            } else {
                format!("last {turns} exchanges")
            }
        );
        println!(
            "  judge:   {}",
            record["judge_model"].as_str().unwrap_or("unknown")
        );
        println!();
        println!("SECOND OPINION");
        println!("{}", record["advice"].as_str().unwrap_or(""));
    }
    Ok(())
}

fn recent_turns(turns: &[sideeye_core::Turn], count: usize) -> Vec<sideeye_core::Turn> {
    let user_positions = turns
        .iter()
        .enumerate()
        .filter(|(_, turn)| turn.role == sideeye_core::Role::User)
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    let start = user_positions
        .len()
        .saturating_sub(count.max(1))
        .min(user_positions.len().saturating_sub(1));
    let start = user_positions.get(start).copied().unwrap_or(0);
    turns[start..].to_vec()
}

fn render_turns(turns: &[sideeye_core::Turn]) -> String {
    turns
        .iter()
        .map(|turn| {
            let label = match turn.role {
                sideeye_core::Role::User => "USER",
                sideeye_core::Role::Assistant => "ASSISTANT",
                sideeye_core::Role::Tool => "TOOL",
                sideeye_core::Role::System => "SYSTEM",
            };
            format!("{label}:\n{}\n", turn.text.trim())
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[allow(clippy::too_many_arguments)]
fn session_review(
    client: String,
    rollout: Option<PathBuf>,
    current: bool,
    all_projects: bool,
    project: Option<String>,
    no_code: bool,
    estimate_only: bool,
    provider: Option<String>,
    base_url: Option<String>,
    model: Option<String>,
    max_tokens: Option<u32>,
    instructions: Option<String>,
    repo: Option<PathBuf>,
    diff_base: Option<String>,
    out: Option<PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    let config = load_judge_config();
    let provider_name = provider
        .or(config.provider.clone())
        .unwrap_or_else(|| "anthropic".to_owned());
    let model = model
        .or(config.model.clone())
        .unwrap_or_else(|| "claude-opus-4-8".to_owned());
    let max_tokens = max_tokens.or(config.max_tokens).unwrap_or(4096);
    let request = SessionRequest {
        client,
        rollout,
        current,
        all_projects,
        project,
        home: None,
        cwd: None,
    };
    let session_path = sideeye_capture::resolve_session(&request)?;
    let captured = sideeye_capture::parse_session_file(&session_path)?;
    let client_name = captured.client.clone();
    let touched = captured
        .touched_files
        .iter()
        .map(|file| file.path.clone())
        .collect::<Vec<_>>();

    let repo = repo.or_else(current_git_root);
    let artifacts = if no_code {
        Vec::new()
    } else if let Some(repo) = &repo {
        let base = diff_base.unwrap_or_else(|| default_diff_base(repo));
        if touched.is_empty() {
            sideeye_capture::collect_diff_artifacts_all(repo, &base)?
        } else {
            sideeye_capture::collect_diff_artifacts(repo, &base, &touched)?
        }
    } else {
        Vec::new()
    };
    let judge = build_provider(
        &provider_name,
        base_url,
        model.clone(),
        None,
        max_tokens,
        instructions,
    )?;
    let mut max_chars = 400_000_usize;
    let mut packet = None;
    let mut estimate_output = None;
    let mut last_error = None;
    for attempt in 0..7 {
        let candidate = captured
            .clone()
            .into_packet(artifacts.clone())?
            .redacted()
            .budgeted(max_chars);
        candidate.validate()?;
        guard_self_grading(&candidate, &model)?;
        match judge.estimate(&candidate) {
            Ok(estimate) => {
                packet = Some(candidate);
                estimate_output = Some(estimate);
                break;
            }
            Err(error) if is_context_error(&error) && attempt < 6 => {
                last_error = Some(error);
                max_chars = (max_chars * 6 / 10).max(20_000);
            }
            Err(error) => return Err(error.into()),
        }
    }
    let packet = packet.ok_or_else(|| {
        format!(
            "could not fit this session into the judge context even after trimming; try --no-code, a smaller scope, or a larger-context judge{}",
            last_error
                .map(|error| format!("\n\n{error}"))
                .unwrap_or_default()
        )
    })?;
    if estimate_only {
        println!("{}", serde_json::to_string(&estimate_output)?);
        return Ok(());
    }
    let verdict = judge.judge(&packet)?;
    verdict.validate()?;
    let out = out.or_else(|| Some(state_home().join(format!("{client_name}-verdicts.jsonl"))));
    if let Some(path) = out {
        persist_verdict(&path, &packet, &verdict)?;
    }
    println!("{}", serde_json::to_string(&verdict)?);
    Ok(())
}

fn is_context_error(error: &sideeye_providers::ProviderError) -> bool {
    let text = error.to_string();
    text.contains("context window") || text.contains("estimated packet is")
}

fn default_diff_base(repo: &Path) -> String {
    for reference in ["origin/main", "main", "master"] {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(["merge-base", "HEAD", reference])
            .output();
        if let Ok(output) = output
            && output.status.success()
        {
            let value = String::from_utf8_lossy(&output.stdout).trim().to_owned();
            if !value.is_empty() {
                return value;
            }
        }
    }
    "HEAD".to_owned()
}

fn persist_verdict(
    path: &Path,
    packet: &ReviewPacket,
    verdict: &sideeye_core::Verdict,
) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let record = serde_json::json!({
        "record_id": new_record_id(&packet.packet_id),
        "session_id": packet.session_id,
        "source": packet.source,
        "verdict": verdict,
    });
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    writeln!(file, "{}", serde_json::to_string(&record)?)?;
    Ok(())
}

fn new_record_id(packet_id: &str) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{packet_id}-{nanos}")
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
    out: Option<PathBuf>,
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
        "anthropic" | "openai" | "openai-compatible" => {
            guard_self_grading(&packet, &model)?;
            build_provider(
                &provider_name,
                base_url,
                model,
                rubric.as_ref(),
                max_tokens,
                instructions,
            )?
            .judge(&packet)?
        }
        other => {
            return Err(format!(
                "provider {other:?} is not implemented; use --provider fake, anthropic, or openai-compatible"
            )
            .into());
        }
    };
    verdict.validate()?;
    if let Some(path) = out {
        persist_verdict(&path, &packet, &verdict)?;
    }
    println!("{}", serde_json::to_string(&verdict)?);
    Ok(())
}

fn label_command(
    label: String,
    id: Option<String>,
    ledger: Option<PathBuf>,
    labels: Option<PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    let label = normalize_label(&label)?;
    let ledger = ledger.unwrap_or_else(default_verdict_ledger);
    let records = read_jsonl(&ledger)?;
    let record = match id {
        Some(id) => records
            .iter()
            .find(|record| record.get("record_id").and_then(Value::as_str) == Some(id.as_str()))
            .ok_or_else(|| format!("no verdict with record_id {id:?} in {}", ledger.display()))?,
        None => records
            .last()
            .ok_or_else(|| format!("verdict ledger is empty: {}", ledger.display()))?,
    };
    let record_id = record
        .get("record_id")
        .and_then(Value::as_str)
        .ok_or("this verdict has no record_id; rerun the review with 0.6.7 or newer")?;
    let labels = labels.unwrap_or_else(default_labels_ledger);
    if let Some(parent) = labels.parent() {
        fs::create_dir_all(parent)?;
    }
    let entry = serde_json::json!({
        "record_id": record_id,
        "label": label,
        "at": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
    });
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&labels)?;
    writeln!(file, "{}", serde_json::to_string(&entry)?)?;
    println!("labeled {record_id} as {label}");
    Ok(())
}

fn stats_command(
    ledger: Option<PathBuf>,
    labels: Option<PathBuf>,
    json: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let ledger = ledger.unwrap_or_else(default_verdict_ledger);
    let labels = labels.unwrap_or_else(default_labels_ledger);
    let records = read_jsonl(&ledger)?;
    let mut latest: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for entry in read_jsonl(&labels)? {
        if let (Some(id), Some(label)) = (
            entry.get("record_id").and_then(Value::as_str),
            entry.get("label").and_then(Value::as_str),
        ) {
            latest.insert(id.to_owned(), label.to_owned());
        }
    }

    let total = records.len();
    let mut confirmed = 0_u64;
    let mut false_positive = 0_u64;
    let mut neutral = 0_u64;
    let mut labeled = 0_u64;
    let mut cost = 0.0_f64;
    for record in &records {
        if let Some(value) = record
            .get("verdict")
            .and_then(|verdict| verdict.get("cost_usd"))
            .and_then(Value::as_f64)
        {
            cost += value;
        }
        let Some(id) = record.get("record_id").and_then(Value::as_str) else {
            continue;
        };
        let Some(label) = latest.get(id) else {
            continue;
        };
        labeled += 1;
        match label.as_str() {
            "confirmed" => confirmed += 1,
            "false-positive" => false_positive += 1,
            "neutral" => neutral += 1,
            _ => {}
        }
    }
    let precision = (confirmed + false_positive > 0)
        .then(|| confirmed as f64 / (confirmed + false_positive) as f64);
    let false_positive_rate = (labeled > 0).then(|| false_positive as f64 / labeled as f64);
    let coverage = (total > 0).then(|| labeled as f64 / total as f64);
    let cost_per_confirmed = (confirmed > 0).then(|| cost / confirmed as f64);
    let report = serde_json::json!({
        "ledger": ledger,
        "total_verdicts": total,
        "labeled": labeled,
        "coverage": coverage,
        "confirmed": confirmed,
        "false_positive": false_positive,
        "neutral": neutral,
        "precision": precision,
        "false_positive_rate": false_positive_rate,
        "total_cost_usd": (cost * 1_000_000.0).round() / 1_000_000.0,
        "cost_per_confirmed_usd": cost_per_confirmed.map(|value| (value * 1_000_000.0).round() / 1_000_000.0),
        "catch_rate": Value::Null,
        "catch_rate_note": "requires a labeled defect benchmark",
    });
    if json {
        println!("{}", serde_json::to_string(&report)?);
    } else {
        println!("sideeye stats");
        println!("  ledger:             {}", ledger.display());
        println!("  verdicts:           {total}");
        println!("  labeled:            {labeled}");
        println!(
            "  coverage:           {}",
            coverage
                .map(|value| format!("{:.0}%", value * 100.0))
                .unwrap_or_else(|| "n/a".to_owned())
        );
        println!("  confirmed:          {confirmed}");
        println!("  false positive:     {false_positive}");
        println!("  neutral:            {neutral}");
        println!(
            "  precision:          {}",
            precision
                .map(|value| format!("{:.2}", value))
                .unwrap_or_else(|| "n/a".to_owned())
        );
        println!(
            "  false-positive rate:{}",
            false_positive_rate
                .map(|value| format!(" {:.0}%", value * 100.0))
                .unwrap_or_else(|| "n/a".to_owned())
        );
        println!("  total cost:         ${cost:.4}");
        println!(
            "  cost / confirmed:   {}",
            cost_per_confirmed
                .map(|value| format!("${value:.4}"))
                .unwrap_or_else(|| "n/a".to_owned())
        );
        println!("  catch rate:         n/a (needs a labeled defect benchmark)");
    }
    Ok(())
}

fn normalize_label(value: &str) -> Result<&'static str, Box<dyn std::error::Error>> {
    match value.trim().to_lowercase().as_str() {
        "confirmed" | "confirm" | "true-positive" | "true_positive" | "tp" => Ok("confirmed"),
        "false-positive" | "false_positive" | "fp" => Ok("false-positive"),
        "neutral" | "unsure" => Ok("neutral"),
        other => Err(
            format!("unknown label {other:?}; use confirmed, false-positive, or neutral").into(),
        ),
    }
}

fn read_jsonl(path: &Path) -> Result<Vec<Value>, Box<dyn std::error::Error>> {
    let Ok(text) = fs::read_to_string(path) else {
        return Ok(Vec::new());
    };
    Ok(text
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line.trim()).ok())
        .collect())
}

fn default_verdict_ledger() -> PathBuf {
    if let Ok(path) = env::var("SIDEEYE_VERDICTS") {
        return PathBuf::from(path);
    }
    state_home().join("opencode-verdicts.jsonl")
}

fn default_labels_ledger() -> PathBuf {
    if let Ok(path) = env::var("SIDEEYE_LABELS") {
        return PathBuf::from(path);
    }
    state_home().join("labels.jsonl")
}

fn state_home() -> PathBuf {
    env::var("HOME")
        .map(|home| PathBuf::from(home).join(".local/share/sideeye"))
        .unwrap_or_else(|_| PathBuf::from(".").join("sideeye"))
}

#[allow(clippy::too_many_arguments)]
fn estimate(
    path: PathBuf,
    provider_name: String,
    base_url: Option<String>,
    model: String,
    rubric: Option<PathBuf>,
    max_tokens: u32,
    instructions: Option<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let packet: ReviewPacket = serde_json::from_str(&read_input(path)?)?;
    let packet = packet.redacted();
    guard_self_grading(&packet, &model)?;
    let estimate = build_provider(
        &provider_name,
        base_url,
        model,
        rubric.as_ref(),
        max_tokens,
        instructions,
    )?
    .estimate(&packet)?;
    println!("{}", serde_json::to_string(&estimate)?);
    Ok(())
}

const EMBEDDED_RUBRIC: &str = include_str!("../../../rubric/rubric_session_v2.md");
const EMBEDDED_ADVICE_RUBRIC: &str = include_str!("../../../rubric/rubric_advice_v1.md");

fn build_provider(
    provider_name: &str,
    base_url: Option<String>,
    model: String,
    rubric: Option<&PathBuf>,
    max_tokens: u32,
    instructions: Option<String>,
) -> Result<Box<dyn JudgeProvider>, Box<dyn std::error::Error>> {
    let (base, key, rubric_text, rubric_version) =
        provider_material(base_url, rubric, EMBEDDED_RUBRIC, "rubric_session_v2")?;
    let instructions = instructions.unwrap_or_default();
    let timeout = env::var("SIDEEYE_JUDGE_TIMEOUT_SECS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok());
    match provider_name {
        "anthropic" => {
            let mut provider = sideeye_providers::AnthropicProvider::new(
                base,
                key,
                model,
                rubric_text,
                rubric_version,
            )
            .with_max_tokens(max_tokens)
            .with_instructions(instructions);
            if let Some(seconds) = timeout {
                provider = provider.with_timeout_secs(seconds);
            }
            Ok(Box::new(provider))
        }
        "openai" | "openai-compatible" => {
            let mut provider = sideeye_providers::OpenAiProvider::new(
                base,
                key,
                model,
                rubric_text,
                rubric_version,
            )
            .with_max_tokens(max_tokens)
            .with_instructions(instructions);
            if let Some(seconds) = timeout {
                provider = provider.with_timeout_secs(seconds);
            }
            Ok(Box::new(provider))
        }
        other => Err(format!(
            "provider {other:?} is not implemented; use anthropic or openai-compatible"
        )
        .into()),
    }
}

fn provider_material(
    base_url: Option<String>,
    rubric: Option<&PathBuf>,
    default_rubric: &str,
    default_version: &str,
) -> Result<(String, String, String, String), Box<dyn std::error::Error>> {
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
        None => (default_rubric.to_owned(), default_version.to_owned()),
    };
    Ok((base, key, rubric_text, rubric_version))
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

#[allow(clippy::too_many_arguments)]
fn packet_from_opencode(
    export_path: PathBuf,
    redact: bool,
    repo: Option<PathBuf>,
    diff_base: String,
    touched_files: Vec<String>,
    max_packet_chars: usize,
    scope: String,
) -> Result<(), Box<dyn std::error::Error>> {
    let export: serde_json::Value = serde_json::from_str(&read_input(export_path)?)?;
    let artifacts = match repo {
        Some(repo) => sideeye_capture::collect_diff_artifacts(&repo, &diff_base, &touched_files)?,
        None => Vec::new(),
    };
    let scope = sideeye_capture::ReviewScope::parse(&scope)
        .ok_or_else(|| format!("unknown scope {scope:?}; use session or last-exchange"))?;
    let packet = sideeye_capture::from_opencode_export_scoped(&export, artifacts, scope)?;
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

#[allow(clippy::too_many_arguments)]
fn setup(
    base_url: Option<String>,
    api_key: Option<String>,
    no_opencode: bool,
    no_skills: bool,
    repo: Option<PathBuf>,
    provider: String,
    model: String,
    max_tokens: Option<u32>,
    yes: bool,
    verify: bool,
    no_verify: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let interactive = !yes && io::stdin().is_terminal();
    let mut provider = normalize_provider(&provider)?;
    if interactive {
        let providers = ["Anthropic-compatible (Claude)", "OpenAI-compatible / local"];
        let default = usize::from(provider != "anthropic");
        let choice = Select::new()
            .with_prompt("Judge provider")
            .items(&providers)
            .default(default)
            .interact()?;
        provider = if choice == 0 {
            "anthropic"
        } else {
            "openai-compatible"
        };
    }
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

    println!("Side-Eye setup");
    println!("The judge is independent of the model that wrote the code.\n");

    let existing_provider = judge
        .get("provider")
        .and_then(serde_json::Value::as_str)
        .and_then(|value| normalize_provider(value).ok())
        .unwrap_or("anthropic");
    let provider_changed = existing_provider != provider;
    let existing_base = (!provider_changed)
        .then(|| {
            judge
                .get("base_url")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        })
        .flatten();
    // An explicit Side-Eye route or the saved route wins over ambient
    // ANTHROPIC_BASE_URL. Ambient values often point at the session's local
    // drafting proxy and must not silently replace the judge.
    let default_base = if provider == "anthropic" {
        AMBIENT_BASE_DEFAULT
    } else {
        "http://127.0.0.1:11434"
    };
    let mut selected_base = base_url
        .or_else(|| nonempty_env("SIDEEYE_JUDGE_BASE_URL"))
        .or(existing_base)
        .or_else(|| nonempty_env("ANTHROPIC_BASE_URL"))
        .unwrap_or_else(|| default_base.to_owned());
    if interactive {
        selected_base = prompt_input("Judge base URL", &selected_base)?;
    }
    selected_base = normalize_base_url(&selected_base);
    if selected_base.is_empty() {
        return Err("judge base URL cannot be empty".into());
    }
    judge["base_url"] = serde_json::json!(selected_base);

    let existing_key = (!provider_changed)
        .then(|| {
            judge
                .get("api_key")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        })
        .flatten();
    let mut key = api_key
        .or_else(|| nonempty_env("SIDEEYE_JUDGE_API_KEY"))
        .or(existing_key.clone());
    let keep_existing = match (&key, &existing_key, interactive) {
        (Some(_), Some(_), true) if key.as_deref() == existing_key.as_deref() => Confirm::new()
            .with_prompt("Keep the existing judge API key?")
            .default(true)
            .interact()?,
        _ => false,
    };
    if !keep_existing && (key.is_none() || key.as_deref() == existing_key.as_deref()) {
        if interactive {
            let prompt = if provider == "anthropic" {
                "Judge API key (hidden): "
            } else {
                "Judge API key (hidden; use 'local' if the server needs none): "
            };
            key = Some(rpassword::prompt_password(prompt)?);
        } else if key.is_none() {
            return Err(
                "no judge credential configured; set SIDEEYE_JUDGE_API_KEY or run sideeye setup in an interactive terminal".into(),
            );
        }
    }
    let key = key.unwrap_or_default();
    if key.trim().is_empty() {
        return Err("judge API key cannot be empty".into());
    }
    judge["api_key"] = serde_json::json!(key.trim());
    judge["provider"] = serde_json::json!(provider);
    let key = key.trim().to_owned();

    let mut model = model;
    let default_max_tokens = if provider == "anthropic" { 8192 } else { 16384 };
    let mut max_tokens = max_tokens.unwrap_or(default_max_tokens);
    let anthropic_default = "claude-opus-4-8";
    if interactive {
        if provider != "anthropic" && model == anthropic_default {
            model = prompt_required("Judge model (required for this provider)")?;
        } else {
            model = prompt_input("Judge model", &model)?;
        }
        max_tokens = prompt_u32("Output token ceiling", max_tokens)?;
    } else if provider != "anthropic" && model == anthropic_default {
        return Err(
            "--model is required when --provider is openai-compatible (for example qwen2.5-coder or gpt-4.1)"
                .into(),
        );
    }
    judge["model"] = serde_json::json!(model);
    judge["max_tokens"] = serde_json::json!(max_tokens);

    let harnesses = detect_harnesses();
    let detected = |name: &str| {
        harnesses
            .iter()
            .find(|(candidate, _)| *candidate == name)
            .map(|(_, detected)| *detected)
            .unwrap_or(false)
    };
    let opencode = detected("OpenCode");
    let claude = detected("Claude Code");
    let codex = detected("Codex");

    let mut repo = repo;
    if interactive && opencode {
        let suggestion = current_git_root()
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_default();
        let entered = Input::<String>::new()
            .with_prompt("Repository to review (blank = auto-detect per session)")
            .with_initial_text(suggestion)
            .allow_empty(true)
            .interact_text()?;
        let entered = entered.trim();
        if !entered.is_empty() {
            repo = Some(PathBuf::from(entered));
        }
    }

    if let Some(path) = &repo
        && !is_git_repository(path)
    {
        return Err(format!(
            "OpenCode review repository is not a git checkout: {}",
            path.display()
        )
        .into());
    }

    config["judge"] = judge;
    write_json_secure(&path, &config)?;

    println!();
    println!("Judge provider: {provider}");
    println!("Judge route: {}", selected_base);
    println!("Judge model: {model} (max {max_tokens} output tokens)");
    for (name, detected) in &harnesses {
        println!(
            "{name}: {}",
            if *detected {
                "detected"
            } else {
                "not detected"
            }
        );
    }
    println!(
        "Review repository: {}",
        repo.as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "auto-detect per session".to_owned())
    );

    if no_opencode || !opencode {
        println!("OpenCode plugin: skipped");
    } else {
        match setup_ext::install_opencode_plugin(repo.as_deref(), &model, max_tokens, provider) {
            Ok(path) => println!("OpenCode plugin: configured ({})", path.display()),
            Err(error) => return Err(error.into()),
        }
    }

    if !no_skills {
        match setup_ext::install_harness_skills(claude, codex) {
            Ok(paths) => {
                for path in paths {
                    println!("Skill: configured ({})", path.display());
                }
            }
            Err(error) => return Err(error.into()),
        }
    }

    let should_verify = verify || (interactive && !no_verify);
    if should_verify {
        match probe_route(provider, &selected_base, &key, &model) {
            Ok(detail) => println!("Free judge check: OK ({detail}; no review spend)"),
            Err(error) => {
                println!("Free judge check: failed: {error}");
                println!("Config saved. Fix the route and run `sideeye doctor --verify`.");
                if verify {
                    return Err(error);
                }
            }
        }
    }

    println!();
    println!("Next:");
    println!("  1. Restart OpenCode (plugins load at startup).");
    println!("  2. In a session, run /sideeye.");
    println!(
        "  3. Choose Last exchange or Whole session, add optional instructions, confirm the estimate."
    );
    println!("Run `sideeye doctor` to check the route without spending.");
    Ok(())
}

fn prompt_input(prompt: &str, default: &str) -> Result<String, Box<dyn std::error::Error>> {
    let value = Input::<String>::new()
        .with_prompt(prompt)
        .with_initial_text(default)
        .interact_text()?;
    let value = value.trim().to_owned();
    Ok(if value.is_empty() {
        default.to_owned()
    } else {
        value
    })
}

fn prompt_required(prompt: &str) -> Result<String, Box<dyn std::error::Error>> {
    loop {
        let value = Input::<String>::new().with_prompt(prompt).interact_text()?;
        let value = value.trim();
        if !value.is_empty() {
            return Ok(value.to_owned());
        }
        println!("A value is required.");
    }
}

fn prompt_u32(prompt: &str, default: u32) -> Result<u32, Box<dyn std::error::Error>> {
    let value = prompt_input(prompt, &default.to_string())?;
    value
        .parse::<u32>()
        .map_err(|_| format!("{prompt} must be a positive integer").into())
}

fn current_git_root() -> Option<PathBuf> {
    let output = std::process::Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let path = String::from_utf8(output.stdout).ok()?.trim().to_owned();
    (!path.is_empty()).then(|| PathBuf::from(path))
}

fn is_git_repository(path: &Path) -> bool {
    std::process::Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn probe_route(
    provider_name: &str,
    base_url: &str,
    api_key: &str,
    model: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    match provider_name {
        "anthropic" => {
            let provider = sideeye_providers::AnthropicProvider::new(
                base_url,
                api_key,
                model,
                EMBEDDED_RUBRIC,
                "rubric_session_v2",
            );
            Ok(format!("{} input tokens", provider.probe()?))
        }
        "openai" | "openai-compatible" => {
            let provider = sideeye_providers::OpenAiProvider::new(
                base_url,
                api_key,
                model,
                EMBEDDED_RUBRIC,
                "rubric_session_v2",
            );
            Ok(provider.probe()?)
        }
        other => Err(format!("unknown provider {other:?}").into()),
    }
}

fn normalize_provider(value: &str) -> Result<&'static str, Box<dyn std::error::Error>> {
    match value.trim().to_lowercase().as_str() {
        "anthropic" => Ok("anthropic"),
        "openai" | "openai-compatible" => Ok("openai-compatible"),
        other => {
            Err(format!("unknown provider {other:?}; use anthropic or openai-compatible").into())
        }
    }
}

fn doctor(
    base_url: Option<String>,
    verify: bool,
    model: String,
    provider: Option<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let config = load_judge_config();
    let provider = provider
        .or(config.provider.clone())
        .unwrap_or_else(|| "anthropic".to_owned());
    let provider = normalize_provider(&provider)?;
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
    if verify {
        let base = route
            .base
            .as_deref()
            .ok_or("no judge base URL configured")?;
        let key = route
            .key
            .as_deref()
            .ok_or("no judge credential configured")?;
        let detail = probe_route(provider, base, key, &model)?;
        println!("  free check:    OK ({detail}; no review spend)");
    }
    println!("  verdict:       ready");
    Ok(())
}

#[derive(Debug, Default, PartialEq, Eq)]
struct JudgeConfig {
    provider: Option<String>,
    base_url: Option<String>,
    api_key: Option<String>,
    model: Option<String>,
    max_tokens: Option<u32>,
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
        provider: judge
            .get("provider")
            .and_then(Value::as_str)
            .map(str::to_owned),
        model: judge
            .get("model")
            .and_then(Value::as_str)
            .map(str::to_owned),
        max_tokens: judge
            .get("max_tokens")
            .and_then(Value::as_u64)
            .and_then(|value| u32::try_from(value).ok()),
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
            provider: None,
            model: None,
            max_tokens: None,
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
