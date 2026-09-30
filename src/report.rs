use crate::model::*;
use anyhow::Result;
use std::collections::BTreeMap;

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
pub fn summary(run: &Run) -> String {
    format!(
        "{}: {}\n{} cases; {} calls; reserved ${:.4}; reported ${:.4}; {:.2}s{}\n{}\n",
        run.status.label(),
        run.suite_name,
        run.cases.len(),
        run.observations.len(),
        run.reserved_microusd as f64 / 1_000_000.0,
        run.actual_microusd as f64 / 1_000_000.0,
        run.elapsed_ms as f64 / 1000.0,
        if run.reused { " (saved evidence)" } else { "" },
        run.message
    )
}
pub fn markdown(run: &Run) -> String {
    // CI summaries deliberately omit customer payloads and grader reasons.
    format!(
        "## EvalProof: {}\n\n{} cases, {} grading calls. Reserved model cost: ${:.4}.\n\n{}\n\nEvidence created at Unix timestamp {}. {}\n",
        run.status.label(),
        run.cases.len(),
        run.observations.len(),
        run.reserved_microusd as f64 / 1_000_000.0,
        if run.status == Status::Pass {
            "Required conditions are supported by the saved evidence."
        } else {
            "Open the local report for findings and next steps."
        },
        run.created_at,
        if run.reused {
            "This run reused saved evidence."
        } else {
            ""
        }
    )
}
pub fn html(run: &Run) -> Result<String> {
    let mut out = String::from(
        r##"<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'unsafe-inline'; base-uri 'none'; form-action 'none'"><title>EvalProof evidence report</title><style>
:root{color-scheme:light dark;--bg:#f6f7fa;--panel:#fff;--text:#182131;--muted:#4b596d;--line:#cbd3df;--accent:#254fba;--bad:#a71d32;--good:#17643e;--warn:#765100;--radius:12px}
@media(prefers-color-scheme:dark){:root{--bg:#111722;--panel:#1a2331;--text:#edf1f8;--muted:#b2bfd3;--line:#46536b;--accent:#abc4ff;--bad:#ffacb7;--good:#9bdfb8;--warn:#f3cf82}}
*{box-sizing:border-box}body{margin:0;background:var(--bg);color:var(--text);font:16px/1.55 system-ui,sans-serif}main{max-width:1120px;margin:auto;padding:32px 20px 64px}h1{font-size:clamp(26px,4vw,38px);line-height:1.2;margin:8px 0}h2{margin-top:32px;font-size:22px}h3{font-size:18px}p{max-width:85ch}a{color:var(--accent)}a:focus-visible,summary:focus-visible{outline:3px solid var(--accent);outline-offset:4px}.eyebrow{font-weight:700;color:var(--accent);letter-spacing:.08em}.muted{color:var(--muted)}.status{font-weight:750}.PASS{color:var(--good)}.FAIL,.ERROR{color:var(--bad)}.INCONCLUSIVE{color:var(--warn)}.panel,details{background:var(--panel);border:1px solid var(--line);border-radius:var(--radius);padding:20px;margin:16px 0}.grid{display:grid;grid-template-columns:repeat(2,minmax(0,1fr));gap:16px}pre{white-space:pre-wrap;overflow-wrap:anywhere;font:14px/1.5 ui-monospace,monospace;background:var(--bg);padding:16px;border-radius:8px;max-height:480px;overflow:auto}code{overflow-wrap:anywhere}summary{cursor:pointer;min-height:44px;overflow-wrap:anywhere}table{border-collapse:collapse;width:100%;font-variant-numeric:tabular-nums}td,th{padding:12px;text-align:left;border-bottom:1px solid var(--line)}th{font-size:14px;color:var(--muted)}.table-wrap{overflow:auto}.number{text-align:right}li{margin:8px 0;overflow-wrap:anywhere}.skip{position:absolute;left:-10000px}.skip:focus{left:12px;top:8px}@media(max-width:650px){.grid{grid-template-columns:1fr}main{padding:24px 14px}td,th{padding:10px 8px}.panel,details{padding:16px}}
</style></head><body><a class="skip" href="#findings">Skip to findings</a><main><div class="eyebrow">EVALPROOF / GRADER EVIDENCE</div>"##,
    );
    out.push_str(&format!(
        "<h1>{}</h1><p class=\"status {}\">{}</p><p>{}</p>",
        escape(&run.suite_name),
        run.status.label(),
        run.status.label(),
        escape(&run.message)
    ));
    out.push_str(&format!("<div class=\"panel\"><strong>{} cases / {} grading calls</strong><p>Reserved: ${:.4} &middot; Reported actual: ${:.4} &middot; Calls without cost reporting: {} &middot; Time: {:.2}s</p><p class=\"muted\">Evidence created: {} (Unix time). {} Run: <code>{}</code></p></div>",run.cases.len(),run.observations.len(),run.reserved_microusd as f64/1_000_000.0,run.actual_microusd as f64/1_000_000.0,run.unreported_cost_calls,run.elapsed_ms as f64/1000.0,run.created_at,if run.reused{"Reused saved evidence."}else{"Fresh run."},escape(&run.id)));
    if run.mode == Mode::Llm {
        out.push_str(&format!("<h2>Statistical evidence</h2><p>At most {:.1}% error per metric with {:.1}% simultaneous confidence across the predeclared decision points. This describes the frozen test population under independent, stable grader calls. It does not measure production-wide accuracy. Intervals below are from the last completed decision point.</p><div class=\"table-wrap\"><table><thead><tr><th>Metric</th><th>Samples</th><th>Errors</th><th>Error-rate interval</th><th>Decision</th></tr></thead><tbody>",run.policy.tolerance*100.0,run.policy.confidence*100.0));
        for m in &run.metrics {
            out.push_str(&format!("<tr><td><code>{}</code></td><td class=\"number\">{}</td><td class=\"number\">{}</td><td>{:.2}% - {:.2}%</td><td>{}</td></tr>",escape(&m.id),m.samples,m.errors,m.lower*100.0,m.upper*100.0,m.decision.label()));
        }
        out.push_str("</tbody></table></div>");
    }
    if !run.unsupported.is_empty() {
        out.push_str("<h2>Coverage requiring attention</h2><ul>");
        for u in &run.unsupported {
            out.push_str(&format!("<li>{}</li>", escape(u)));
        }
        out.push_str("</ul>");
    }
    if !run.comparison.is_empty() {
        out.push_str("<h2>Compared with baseline</h2><ul>");
        for (k, v) in &run.comparison {
            out.push_str(&format!(
                "<li>{}: {}</li>",
                escape(&k.replace('_', " ")),
                v.len()
            ));
        }
        out.push_str("</ul>");
    }
    let mut observations: BTreeMap<&str, Vec<&Observation>> = BTreeMap::new();
    for o in &run.observations {
        observations.entry(&o.case_id).or_default().push(o);
    }
    out.push_str("<h2 id=\"findings\">Findings</h2>");
    let mut count = 0;
    for c in &run.cases {
        let obs = observations.get(c.id.as_str());
        let bad: Vec<_> = obs
            .into_iter()
            .flatten()
            .filter(|o| o.grade.verdict != c.expected)
            .collect();
        if bad.is_empty() {
            continue;
        }
        count += 1;
        out.push_str(&format!("<details><summary><strong>{}</strong> / {} / {} ({} unexpected responses)</summary><p>{}</p><p>Expected: <strong>{:?}</strong>. Operator: <code>{}</code>.</p><div class=\"grid\"><div><h3>Original</h3><pre>{}</pre></div><div><h3>Tested output</h3><pre>{}</pre></div></div>",escape(&c.fixture_id),escape(&c.path),escape(c.rule_id.as_deref().unwrap_or("valid control")),bad.len(),escape(&c.proof),c.expected,escape(&c.operator),escape(&serde_json::to_string_pretty(&c.original)?),escape(&c.output_text)));
        for o in bad.iter().take(5) {
            out.push_str(&format!(
                "<p>Grader returned <strong>{:?}</strong>: {}</p>",
                o.grade.verdict,
                escape(&o.grade.reason)
            ));
        }
        if run.mode == Mode::Deterministic {
            out.push_str(&format!("<p>Reproduce using your suite path:</p><pre>evalproof diagnose --suite evalproof.json --case {}</pre>",escape(&c.id)));
        }
        out.push_str(&format!(
            "<p class=\"muted\">Case ID: <code>{}</code></p></details>",
            escape(&c.id)
        ));
    }
    if count == 0 {
        out.push_str(if run.status==Status::Pass{"<p>No mismatches were observed in this run. See coverage and statistical scope above.</p>"}else{"<p>No mismatches have been observed, but this run has not established a passing result. Resolve the status above before relying on it.</p>"});
    }
    out.push_str(&format!("<h2>Provenance</h2><div class=\"panel\"><p>Engine: {} / Protocol: {}</p><p>Configuration fingerprint: <code>{}</code></p><p>Sampling seed: <code>{}</code></p><p class=\"muted\">The JSON run artifact contains the complete cases and observations. This report contains customer data; share it only through your own approved storage.</p></div><p class=\"muted\">Gate every pull request on this evidence with EvalProof Team: <a href=\"{}\" rel=\"noreferrer\">start a free trial</a>.</p></main></body></html>",escape(&run.engine),run.version,escape(&run.fingerprint),escape(&run.seed),escape(crate::license::checkout_url())));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn escapes_untrusted_html() {
        assert_eq!(escape("<script>\"'&"), "&lt;script&gt;&quot;&#39;&amp;");
    }
}
