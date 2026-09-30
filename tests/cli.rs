use anyhow::Result;
use evalproof::model::{Run, Status};
use serde_json::{Value, json};
use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

const CI_VARIABLES: [&str; 7] = [
    "CI",
    "GITHUB_ACTIONS",
    "GITLAB_CI",
    "BUILDKITE",
    "CIRCLECI",
    "JENKINS_URL",
    "TF_BUILD",
];
fn command(root: &Path, args: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_evalproof"));
    command.current_dir(root).args(args);
    // Tests exercise developer-machine behavior even when the suite itself runs in CI.
    for name in CI_VARIABLES
        .into_iter()
        .chain(["EVALPROOF_ENTITLEMENT", "EVALPROOF_LICENSE_KEY"])
    {
        command.env_remove(name);
    }
    command
}
fn cli(root: &Path, args: &[&str]) -> Result<Output> {
    Ok(command(root, args).output()?)
}

#[test]
fn diagnose_in_ci_requires_a_subscription() -> Result<()> {
    let root = setup()?;
    let output = command(root.path(), &["diagnose"])
        .env("CI", "true")
        .output()?;
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("subscription"), "{stderr}");
    assert!(!root.path().join(".evalproof/latest.json").exists());
    let output = command(root.path(), &["diagnose"])
        .env("CI", "false")
        .output()?;
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("evalproof license buy"));
    Ok(())
}

#[test]
fn license_buy_prints_the_checkout_link() -> Result<()> {
    let root = tempfile::tempdir()?;
    let output = cli(root.path(), &["license", "buy"])?;
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).starts_with("https://"));
    Ok(())
}
fn setup() -> Result<tempfile::TempDir> {
    let root = tempfile::tempdir()?;
    let out = cli(root.path(), &["init", "--demo"])?;
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    Ok(root)
}
fn configure(root: &Path, edit: impl FnOnce(&mut Value)) -> Result<()> {
    let path = root.join("evalproof.json");
    let mut v: Value = serde_json::from_slice(&fs::read(&path)?)?;
    edit(&mut v);
    fs::write(path, serde_json::to_vec_pretty(&v)?)?;
    Ok(())
}
fn latest(root: &Path) -> Result<Run> {
    Ok(serde_json::from_slice(&fs::read(
        root.join(".evalproof/latest.json"),
    )?)?)
}

#[test]
fn weak_grader_is_exposed_and_fixed_grader_passes() -> Result<()> {
    let root = setup()?;
    let output = cli(root.path(), &["diagnose"])?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(latest(root.path())?.status, Status::Fail);
    configure(root.path(), |v| {
        v["adapter"]["command"][2] = json!("grader.py:strict")
    })?;
    let output = cli(root.path(), &["diagnose"])?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(latest(root.path())?.status, Status::Pass);
    Ok(())
}

#[test]
fn grader_exceptions_are_errors_not_caught_mutations() -> Result<()> {
    let root = setup()?;
    fs::write(
        root.path().join("grader.py"),
        "def weak(output, context):\n    raise RuntimeError('secret payload')\n",
    )?;
    let output = cli(root.path(), &["diagnose"])?;
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(latest(root.path())?.status, Status::Error);
    assert!(!String::from_utf8_lossy(&output.stderr).contains("secret payload"));
    Ok(())
}

#[test]
fn unconfirmed_contracts_cannot_pass() -> Result<()> {
    let root = setup()?;
    configure(root.path(), |v| v["confirmed"] = json!(false))?;
    assert_eq!(cli(root.path(), &["diagnose"])?.status.code(), Some(2));
    let r = latest(root.path())?;
    assert!(r.observations.is_empty());
    assert_eq!(r.status, Status::Error);
    Ok(())
}

#[test]
fn llm_budget_resume_preserves_observations_and_spend() -> Result<()> {
    let root = setup()?;
    configure(root.path(), |v| {
        v["adapter"]["command"][2] = json!("grader.py:strict");
        v["adapter"]["mode"] = json!("llm");
        v["adapter"]["fresh_calls"] = json!(true);
        v["adapter"]["max_cost_microusd"] = json!(10_000);
        v["policy"]["budget_microusd"] = json!(80_000);
    })?;
    let output = cli(root.path(), &["diagnose", "--run-id", "resume-test"])?;
    assert_eq!(
        output.status.code(),
        Some(2),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let first = latest(root.path())?;
    assert_eq!(first.status, Status::Inconclusive);
    assert_eq!(first.reserved_microusd, 80_000);
    let output = cli(
        root.path(),
        &[
            "diagnose",
            "--run-id",
            "resume-test",
            "--budget-microusd",
            "5000000",
        ],
    )?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let second = latest(root.path())?;
    assert_eq!(second.status, Status::Pass);
    assert_eq!(second.observations.len(), 320);
    assert_eq!(second.reserved_microusd, 3_200_000);
    assert_eq!(
        serde_json::to_value(&first.observations)?,
        serde_json::to_value(&second.observations[..first.observations.len()])?
    );
    cli(
        root.path(),
        &[
            "diagnose",
            "--run-id",
            "resume-test",
            "--budget-microusd",
            "5000000",
        ],
    )?;
    let third = latest(root.path())?;
    assert!(third.reused);
    assert_eq!(third.observations.len(), 320);
    Ok(())
}

#[test]
fn model_grader_that_accepts_everything_fails_statistically() -> Result<()> {
    let root = setup()?;
    fs::write(
        root.path().join("grader.py"),
        "def weak(output, context):\n    return True\n",
    )?;
    configure(root.path(), |v| {
        v["adapter"]["mode"] = json!("llm");
        v["adapter"]["fresh_calls"] = json!(true);
        v["adapter"]["max_cost_microusd"] = json!(0);
    })?;
    assert!(cli(root.path(), &["diagnose"])?.status.success());
    let r = latest(root.path())?;
    assert_eq!(r.status, Status::Fail);
    assert!(r.metrics.iter().any(|m| m.lower > 0.05));
    Ok(())
}

#[test]
fn single_case_is_not_a_paid_suite_gate() -> Result<()> {
    let root = setup()?;
    let output = cli(root.path(), &["check", "--case", "abc"])?;
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("subset"));
    Ok(())
}

#[test]
fn report_escapes_customer_payload_and_has_no_remote_assets() -> Result<()> {
    let root = setup()?;
    configure(root.path(), |v| {
        v["name"] = json!("<script>alert(1)</script>");
        v["fixtures"][0]["output"]["invoice_id"] = json!("<img src=x onerror=alert(1)>");
        for r in v["fixtures"][0]["rules"]
            .as_array_mut()
            .expect("test array")
        {
            if r["id"] == "exact:/invoice_id" {
                r["value"] = json!("<img src=x onerror=alert(1)>");
            }
        }
    })?;
    cli(root.path(), &["diagnose"])?;
    let html = fs::read_to_string(root.path().join(".evalproof/report.html"))?;
    assert!(!html.contains("<script>"));
    assert!(!html.contains("<img"));
    assert!(html.contains("&lt;script&gt;"));
    assert!(html.contains("default-src 'none'"));
    Ok(())
}

#[test]
fn dependency_changes_invalidate_deterministic_cache() -> Result<()> {
    let root = setup()?;
    configure(root.path(), |v| v["adapter"]["cache_safe"] = json!(true))?;
    cli(root.path(), &["diagnose"])?;
    let first = latest(root.path())?;
    cli(root.path(), &["diagnose"])?;
    assert!(latest(root.path())?.reused);
    fs::write(
        root.path().join("grader.py"),
        "def weak(output, context):\n    return output == {'invoice_id':'INV-42','total':125}\n",
    )?;
    cli(root.path(), &["diagnose"])?;
    let changed = latest(root.path())?;
    assert_ne!(changed.fingerprint, first.fingerprint);
    assert!(!changed.reused);
    assert_eq!(changed.status, Status::Pass);
    Ok(())
}

#[test]
fn crashed_reservation_is_not_retried() -> Result<()> {
    let root = setup()?;
    configure(root.path(), |v| {
        v["adapter"]["mode"] = json!("llm");
        v["adapter"]["fresh_calls"] = json!(true);
        v["adapter"]["max_cost_microusd"] = json!(10_000);
        v["policy"]["budget_microusd"] = json!(80_000);
    })?;
    cli(root.path(), &["diagnose", "--run-id", "crash"])?;
    let path = root.path().join(".evalproof/runs/crash.json");
    let mut r: Run = evalproof::storage::read_json(&path)?;
    r.pending = 2;
    evalproof::storage::write_json(&path, &r)?;
    cli(
        root.path(),
        &[
            "diagnose",
            "--run-id",
            "crash",
            "--budget-microusd",
            "5000000",
        ],
    )?;
    let resumed = latest(root.path())?;
    assert_eq!(resumed.status, Status::Error);
    assert_eq!(resumed.observations.len(), r.observations.len());
    Ok(())
}

#[test]
fn subset_diagnostic_cannot_poison_full_suite_cache() -> Result<()> {
    let root = setup()?;
    configure(root.path(), |v| v["adapter"]["cache_safe"] = json!(true))?;
    cli(root.path(), &["diagnose"])?;
    let full = latest(root.path())?;
    let case = full
        .cases
        .iter()
        .find(|c| c.operator == "original")
        .expect("original control");
    cli(root.path(), &["diagnose", "--case", &case.id])?;
    let subset = latest(root.path())?;
    assert_eq!(subset.status, Status::Pass);
    assert_ne!(full.fingerprint, subset.fingerprint);
    cli(root.path(), &["diagnose"])?;
    let again = latest(root.path())?;
    assert_eq!(again.status, Status::Fail);
    assert_eq!(again.cases.len(), full.cases.len());
    Ok(())
}

#[test]
fn tampered_cached_pass_is_rejected() -> Result<()> {
    let root = setup()?;
    configure(root.path(), |v| v["adapter"]["cache_safe"] = json!(true))?;
    cli(root.path(), &["diagnose"])?;
    let mut run = latest(root.path())?;
    run.status = Status::Pass;
    evalproof::storage::write_json(&root.path().join(".evalproof/latest.json"), &run)?;
    let result = cli(root.path(), &["diagnose"])?;
    assert_eq!(result.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&result.stderr).contains("not supported"));
    Ok(())
}

#[test]
fn ci_retry_requires_saved_evidence() -> Result<()> {
    let root = setup()?;
    let result = cli(
        root.path(),
        &["diagnose", "--run-id", "missing", "--require-resume"],
    )?;
    assert_eq!(result.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&result.stderr).contains("missing"));
    Ok(())
}

#[test]
fn timeout_cannot_count_as_rejection() -> Result<()> {
    let root = setup()?;
    fs::write(
        root.path().join("grader.py"),
        "import time\ndef weak(output, context):\n    time.sleep(3)\n    return False\n",
    )?;
    configure(root.path(), |v| {
        v["policy"]["call_timeout_seconds"] = json!(1)
    })?;
    assert_eq!(cli(root.path(), &["diagnose"])?.status.code(), Some(2));
    assert_eq!(latest(root.path())?.status, Status::Error);
    Ok(())
}

#[test]
fn critical_metrics_receive_balanced_samples_when_workers_are_fewer() -> Result<()> {
    let root = setup()?;
    configure(root.path(), |v| {
        v["adapter"]["command"][2] = json!("grader.py:strict");
        v["adapter"]["mode"] = json!("llm");
        v["adapter"]["fresh_calls"] = json!(true);
        v["adapter"]["max_cost_microusd"] = json!(1000);
        v["policy"]["budget_microusd"] = json!(28000);
        for r in v["fixtures"][0]["rules"].as_array_mut().expect("rules") {
            r["critical"] = json!(true);
        }
        let mut second = v["fixtures"][0].clone();
        second["id"] = json!("invoice-2");
        v["fixtures"].as_array_mut().expect("fixtures").push(second);
    })?;
    assert_eq!(cli(root.path(), &["diagnose"])?.status.code(), Some(2));
    let run = latest(root.path())?;
    assert_eq!(run.metrics.len(), 14);
    assert!(run.metrics.iter().all(|m| m.samples == 2));
    Ok(())
}
