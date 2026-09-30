use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Suite {
    pub version: u32,
    pub name: String,
    pub confirmed: bool,
    pub adapter: Adapter,
    pub fixtures: Vec<Fixture>,
    #[serde(default)]
    pub policy: Policy,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Adapter {
    pub command: Vec<String>,
    pub mode: Mode,
    /// Hash these files/directories as well as the suite and executable.
    #[serde(default)]
    pub dependencies: Vec<String>,
    #[serde(default)]
    pub environment: Vec<String>,
    /// Explicit assertion by the owner that dependencies and purity are complete.
    #[serde(default)]
    pub cache_safe: bool,
    /// Bounds the whole callback, including nested calls and retries, in microdollars.
    #[serde(default)]
    pub max_cost_microusd: Option<u64>,
    #[serde(default)]
    pub fresh_calls: bool,
    #[serde(default)]
    pub identity: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Deterministic,
    Llm,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fixture {
    pub id: String,
    pub output: Value,
    #[serde(default)]
    pub context: Value,
    pub rules: Vec<Rule>,
    #[serde(default)]
    pub semantic_json: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct Rule {
    pub id: String,
    /// RFC 6901 JSON pointer. Empty string addresses the root.
    pub path: String,
    #[serde(default)]
    pub critical: bool,
    #[serde(flatten)]
    pub constraint: Constraint,
}

impl<'de> Deserialize<'de> for Rule {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        let mut fields = BTreeMap::<String, Value>::deserialize(deserializer)?;
        let id = fields
            .remove("id")
            .and_then(|v| v.as_str().map(str::to_owned))
            .ok_or_else(|| D::Error::custom("rule id must be a string"))?;
        let path = fields
            .remove("path")
            .and_then(|v| v.as_str().map(str::to_owned))
            .ok_or_else(|| D::Error::custom("rule path must be a string"))?;
        let critical = match fields.remove("critical") {
            Some(Value::Bool(v)) => v,
            None => false,
            _ => return Err(D::Error::custom("critical must be boolean")),
        };
        let constraint = serde_json::from_value(Value::Object(fields.into_iter().collect()))
            .map_err(D::Error::custom)?;
        Ok(Self {
            id,
            path,
            critical,
            constraint,
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Constraint {
    Required,
    Type {
        value: JsonType,
    },
    Exact {
        value: Value,
    },
    Enum {
        values: Vec<Value>,
    },
    Number {
        value: String,
        tolerance: String,
    },
    Cardinality {
        min: usize,
        max: usize,
    },
    Unique {
        #[serde(default)]
        key: Option<String>,
    },
    Members {
        key: String,
        values: Vec<Value>,
    },
    Unordered {
        #[serde(default)]
        key: Option<String>,
    },
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JsonType {
    Null,
    Boolean,
    Number,
    String,
    Array,
    Object,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Policy {
    pub tolerance: f64,
    pub confidence: f64,
    pub budget_microusd: u64,
    pub time_limit_seconds: u64,
    pub call_timeout_seconds: u64,
    pub concurrency: usize,
    pub max_cases: usize,
}
impl Default for Policy {
    fn default() -> Self {
        Self {
            tolerance: 0.05,
            confidence: 0.99,
            budget_microusd: 5_000_000,
            time_limit_seconds: 300,
            call_timeout_seconds: 30,
            concurrency: 8,
            max_cases: 10_000,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Case {
    pub id: String,
    pub fixture_id: String,
    pub rule_id: Option<String>,
    pub path: String,
    pub operator: String,
    pub expected: Verdict,
    pub original: Value,
    pub output: Value,
    /// Serialized output is used only by graders configured to consume text.
    pub output_text: String,
    pub context: Value,
    pub proof: String,
    pub critical: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Accept,
    Reject,
    Error,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Grade {
    pub verdict: Verdict,
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    pub score: Option<f64>,
    #[serde(default)]
    pub cost_microusd: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Status {
    Pass,
    Fail,
    Inconclusive,
    Error,
}
impl Status {
    pub fn exit_code(self) -> i32 {
        match self {
            Self::Pass => 0,
            Self::Fail => 1,
            _ => 2,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Pass => "PASS",
            Self::Fail => "FAIL",
            Self::Inconclusive => "INCONCLUSIVE",
            Self::Error => "ERROR",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Observation {
    pub sequence: usize,
    pub metric: String,
    pub case_id: String,
    pub grade: Grade,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Metric {
    pub id: String,
    pub population: Vec<usize>,
    pub samples: u32,
    pub errors: u32,
    pub lower: f64,
    pub upper: f64,
    pub decision: Status,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Run {
    pub version: u32,
    pub engine: String,
    pub id: String,
    pub fingerprint: String,
    pub suite_name: String,
    pub mode: Mode,
    pub created_at: u64,
    pub updated_at: u64,
    pub seed: String,
    pub policy: Policy,
    pub status: Status,
    pub message: String,
    pub cases: Vec<Case>,
    pub unsupported: Vec<String>,
    pub metrics: Vec<Metric>,
    pub observations: Vec<Observation>,
    pub pending: usize,
    pub reserved_microusd: u64,
    pub actual_microusd: u64,
    pub unreported_cost_calls: usize,
    pub elapsed_ms: u64,
    pub complete: bool,
    pub reused: bool,
    #[serde(default)]
    pub comparison: BTreeMap<String, Vec<String>>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GradeRequest {
    pub version: u32,
    pub id: usize,
    pub output: Value,
    pub output_text: String,
    pub context: Value,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GradeResponse {
    pub version: u32,
    pub id: usize,
    pub result: Grade,
}
