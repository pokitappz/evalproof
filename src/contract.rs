use crate::model::*;
use anyhow::{Context, Result, bail, ensure};
use rust_decimal::Decimal;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, str::FromStr};

fn decimal(v: &Value) -> Result<Decimal> {
    ensure!(v.is_number(), "expected a JSON number");
    Decimal::from_str_exact(&v.to_string()).context("number exceeds supported decimal precision")
}
fn number(s: &str) -> Result<Decimal> {
    Decimal::from_str_exact(s).context("use an ordinary decimal with at most 28 fractional digits")
}
fn pointer(path: &str) -> Result<()> {
    ensure!(
        path.is_empty() || path.starts_with('/'),
        "path must be an RFC 6901 JSON pointer"
    );
    let mut chars = path.chars();
    while let Some(c) = chars.next() {
        if c == '~' {
            ensure!(
                matches!(chars.next(), Some('0' | '1')),
                "invalid JSON pointer escape"
            );
        }
    }
    Ok(())
}
fn same(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(_), Value::Number(_)) => match (decimal(a), decimal(b)) {
            (Ok(x), Ok(y)) => x == y,
            _ => a == b,
        },
        (Value::Array(a), Value::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(x, y)| same(x, y))
        }
        (Value::Object(a), Value::Object(b)) => {
            a.len() == b.len() && a.iter().all(|(k, v)| b.get(k).is_some_and(|x| same(v, x)))
        }
        _ => a == b,
    }
}
pub fn holds(output: &Value, rule: &Rule) -> Result<bool> {
    let value = output.pointer(&rule.path);
    if matches!(rule.constraint, Constraint::Required) {
        return Ok(value.is_some());
    }
    let Some(value) = value else {
        return Ok(true);
    };
    Ok(match &rule.constraint {
        Constraint::Required => true,
        Constraint::Type { value: ty } => match ty {
            JsonType::Null => value.is_null(),
            JsonType::Boolean => value.is_boolean(),
            JsonType::Number => value.is_number(),
            JsonType::String => value.is_string(),
            JsonType::Array => value.is_array(),
            JsonType::Object => value.is_object(),
        },
        Constraint::Exact { value: expected } => same(value, expected),
        Constraint::Enum { values } => values.iter().any(|v| same(v, value)),
        Constraint::Number {
            value: expected,
            tolerance,
        } => {
            if !value.is_number() {
                false
            } else {
                decimal(value)?
                    .checked_sub(number(expected)?)
                    .context("decimal subtraction overflow")?
                    .abs()
                    <= number(tolerance)?
            }
        }
        Constraint::Cardinality { min, max } => value
            .as_array()
            .is_some_and(|a| (*min..=*max).contains(&a.len())),
        Constraint::Unique { key } => {
            let Some(a) = value.as_array() else {
                return Ok(false);
            };
            let mut items: Vec<&Value> = Vec::new();
            for item in a {
                let Some(v) = (match key {
                    Some(k) => item.pointer(k),
                    None => Some(item),
                }) else {
                    return Ok(false);
                };
                if items.iter().any(|previous| same(previous, v)) {
                    return Ok(false);
                }
                items.push(v);
            }
            true
        }
        Constraint::Members { key, values } => {
            let Some(items) = value.as_array() else {
                return Ok(false);
            };
            let mut remaining: Vec<&Value> = values.iter().collect();
            for item in items {
                let Some(identity) = item.pointer(key) else {
                    return Ok(false);
                };
                let Some(index) = remaining.iter().position(|v| same(v, identity)) else {
                    return Ok(false);
                };
                remaining.remove(index);
            }
            remaining.is_empty()
        }
        Constraint::Unordered { key } => value.as_array().is_some_and(|a| {
            key.as_ref()
                .is_none_or(|k| a.iter().all(|v| v.pointer(k).is_some()))
        }),
    })
}

pub fn validate(suite: &Suite) -> Result<()> {
    ensure!(suite.version == 1, "unsupported suite version");
    ensure!(!suite.name.trim().is_empty(), "suite name is required");
    ensure!(!suite.fixtures.is_empty(), "suite has no fixtures");
    ensure!(
        suite.fixtures.len() <= 1000,
        "split suites with more than 1000 fixtures"
    );
    ensure!(
        !suite.adapter.command.is_empty() && !suite.adapter.command[0].is_empty(),
        "adapter command is required"
    );
    let p = &suite.policy;
    ensure!(
        p.tolerance > 0.0 && p.tolerance < 0.5,
        "tolerance must be between 0 and 0.5"
    );
    ensure!(
        p.confidence >= 0.9 && p.confidence < 1.0,
        "confidence must be at least 0.9 and below 1"
    );
    ensure!(
        (1..=32).contains(&p.concurrency),
        "concurrency must be 1-32"
    );
    ensure!(
        (1..=86_400).contains(&p.time_limit_seconds),
        "time limit must be 1-86400 seconds"
    );
    ensure!(
        (1..=600).contains(&p.call_timeout_seconds),
        "call timeout must be 1-600 seconds"
    );
    ensure!(
        (1..=100_000).contains(&p.max_cases),
        "max_cases must be 1-100000"
    );
    ensure!(
        p.budget_microusd <= 1_000_000_000,
        "budget exceeds $1000 per run"
    );
    let mut ids = BTreeSet::new();
    for f in &suite.fixtures {
        ensure!(
            serde_json::to_vec(&f.output)?.len() <= 128 * 1024
                && serde_json::to_vec(&f.context)?.len() <= 128 * 1024,
            "fixture output or context exceeds 128 KiB; split large fixtures"
        );
        ensure!(
            !f.id.is_empty() && f.id.len() <= 200 && ids.insert(&f.id),
            "fixture IDs must be nonempty and unique"
        );
        ensure!(!f.rules.is_empty(), "fixture {} has no rules", f.id);
        ensure!(
            f.rules.len() <= 64,
            "split fixtures with more than 64 rules"
        );
        let mut rules = BTreeSet::new();
        for r in &f.rules {
            ensure!(
                !r.id.is_empty() && rules.insert(&r.id),
                "rule IDs must be unique within a fixture"
            );
            pointer(&r.path)?;
            match &r.constraint {
                Constraint::Enum { values } => ensure!(!values.is_empty(), "enum must have values"),
                Constraint::Number { value, tolerance } => {
                    number(value)?;
                    ensure!(
                        number(tolerance)? >= Decimal::ZERO,
                        "tolerance cannot be negative"
                    );
                }
                Constraint::Cardinality { min, max } => ensure!(
                    min <= max && *max <= 10_000,
                    "cardinality requires 0 <= min <= max <= 10000"
                ),
                Constraint::Unique { key: Some(k) } | Constraint::Unordered { key: Some(k) } => {
                    pointer(k)?
                }
                Constraint::Members { key, .. } => pointer(key)?,
                _ => (),
            }
            ensure!(
                holds(&f.output, r).with_context(|| format!("fixture {}, rule {}", f.id, r.id))?,
                "original fixture {} violates rule {}; correct the fixture or contract",
                f.id,
                r.id
            );
        }
    }
    Ok(())
}

fn replace(output: &Value, path: &str, new: Value) -> Option<Value> {
    let mut copy = output.clone();
    *copy.pointer_mut(path)? = new;
    Some(copy)
}
fn remove(output: &Value, path: &str) -> Option<Value> {
    let (parent, key) = path.rsplit_once('/')?;
    let key = key.replace("~1", "/").replace("~0", "~");
    let mut copy = output.clone();
    match copy.pointer_mut(parent)? {
        Value::Object(o) => {
            o.remove(&key)?;
        }
        Value::Array(a) => {
            let i = usize::from_str(&key).ok()?;
            if i >= a.len() {
                return None;
            }
            a.remove(i);
        }
        _ => return None,
    }
    Some(copy)
}

fn alternatives(v: &Value) -> Vec<Value> {
    let mut out = match v {
        Value::String(s) => vec![json!(format!("{s}__changed")), json!("")],
        Value::Bool(b) => vec![json!(!b)],
        Value::Number(_) => decimal(v)
            .ok()
            .and_then(|n| n.checked_add(Decimal::ONE))
            .and_then(|n| serde_json::from_str(&n.to_string()).ok())
            .into_iter()
            .collect(),
        Value::Array(a) => {
            let mut a = a.clone();
            a.push(Value::Null);
            vec![json!(a)]
        }
        Value::Object(o) => {
            let mut o = o.clone();
            o.insert("__changed".into(), Value::Bool(true));
            vec![json!(o)]
        }
        Value::Null => vec![json!(false)],
    };
    out.extend([
        Value::Null,
        json!("__invalid__"),
        json!(0),
        json!(true),
        json!([]),
        json!({}),
    ]);
    out
}

fn case(
    f: &Fixture,
    r: Option<&Rule>,
    operator: &str,
    output: Value,
    expected: Verdict,
    text: Option<String>,
) -> Result<Case> {
    let output_text = text.unwrap_or(serde_json::to_string(&output)?);
    let digest = hex::encode(Sha256::digest(serde_json::to_vec(&(
        &f.id,
        r.map(|r| &r.id),
        operator,
        &output_text,
    ))?));
    Ok(Case {
        id: digest[..24].into(),
        fixture_id: f.id.clone(),
        rule_id: r.map(|r| r.id.clone()),
        path: r.map_or_else(String::new, |r| r.path.clone()),
        operator: operator.into(),
        expected,
        original: f.output.clone(),
        output,
        output_text,
        context: f.context.clone(),
        proof: match r {
            Some(r) => format!(
                "{} {}: {}",
                if expected == Verdict::Reject {
                    "Violates"
                } else {
                    "Preserves"
                },
                r.id,
                serde_json::to_string(&r.constraint)?
            ),
            None => "Original output satisfies every confirmed contract rule".into(),
        },
        critical: r.is_some_and(|r| r.critical),
    })
}

pub fn generate(suite: &Suite) -> Result<(Vec<Case>, Vec<String>)> {
    validate(suite)?;
    let mut cases = Vec::new();
    let mut unsupported = Vec::new();
    let mut population_bytes = 0usize;
    for f in &suite.fixtures {
        let fixture_start = cases.len();
        cases.push(case(
            f,
            None,
            "original",
            f.output.clone(),
            Verdict::Accept,
            None,
        )?);
        if f.semantic_json {
            cases.push(case(
                f,
                None,
                "json_whitespace",
                f.output.clone(),
                Verdict::Accept,
                Some(serde_json::to_string_pretty(&f.output)?),
            )?);
        }
        for r in &f.rules {
            let mut candidates: Vec<(&str, Value)> = Vec::new();
            let current = f.output.pointer(&r.path);
            match &r.constraint {
                Constraint::Required => {
                    if let Some(v) = remove(&f.output, &r.path) {
                        candidates.push(("remove_required", v));
                    }
                }
                Constraint::Unordered { .. } => {
                    if let Some(Value::Array(a)) = current {
                        let mut a = a.clone();
                        a.reverse();
                        if let Some(v) = replace(&f.output, &r.path, json!(a)) {
                            let valid = f
                                .rules
                                .iter()
                                .map(|rule| holds(&v, rule))
                                .collect::<Result<Vec<_>>>()?
                                .into_iter()
                                .all(|v| v);
                            if valid && !same(&v, &f.output) {
                                cases.push(case(
                                    f,
                                    Some(r),
                                    "reorder_unordered",
                                    v,
                                    Verdict::Accept,
                                    None,
                                )?);
                            }
                        }
                    }
                    continue;
                }
                Constraint::Number { value, tolerance } => {
                    let center = number(value)?;
                    let tol = number(tolerance)?;
                    for boundary in [
                        center
                            .checked_add(tol)
                            .and_then(|n| n.checked_add(Decimal::ONE)),
                        center
                            .checked_sub(tol)
                            .and_then(|n| n.checked_sub(Decimal::ONE)),
                    ]
                    .into_iter()
                    .flatten()
                    {
                        if let Some(v) = replace(
                            &f.output,
                            &r.path,
                            serde_json::from_str(&boundary.to_string())?,
                        ) {
                            candidates.push(("outside_tolerance", v));
                        }
                    }
                }
                Constraint::Cardinality { min, max } => {
                    if let Some(Value::Array(a)) = current {
                        if *min > 0
                            && let Some(v) =
                                replace(&f.output, &r.path, json!(&a[..min.saturating_sub(1)]))
                        {
                            candidates.push(("remove_array_items", v));
                        }
                        let mut expanded = a.clone();
                        let item_bytes =
                            serde_json::to_vec(a.first().unwrap_or(&Value::Null))?.len();
                        ensure!(
                            item_bytes.saturating_mul(max.saturating_add(1)) <= 256 * 1024,
                            "cardinality mutation would exceed the 256 KiB output bound; reduce array limits or fixture size"
                        );
                        expanded.resize(
                            max.saturating_add(1),
                            a.first().cloned().unwrap_or(Value::Null),
                        );
                        if let Some(v) = replace(&f.output, &r.path, json!(expanded)) {
                            candidates.push(("extra_array_items", v));
                        }
                    }
                }
                Constraint::Unique { .. } => {
                    if let Some(Value::Array(a)) = current
                        && let Some(first) = a.first()
                    {
                        let mut a = a.clone();
                        a.push(first.clone());
                        if let Some(v) = replace(&f.output, &r.path, json!(a)) {
                            candidates.push(("duplicate_array_item", v));
                        }
                    }
                }
                Constraint::Members { key, .. } => {
                    if let Some(Value::Array(items)) = current
                        && let Some(first) = items.first()
                        && let Some(identity) = first.pointer(key)
                    {
                        for alternative in alternatives(identity) {
                            if let Some(changed) = replace(first, key, alternative) {
                                let mut altered = items.clone();
                                altered[0] = changed;
                                if let Some(v) = replace(&f.output, &r.path, json!(altered)) {
                                    candidates.push(("wrong_item_identity", v));
                                }
                            }
                        }
                    }
                }
                _ => {
                    if let Some(v) = current {
                        for candidate in alternatives(v) {
                            if let Some(v) = replace(&f.output, &r.path, candidate) {
                                candidates.push(("change_value", v));
                            }
                        }
                    }
                }
            }
            let mut seen = BTreeSet::new();
            let mut count = 0;
            for (op, output) in candidates {
                if !holds(&output, r)? && seen.insert(serde_json::to_string(&output)?) {
                    cases.push(case(f, Some(r), op, output, Verdict::Reject, None)?);
                    count += 1;
                    // Keep mutation populations bounded and predictable per rule.
                    if count == 2 {
                        break;
                    }
                }
            }
            if count == 0 {
                unsupported.push(format!(
                    "{} / {}: no provable incorrect mutation; provide an applicable fixture",
                    f.id, r.id
                ));
            }
            if cases.len() > suite.policy.max_cases {
                bail!(
                    "generated cases exceed max_cases; split the suite or raise its explicit limit"
                );
            }
        }
        for c in &cases[fixture_start..] {
            population_bytes = population_bytes.saturating_add(serde_json::to_vec(c)?.len());
        }
        ensure!(
            population_bytes <= 64 * 1024 * 1024,
            "mutation evidence exceeds 64 MiB; split the suite"
        );
        ensure!(
            cases.len() <= suite.policy.max_cases,
            "generated cases exceed max_cases"
        );
    }
    ensure!(
        cases.len() <= suite.policy.max_cases,
        "generated cases exceed max_cases"
    );
    if !cases.iter().any(|c| c.expected == Verdict::Reject) {
        unsupported.push("No provable incorrect cases were generated".into());
    }
    Ok((cases, unsupported))
}

/// Suggestions intentionally require human confirmation before any gate can pass.
pub fn infer_rules(output: &Value) -> Vec<Rule> {
    fn visit(v: &Value, path: String, out: &mut Vec<Rule>) {
        if let Value::Object(o) = v {
            for (k, v) in o {
                visit(
                    v,
                    format!("{path}/{}", k.replace('~', "~0").replace('/', "~1")),
                    out,
                );
            }
        } else {
            if !path.is_empty() {
                out.push(Rule {
                    id: format!("required:{path}"),
                    path: path.clone(),
                    critical: false,
                    constraint: Constraint::Required,
                });
            }
            out.push(Rule {
                id: format!("exact:{path}"),
                path,
                critical: false,
                constraint: Constraint::Exact { value: v.clone() },
            });
        }
    }
    let mut rules = Vec::new();
    visit(output, String::new(), &mut rules);
    if rules.is_empty() {
        rules.push(Rule {
            id: "exact:root".into(),
            path: String::new(),
            critical: false,
            constraint: Constraint::Exact {
                value: output.clone(),
            },
        });
    }
    rules
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn item_identity_is_independent_of_order() -> Result<()> {
        let rule = Rule {
            id: "items".into(),
            path: "/items".into(),
            critical: false,
            constraint: Constraint::Members {
                key: "/id".into(),
                values: vec![json!("a"), json!("b")],
            },
        };
        assert!(holds(&json!({"items":[{"id":"b"},{"id":"a"}]}), &rule)?);
        assert!(!holds(&json!({"items":[{"id":"a"},{"id":"a"}]}), &rule)?);
        assert!(!holds(&json!({"items":[{"id":"a"},{"id":"c"}]}), &rule)?);
        Ok(())
    }
    #[test]
    fn decimal_tolerance_is_exact() -> Result<()> {
        let rule = Rule {
            id: "price".into(),
            path: "/price".into(),
            critical: false,
            constraint: Constraint::Number {
                value: "0.3".into(),
                tolerance: "0.1".into(),
            },
        };
        assert!(holds(&json!({"price":0.4}), &rule)?);
        assert!(!holds(&json!({"price":0.4000000000000001}), &rule)?);
        Ok(())
    }
    #[test]
    fn missing_and_null_are_distinct() -> Result<()> {
        let r = Rule {
            id: "x".into(),
            path: "/x".into(),
            critical: false,
            constraint: Constraint::Required,
        };
        assert!(holds(&json!({"x":null}), &r)?);
        assert!(!holds(&json!({}), &r)?);
        Ok(())
    }
    #[test]
    fn escaped_pointer_removal() {
        assert_eq!(
            remove(&json!({"a/b":{"~x":1}}), "/a~1b/~0x"),
            Some(json!({"a/b":{}}))
        );
    }
    #[test]
    fn every_emitted_label_has_a_proof() -> Result<()> {
        let f = Fixture {
            id: "sample".into(),
            output: json!({"n":3,"s":"yes","a":[1,2]}),
            context: Value::Null,
            rules: infer_rules(&json!({"n":3,"s":"yes","a":[1,2]})),
            semantic_json: true,
        };
        let suite = Suite {
            version: 1,
            name: "test".into(),
            confirmed: true,
            fixtures: vec![f],
            policy: Policy::default(),
            adapter: Adapter {
                command: vec!["unused".into()],
                mode: Mode::Deterministic,
                dependencies: vec![],
                environment: vec![],
                cache_safe: false,
                max_cost_microusd: None,
                fresh_calls: false,
                identity: String::new(),
            },
        };
        let (cases, unsupported) = generate(&suite)?;
        assert!(unsupported.is_empty());
        for c in cases {
            let all = suite.fixtures[0]
                .rules
                .iter()
                .map(|r| holds(&c.output, r))
                .collect::<Result<Vec<_>>>()?
                .into_iter()
                .all(|x| x);
            assert_eq!(all, c.expected == Verdict::Accept);
        }
        Ok(())
    }
}
