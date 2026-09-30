#![forbid(unsafe_code)]
use anyhow::{Context, Result, ensure};
use clap::{Args, Parser, Subcommand};
use evalproof::{contract, license, model::*, report, runner, storage};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Parser)]
#[command(version, about = "Find the incorrect outputs your AI graders accept")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Import frozen outputs and suggest contracts for review. Never calls a model.
    Init(Init),
    /// Free diagnostic on developer machines. Running in CI requires a subscription.
    Diagnose(RunArgs),
    /// Paid CI gate: 0=PASS, 1=FAIL, 2=INCONCLUSIVE or ERROR.
    Check(RunArgs),
    /// Render previously saved evidence without executing a grader.
    Report {
        #[arg(long, default_value = ".evalproof/latest.json")]
        input: PathBuf,
        #[arg(long, default_value = ".evalproof/report.html")]
        output: PathBuf,
    },
    /// Validate configuration, runtime, coverage and licensing without grading.
    Doctor {
        #[arg(long, default_value = "evalproof.json")]
        suite: PathBuf,
    },
    /// Refresh or inspect the seven-day offline CI entitlement, or open billing.
    License {
        #[arg(value_parser=["refresh","status","buy","portal"],default_value="status")]
        action: String,
        #[arg(long, default_value = ".evalproof/entitlement.lease")]
        path: PathBuf,
    },
}
#[derive(Args)]
struct RunArgs {
    #[arg(long, default_value = "evalproof.json")]
    suite: PathBuf,
    #[arg(long)]
    state: Option<PathBuf>,
    /// Stable ID for CI retries. Raising budgets resumes the same observations.
    #[arg(long)]
    run_id: Option<String>,
    /// Refuse to restart if a CI retry has lost its saved evidence.
    #[arg(long, requires = "run_id")]
    require_resume: bool,
    #[arg(long)]
    baseline: Option<PathBuf>,
    /// Reproduce one deterministic case. Only available for diagnose.
    #[arg(long)]
    case: Option<String>,
    #[arg(long)]
    budget_microusd: Option<u64>,
    #[arg(long)]
    time_limit_seconds: Option<u64>,
    #[arg(long)]
    json: bool,
}
#[derive(Args)]
struct Init {
    #[arg(long, default_value = "evalproof.json")]
    output: PathBuf,
    /// JSON array of {id, output, context} records. Outputs must already be frozen.
    #[arg(long)]
    fixtures: Option<PathBuf>,
    #[arg(long, conflicts_with = "promptfoo")]
    python: Option<String>,
    /// Existing Promptfoo YAML/JSON config. Context must include test_index.
    #[arg(long)]
    promptfoo: Option<PathBuf>,
    /// Create a self-contained invoice example with a deliberately weak grader.
    #[arg(long)]
    demo: bool,
}

fn create_new(path: &Path, data: &[u8]) -> Result<()> {
    use std::io::Write;
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .with_context(|| format!("refusing to overwrite {}", path.display()))?;
    f.write_all(data)?;
    Ok(())
}

fn init(args: Init) -> Result<()> {
    ensure!(
        !args.output.exists(),
        "configuration already exists; init never overwrites it"
    );
    let root = args
        .output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(root)?;
    let detected = if args.promptfoo.is_none() && args.python.is_none() && !args.demo {
        [
            "promptfooconfig.yaml",
            "promptfooconfig.yml",
            "promptfooconfig.json",
        ]
        .into_iter()
        .map(|p| root.join(p))
        .find(|p| p.is_file())
    } else {
        None
    };
    let promptfoo = args.promptfoo.or(detected);
    ensure!(
        args.demo || args.fixtures.is_some(),
        "provide --fixtures frozen-outputs.json, or use --demo for a runnable example"
    );
    ensure!(
        args.demo || promptfoo.is_some() || args.python.is_some(),
        "provide --python grader.py:function or --promptfoo promptfooconfig.yaml"
    );
    let raw: Vec<Value> = match &args.fixtures {
        Some(p) => storage::read_json(p)?,
        None => vec![
            json!({"id":"invoice-1","output":{"invoice_id":"INV-42","total":125.00},"context":{}}),
        ],
    };
    let mut fixtures = Vec::new();
    for (i, record) in raw.into_iter().enumerate() {
        let output = record
            .get("output")
            .context("every fixture requires an output field")?
            .clone();
        // Preserve objects exactly; text outputs must contain valid structured JSON.
        let output = if let Value::String(s) = &output {
            serde_json::from_str(s).context(
                "text output is not structured JSON; wrap literal strings in a JSON object",
            )?
        } else {
            output
        };
        let context = record.get("context").cloned().unwrap_or(json!({}));
        if promptfoo.is_some() {
            ensure!(
                context.get("test_index").and_then(Value::as_u64).is_some(),
                "Promptfoo fixture context requires test_index matching its config test"
            );
        }
        fixtures.push(Fixture {
            id: record
                .get("id")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .unwrap_or(format!("fixture-{}", i + 1)),
            rules: contract::infer_rules(&output),
            output,
            context,
            semantic_json: false,
        });
    }
    let (command, dependencies) = if let Some(config) = promptfoo {
        let adapter = root.join("evalproof-promptfoo.mjs");
        create_new(&adapter, include_bytes!("../packages/node/adapter.mjs"))?;
        let config = fs::canonicalize(config)?;
        (
            vec![
                "node".into(),
                "evalproof-promptfoo.mjs".into(),
                config.to_string_lossy().into(),
            ],
            vec![
                "evalproof-promptfoo.mjs".into(),
                config.to_string_lossy().into(),
                "package-lock.json".into(),
            ],
        )
    } else {
        create_new(
            &root.join("evalproof-python.py"),
            include_bytes!("../packages/python/evalproof/adapter.py"),
        )?;
        let grader = if args.demo {
            create_new(
                &root.join("grader.py"),
                include_bytes!("../examples/python/grader.py"),
            )?;
            "grader.py:weak".to_owned()
        } else {
            args.python.context("missing Python grader")?
        };
        let (path, _) = grader
            .rsplit_once(':')
            .context("use --python grader.py:function")?;
        (
            vec![
                std::env::var("EVALPROOF_PYTHON").unwrap_or_else(|_| {
                    if cfg!(windows) {
                        "python".into()
                    } else {
                        "python3".into()
                    }
                }),
                "evalproof-python.py".into(),
                grader.clone(),
            ],
            vec!["evalproof-python.py".into(), path.into()],
        )
    };
    let suite = Suite {
        version: 1,
        name: "Evaluator contract checks".into(),
        confirmed: args.demo,
        adapter: Adapter {
            command,
            mode: Mode::Deterministic,
            dependencies,
            environment: vec![],
            cache_safe: false,
            max_cost_microusd: None,
            fresh_calls: false,
            identity: "customer-grader-v1".into(),
        },
        fixtures,
        policy: Policy::default(),
    };
    contract::validate(&suite)?;
    create_new(&args.output, &serde_json::to_vec_pretty(&suite)?)?;
    let ignore = root.join(".gitignore");
    let existing = fs::read_to_string(&ignore).unwrap_or_default();
    if !existing.lines().any(|l| l == ".evalproof/") {
        use std::io::Write;
        let mut f = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(ignore)?;
        f.write_all(b"\n.evalproof/\n")?;
    }
    println!(
        "Created {}. {}",
        args.output.display(),
        if args.demo {
            "Run evalproof diagnose to expose the deliberately weak grader."
        } else {
            "Review rules and grader mode, then set confirmed to true. Inferred exact values are suggestions."
        }
    );
    Ok(())
}

/// True when running under a CI system. Free diagnostics are for developer
/// machines; automated gating is the paid product.
fn ci_environment() -> bool {
    let set = |name: &str| {
        std::env::var(name)
            .is_ok_and(|v| !v.is_empty() && !v.eq_ignore_ascii_case("false") && v != "0")
    };
    [
        "CI",
        "GITHUB_ACTIONS",
        "GITLAB_CI",
        "BUILDKITE",
        "CIRCLECI",
        "JENKINS_URL",
        "TF_BUILD",
    ]
    .into_iter()
    .any(set)
}

async fn ensure_entitlement(state: &Path) -> Result<license::Entitlement> {
    let lease_path = state.join("entitlement.lease");
    match license::check(&lease_path) {
        Ok(lease) => {
            if storage::now()?.saturating_sub(lease.issued_at) >= 86_400
                && std::env::var_os("EVALPROOF_LICENSE_KEY").is_some()
                && license::refresh(&lease_path).await.is_err()
            {
                eprintln!("Entitlement refresh unavailable; using the valid offline entitlement.");
            }
            Ok(lease)
        }
        Err(error) => license::refresh(&lease_path).await.with_context(|| {
            format!(
                "CI use requires an EvalProof Team subscription (14-day free trial): {}\n({error:#})\nExisting reports are preserved; diagnose stays free on developer machines.",
                license::checkout_url()
            )
        }),
    }
}

fn open_in_browser(url: &str) {
    use std::io::IsTerminal;
    if !std::io::stdout().is_terminal() || ci_environment() {
        return;
    }
    let opener = if cfg!(target_os = "macos") {
        vec!["open", url]
    } else if cfg!(windows) {
        vec!["cmd", "/C", "start", "", url]
    } else {
        vec!["xdg-open", url]
    };
    // Printing the URL is the primary path; opening a browser is a convenience.
    let _ = std::process::Command::new(opener[0])
        .args(&opener[1..])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

async fn run(args: RunArgs, paid: bool) -> Result<i32> {
    ensure!(
        !paid || args.case.is_none(),
        "--case is available only for diagnose; a subset cannot stand in for a suite gate"
    );
    let suite_path = fs::canonicalize(&args.suite).context("suite not found; run init first")?;
    let root = suite_path
        .parent()
        .context("suite has no parent directory")?;
    let mut suite: Suite = storage::read_json(&suite_path)?;
    if let Some(v) = args.budget_microusd {
        suite.policy.budget_microusd = v;
    }
    if let Some(v) = args.time_limit_seconds {
        suite.policy.time_limit_seconds = v;
    }
    let state = args.state.unwrap_or_else(|| root.join(".evalproof"));
    if args.require_resume {
        let id = args
            .run_id
            .as_deref()
            .context("--require-resume needs --run-id")?;
        storage::validate_run_id(id)?;
        ensure!(
            state.join("runs").join(format!("{id}.json")).is_file(),
            "Saved evidence for this retry is missing. Restore the previous run; restarting would discard observations."
        );
    }
    if paid || ci_environment() {
        ensure_entitlement(&state).await?;
    }
    let options = runner::Options {
        state: state.clone(),
        run_id: args.run_id,
        baseline: args.baseline,
        case_id: args.case,
    };
    let run = runner::execute(&suite, root, &options).await?;
    storage::write_atomic(&state.join("report.html"), report::html(&run)?.as_bytes())?;
    storage::write_atomic(&state.join("summary.md"), report::markdown(&run).as_bytes())?;
    if paid && let Some(path) = std::env::var_os("GITHUB_STEP_SUMMARY") {
        use std::io::Write;
        fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?
            .write_all(report::markdown(&run).as_bytes())?;
    }
    if args.json {
        println!("{}", serde_json::to_string(&run)?);
    } else {
        print!("{}", report::summary(&run));
        println!("Report: {}", state.join("report.html").display());
        if !paid && run.status == Status::Fail {
            println!(
                "Block these in CI before they ship: evalproof license buy ({})",
                license::checkout_url()
            );
        }
    }
    Ok(if paid {
        run.status.exit_code()
    } else {
        match run.status {
            Status::Pass | Status::Fail => 0,
            _ => 2,
        }
    })
}

async fn dispatch(cli: Cli) -> Result<i32> {
    match cli.command {
        Command::Init(args) => init(args)?,
        Command::Diagnose(args) => return run(args, false).await,
        Command::Check(args) => return run(args, true).await,
        Command::Report { input, output } => {
            let run: Run = storage::read_json(&input)?;
            ensure!(run.version == 1, "unsupported report version");
            storage::write_atomic(&output, report::html(&run)?.as_bytes())?;
            println!("Report: {}", output.display());
        }
        Command::Doctor { suite } => {
            let path = fs::canonicalize(suite)?;
            let root = path.parent().context("missing suite parent")?;
            let suite: Suite = storage::read_json(&path)?;
            contract::validate(&suite)?;
            let fingerprint = storage::fingerprint(&suite, root)?;
            let (cases, unsupported) = contract::generate(&suite)?;
            let worker = evalproof::worker::Worker::spawn(
                &suite.adapter,
                root,
                std::time::Duration::from_secs(15),
            )
            .await?;
            worker.shutdown().await?;
            println!(
                "Adapter: ready\nContract review: {}\nGenerated cases: {}\nUnsupported rules: {}\nCache reuse: {}\nFingerprint: {}",
                suite.confirmed,
                cases.len(),
                unsupported.len(),
                suite.adapter.cache_safe,
                fingerprint
            );
            if suite.adapter.mode == Mode::Llm {
                println!(
                    "Fresh calls declared: {}\nMaximum callback cost declared: {}\nLLM identity declared: {}",
                    suite.adapter.fresh_calls,
                    suite.adapter.max_cost_microusd.is_some(),
                    !suite.adapter.identity.is_empty()
                );
            }
            match license::check(&root.join(".evalproof/entitlement.lease")) {
                Ok(lease) => println!("Paid entitlement: valid for {}", lease.org),
                Err(error) => println!(
                    "Paid entitlement: unavailable ({error:#}). Free diagnostics work on developer machines; CI needs a subscription: {}",
                    license::checkout_url()
                ),
            }
            for message in &unsupported {
                println!("Coverage: {message}");
            }
            if !suite.confirmed
                || !unsupported.is_empty()
                || (suite.adapter.mode == Mode::Llm
                    && (!suite.adapter.fresh_calls
                        || suite.adapter.max_cost_microusd.is_none()
                        || suite.adapter.identity.is_empty()))
            {
                return Ok(2);
            }
        }
        Command::License { action, path } => {
            let lease = match action.as_str() {
                "buy" | "portal" => {
                    let url = if action == "buy" {
                        license::checkout_url()
                    } else {
                        license::portal_url()
                    };
                    println!("{url}");
                    open_in_browser(url);
                    return Ok(0);
                }
                "refresh" => license::refresh(&path).await?,
                _ => license::check(&path).with_context(|| {
                    format!(
                        "no valid CI entitlement; start a free trial with: evalproof license buy ({})",
                        license::checkout_url()
                    )
                })?,
            };
            println!(
                "CI entitlement for {} valid until Unix timestamp {}",
                lease.org, lease.expires_at
            );
        }
    }
    Ok(0)
}

#[tokio::main]
async fn main() {
    let code = match dispatch(Cli::parse()).await {
        Ok(code) => code,
        Err(error) => {
            eprintln!("ERROR: {error:#}");
            2
        }
    };
    if std::env::var_os("EVALPROOF_PROFILE").is_some() {
        eprintln!("PROFILE {}", storage::profile());
    }
    std::process::exit(code)
}
