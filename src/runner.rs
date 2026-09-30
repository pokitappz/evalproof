use crate::{VERSION, contract, model::*, stats, storage, worker::Worker};
use anyhow::{Context, Result, ensure};
use futures::future::join_all;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

pub struct Options {
    pub state: PathBuf,
    pub run_id: Option<String>,
    pub baseline: Option<PathBuf>,
    pub case_id: Option<String>,
}

fn metric(id: String, population: Vec<usize>) -> Metric {
    Metric {
        id,
        population,
        samples: 0,
        errors: 0,
        lower: 0.0,
        upper: 1.0,
        decision: Status::Inconclusive,
    }
}
fn metrics(cases: &[Case]) -> Vec<Metric> {
    let mut m = vec![
        metric(
            "missed_errors".into(),
            cases
                .iter()
                .enumerate()
                .filter(|(_, c)| c.expected == Verdict::Reject)
                .map(|(i, _)| i)
                .collect(),
        ),
        metric(
            "false_rejections".into(),
            cases
                .iter()
                .enumerate()
                .filter(|(_, c)| c.expected == Verdict::Accept)
                .map(|(i, _)| i)
                .collect(),
        ),
    ];
    m.extend(
        cases
            .iter()
            .enumerate()
            .filter(|(_, c)| c.critical)
            .map(|(i, c)| metric(format!("critical:{}", c.id), vec![i])),
    );
    m
}

fn findings(run: &Run) -> BTreeSet<String> {
    let expected: std::collections::BTreeMap<_, _> =
        run.cases.iter().map(|c| (&c.id, c.expected)).collect();
    run.observations
        .iter()
        .filter(|o| {
            o.grade.verdict != Verdict::Error
                && expected
                    .get(&o.case_id)
                    .is_some_and(|v| *v != o.grade.verdict)
        })
        .map(|o| o.case_id.clone())
        .collect()
}

fn verify_history(saved: &Run, expected: &Run) -> Result<()> {
    ensure!(
        saved.version == expected.version
            && saved.engine == expected.engine
            && saved.mode == expected.mode,
        "saved evidence has incompatible provenance"
    );
    ensure!(
        serde_json::to_value(&saved.cases)? == serde_json::to_value(&expected.cases)?,
        "saved case population differs from the current contract"
    );
    ensure!(
        saved.created_at <= storage::now()?,
        "saved evidence has a future timestamp"
    );
    ensure!(
        saved.policy.tolerance == expected.policy.tolerance
            && saved.policy.confidence == expected.policy.confidence,
        "saved evidence has a different statistical policy"
    );
    if saved.status == Status::Error || saved.pending > 0 {
        return Ok(());
    }
    let mut rebuilt = metrics(&expected.cases);
    let groups = rebuilt.len();
    for (i, observation) in saved.observations.iter().enumerate() {
        ensure!(
            observation.sequence == i,
            "saved observation sequence is corrupt"
        );
        ensure!(
            observation.grade.verdict != Verdict::Error,
            "saved execution error cannot be reused as evidence"
        );
        if saved.mode == Mode::Deterministic {
            let case = expected
                .cases
                .get(i)
                .context("too many saved deterministic observations")?;
            ensure!(
                case.id == observation.case_id,
                "saved deterministic case order differs"
            );
        } else {
            let m = rebuilt
                .iter_mut()
                .find(|m| m.id == observation.metric)
                .context("unknown saved metric")?;
            ensure!(
                m.samples < 640 && m.decision == Status::Inconclusive,
                "saved sampling continued beyond its decision"
            );
            let selected = storage::sample(&saved.seed, &m.id, m.samples, m.population.len())?;
            let case = &expected.cases[m.population[selected]];
            ensure!(
                case.id == observation.case_id,
                "saved sample does not match the frozen sampling plan"
            );
            m.samples += 1;
            if case.expected != observation.grade.verdict {
                m.errors += 1;
            }
            stats::decide(m, &expected.policy, groups)?;
        }
    }
    if saved.mode == Mode::Llm {
        ensure!(
            serde_json::to_value(&rebuilt)? == serde_json::to_value(&saved.metrics)?,
            "saved statistical counters or decisions are corrupt"
        );
    }
    if saved.status == Status::Pass {
        ensure!(
            saved.complete && expected.unsupported.is_empty() && saved.unsupported.is_empty(),
            "incomplete coverage cannot be cached as PASS"
        );
        ensure!(
            if saved.mode == Mode::Llm {
                rebuilt.iter().all(|m| m.decision == Status::Pass)
            } else {
                saved.observations.len() == expected.cases.len() && findings(saved).is_empty()
            },
            "saved PASS is not supported by its observations"
        );
    }
    Ok(())
}
fn compare(run: &mut Run, baseline: &Path) -> Result<()> {
    let old: Run = storage::read_json(baseline)?;
    ensure!(old.version == 1, "unsupported baseline version");
    let before = findings(&old);
    let after = findings(run);
    let observed: BTreeSet<_> = run
        .observations
        .iter()
        .filter(|o| o.grade.verdict != Verdict::Error)
        .map(|o| o.case_id.clone())
        .collect();
    let now_cases: BTreeSet<_> = run.cases.iter().map(|c| c.id.clone()).collect();
    run.comparison
        .insert("new".into(), after.difference(&before).cloned().collect());
    // A stochastic non-observation is not proof a previous problem was fixed.
    if run.mode == Mode::Deterministic && run.complete {
        run.comparison.insert(
            "resolved".into(),
            before
                .difference(&after)
                .filter(|id| observed.contains(*id))
                .cloned()
                .collect(),
        );
    } else {
        run.comparison.insert(
            "not_observed_this_run".into(),
            before
                .difference(&after)
                .filter(|id| now_cases.contains(*id))
                .cloned()
                .collect(),
        );
    }
    run.comparison.insert(
        "removed_from_suite".into(),
        before.difference(&now_cases).cloned().collect(),
    );
    Ok(())
}

fn save(run: &mut Run, path: &Path, latest: &Path, baseline: Option<&Path>) -> Result<()> {
    run.updated_at = storage::now()?;
    if let Some(baseline) = baseline {
        compare(run, baseline)?;
    }
    storage::write_json(path, run)?;
    storage::write_json(latest, run)
}

fn publish_saved(run: &mut Run, latest: &Path, baseline: Option<&Path>) -> Result<()> {
    if let Some(baseline) = baseline {
        compare(run, baseline)?;
    }
    storage::write_json(latest, run)
}

#[derive(Clone)]
struct Job {
    group: usize,
    case: usize,
    sequence: usize,
}

pub async fn execute(suite: &Suite, root: &Path, options: &Options) -> Result<Run> {
    contract::validate(suite)?;
    let _lock = storage::lock(&options.state)?;
    let base_fingerprint = storage::fingerprint(suite, root)?;
    // Subset diagnostics must never poison a full-suite cache or resume identity.
    let fingerprint = match &options.case_id {
        Some(id) => hex::encode(Sha256::digest(format!("subset:{base_fingerprint}:{id}"))),
        None => base_fingerprint,
    };
    let now = storage::now()?;
    let latest = options.state.join("latest.json");
    let run_id = match &options.run_id {
        Some(id) => {
            storage::validate_run_id(id)?;
            id.clone()
        }
        None if suite.adapter.mode == Mode::Llm => {
            format!("{}-{}", &fingerprint[..24], now / 86_400)
        }
        None => format!(
            "{}-{}",
            &fingerprint[..24],
            SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
        ),
    };
    let path = options.state.join("runs").join(format!("{run_id}.json"));
    let (mut cases, unsupported) = contract::generate(suite)?;
    if let Some(id) = &options.case_id {
        cases.retain(|c| c.id == *id);
        ensure!(!cases.is_empty(), "case ID is not present in this suite");
    }
    let mut run = Run {
        version: 1,
        engine: VERSION.into(),
        id: run_id,
        fingerprint: fingerprint.clone(),
        suite_name: suite.name.clone(),
        mode: suite.adapter.mode,
        created_at: now,
        updated_at: now,
        seed: hex::encode(Sha256::digest(format!(
            "{fingerprint}:{now}:{}",
            SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
        ))),
        policy: suite.policy.clone(),
        status: Status::Inconclusive,
        message: "Collecting evidence".into(),
        metrics: metrics(&cases),
        cases,
        unsupported,
        observations: vec![],
        pending: 0,
        reserved_microusd: 0,
        actual_microusd: 0,
        unreported_cost_calls: 0,
        elapsed_ms: 0,
        complete: false,
        reused: false,
        comparison: Default::default(),
    };
    if !suite.confirmed {
        run.status = Status::Error;
        run.message =
            "Review the suggested contracts and set confirmed to true before grading".into();
        run.complete = true;
        save(&mut run, &path, &latest, options.baseline.as_deref())?;
        return Ok(run);
    }
    if suite.adapter.mode == Mode::Llm {
        ensure!(
            options.case_id.is_none(),
            "single-case replay is diagnostic only for deterministic graders; use a separate declared LLM suite"
        );
        ensure!(
            suite.adapter.fresh_calls,
            "LLM gates require fresh_calls=true and independent, uncached grader invocations"
        );
        ensure!(
            !suite.adapter.identity.trim().is_empty(),
            "LLM adapter identity must pin its model/version and grading configuration"
        );
        ensure!(
            suite.adapter.max_cost_microusd.is_some(),
            "declare max_cost_microusd for the complete grading callback, including retries"
        );
        ensure!(
            run.metrics.iter().all(|m| !m.population.is_empty()),
            "LLM suite requires both incorrect and valid cases"
        );
    }
    if path.exists() && options.case_id.is_none() {
        let previous: Run = storage::read_json(&path)?;
        ensure!(
            previous.version == 1 && previous.fingerprint == fingerprint,
            "run ID belongs to a different suite/configuration; choose an ID for this revision"
        );
        verify_history(&previous, &run)?;
        run = previous;
        ensure!(
            now.saturating_sub(run.created_at) < 86_400,
            "evidence is older than 24 hours; begin a new explicitly identified run"
        );
        run.policy.budget_microusd = suite.policy.budget_microusd;
        run.policy.time_limit_seconds = suite.policy.time_limit_seconds;
        if run.pending > 0 {
            run.status = Status::Error;
            run.complete = true;
            run.message="Run was interrupted with calls in flight. Charges remain reserved; missing outcomes cannot be retried as new evidence. Start a reviewed new run.".into();
        }
        if run.complete {
            run.reused = true;
            publish_saved(&mut run, &latest, options.baseline.as_deref())?;
            return Ok(run);
        }
    } else if latest.exists()
        && suite.adapter.cache_safe
        && options.case_id.is_none()
        && options.run_id.is_none()
    {
        let previous: Run = storage::read_json(&latest)?;
        if previous.fingerprint == fingerprint
            && previous.complete
            && previous.pending == 0
            && now.saturating_sub(previous.created_at) < 86_400
        {
            verify_history(&previous, &run)?;
            run = previous;
            run.reused = true;
            publish_saved(&mut run, &latest, options.baseline.as_deref())?;
            return Ok(run);
        }
    }
    run.reused = false;
    save(&mut run, &path, &latest, None)?;
    let started = Instant::now();
    let prior_elapsed = run.elapsed_ms;
    let limit_ms = suite.policy.time_limit_seconds.saturating_mul(1000);
    let timeout = Duration::from_secs(suite.policy.call_timeout_seconds);
    let mut workers = Vec::new();
    let remaining = Duration::from_millis(limit_ms.saturating_sub(prior_elapsed));
    let count = if remaining.is_zero() {
        0
    } else {
        suite.policy.concurrency.min(run.cases.len())
    };
    for result in
        join_all((0..count).map(|_| Worker::spawn(&suite.adapter, root, timeout.min(remaining))))
            .await
    {
        match result {
            Ok(w) => workers.push(w),
            Err(_) => {
                run.status = Status::Error;
                run.message="Adapter startup failed. Run doctor and verify the adapter command and runtime.".into();
                run.complete = true;
            }
        }
    }
    let cost = if suite.adapter.mode == Mode::Llm {
        suite
            .adapter
            .max_cost_microusd
            .context("missing cost bound")?
    } else {
        0
    };
    let mut terminal_error = false;
    while !run.complete && !workers.is_empty() {
        run.elapsed_ms =
            prior_elapsed.saturating_add(u64::try_from(started.elapsed().as_millis())?);
        if limit_ms.saturating_sub(run.elapsed_ms)
            < suite.policy.call_timeout_seconds.saturating_mul(1000)
        {
            run.message="Insufficient time remains for another bounded call; increase the time budget to resume this evidence".into();
            break;
        }
        let mut jobs = Vec::new();
        if suite.adapter.mode == Mode::Deterministic {
            let done = run.observations.len();
            for i in done..run.cases.len().min(done + workers.len()) {
                jobs.push(Job {
                    group: 0,
                    case: i,
                    sequence: run.observations.len() + jobs.len(),
                });
            }
        } else {
            // Round-robin unresolved metrics; never dispatch beyond a predetermined look.
            let mut allocated = vec![0u32; run.metrics.len()];
            'allocate: loop {
                let before = jobs.len();
                let mut order: Vec<usize> = (0..run.metrics.len()).collect();
                order.sort_by_key(|g| (run.metrics[*g].samples + allocated[*g], *g));
                for g in order {
                    let m = &run.metrics[g];
                    if jobs.len() >= workers.len() {
                        break 'allocate;
                    }
                    if m.decision != Status::Inconclusive {
                        continue;
                    }
                    let next = stats::LOOKS
                        .iter()
                        .copied()
                        .find(|n| *n > m.samples)
                        .unwrap_or(m.samples);
                    if m.samples + allocated[g] >= next {
                        continue;
                    }
                    let index = storage::sample(
                        &run.seed,
                        &m.id,
                        m.samples + allocated[g],
                        m.population.len(),
                    )?;
                    jobs.push(Job {
                        group: g,
                        case: m.population[index],
                        sequence: run.observations.len() + jobs.len(),
                    });
                    allocated[g] += 1;
                }
                if jobs.len() == before {
                    break;
                }
            }
        }
        if jobs.is_empty() {
            run.complete = true;
            break;
        }
        let available = run
            .policy
            .budget_microusd
            .saturating_sub(run.reserved_microusd);
        if let Some(calls) = available.checked_div(cost) {
            jobs.truncate(usize::try_from(calls).unwrap_or(usize::MAX));
        }
        if jobs.is_empty() {
            run.message =
                "Model cost budget exhausted; increase the budget to resume this evidence".into();
            break;
        }
        run.pending = jobs.len();
        run.reserved_microusd = run
            .reserved_microusd
            .checked_add(
                cost.checked_mul(u64::try_from(jobs.len())?)
                    .context("cost overflow")?,
            )
            .context("cost overflow")?;
        // Persist reservations BEFORE any invocation, including those lost to interruption.
        // The run file is the durable recovery source. Publishing the identical
        // full latest snapshot for every reservation doubled measured fsync cost.
        storage::write_json(&path, &run)?;
        run.elapsed_ms =
            prior_elapsed.saturating_add(u64::try_from(started.elapsed().as_millis())?);
        let remaining = Duration::from_millis(limit_ms.saturating_sub(run.elapsed_ms));
        if remaining < timeout {
            run.reserved_microusd = run
                .reserved_microusd
                .checked_sub(cost * u64::try_from(jobs.len())?)
                .context("invalid reservation")?;
            run.pending = 0;
            run.message="Insufficient time remains for another bounded call; increase the time budget to resume this evidence".into();
            break;
        }
        let results = join_all(
            workers
                .iter_mut()
                .zip(&jobs)
                .map(|(w, j)| w.grade(j.sequence, &run.cases[j.case], timeout.min(remaining))),
        )
        .await;
        for (job, result) in jobs.iter().zip(results) {
            let grade=match result {
                Ok(g)=>g,
                Err(_)=>Grade{verdict:Verdict::Error,reason:"Adapter failed, timed out, or returned an invalid response. This is not a rejection.".into(),score:None,cost_microusd:None},
            };
            if grade.verdict == Verdict::Error {
                terminal_error = true;
            }
            if let Some(actual) = grade.cost_microusd {
                run.actual_microusd = run
                    .actual_microusd
                    .checked_add(actual)
                    .context("actual cost overflow")?;
                if actual > cost {
                    terminal_error = true;
                    run.message =
                        "Grader exceeded its declared call-cost bound; remaining calls stopped"
                            .into();
                }
            } else if suite.adapter.mode == Mode::Llm {
                run.unreported_cost_calls += 1;
            }
            if suite.adapter.mode == Mode::Llm && grade.verdict != Verdict::Error {
                let m = &mut run.metrics[job.group];
                m.samples += 1;
                if grade.verdict != run.cases[job.case].expected {
                    m.errors += 1;
                }
            }
            run.observations.push(Observation {
                sequence: job.sequence,
                metric: if suite.adapter.mode == Mode::Llm {
                    run.metrics[job.group].id.clone()
                } else {
                    "deterministic".into()
                },
                case_id: run.cases[job.case].id.clone(),
                grade,
            });
        }
        run.pending = 0;
        run.elapsed_ms =
            prior_elapsed.saturating_add(u64::try_from(started.elapsed().as_millis())?);
        if terminal_error {
            run.status = Status::Error;
            run.complete = true;
            if !run.message.contains("cost bound") {
                run.message="At least one invocation failed. Partial evidence is retained, but cannot produce a passing gate.".into();
            }
        } else if suite.adapter.mode == Mode::Llm {
            let groups = run.metrics.len();
            for m in &mut run.metrics {
                stats::decide(m, &run.policy, groups)?;
            }
            if run.metrics.iter().any(|m| m.decision == Status::Fail) {
                run.status = Status::Fail;
                run.complete = true;
            } else if run.metrics.iter().all(|m| m.decision == Status::Pass) {
                run.status = Status::Pass;
                run.complete = true;
            }
        } else if run.observations.len() == run.cases.len() {
            run.status = if findings(&run).is_empty() {
                Status::Pass
            } else {
                Status::Fail
            };
            run.complete = true;
        }
        storage::write_json(&path, &run)?;
    }
    for worker in workers {
        if worker.shutdown().await.is_err() {
            run.status = Status::Error;
            run.complete = true;
            run.message = "Adapter cleanup failed".into();
        }
    }
    run.elapsed_ms = prior_elapsed.saturating_add(u64::try_from(started.elapsed().as_millis())?);
    if !run.unsupported.is_empty() {
        run.status = Status::Error;
        run.complete = true;
        run.message="Required contract coverage is unsupported. Review the listed rules; this suite cannot pass.".into();
    } else if run.status == Status::Pass {
        run.message = if options.case_id.is_some() {
            "Single-case diagnostic completed; this is not a suite gate".into()
        } else {
            "All required gate conditions are supported by the recorded evidence".into()
        };
    } else if run.status == Status::Fail {
        run.message = "The grader does not meet the configured error policy".into();
    } else if run.complete && run.status == Status::Inconclusive {
        run.message="All predetermined sampling points are exhausted without a decision. Review the grader or design a new statistical plan.".into();
    }
    save(&mut run, &path, &latest, options.baseline.as_deref())?;
    Ok(run)
}
