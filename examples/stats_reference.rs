use anyhow::Result;
fn main() -> Result<()> {
    let mut rows = Vec::new();
    for n in evalproof::stats::LOOKS {
        for k in [0, 1, n / 2, n - 1, n] {
            for alpha in [0.05, 0.01 / 12.0, 0.01 / 24.0] {
                let (lower, upper) = evalproof::stats::interval(k, n, alpha)?;
                rows.push(
                    serde_json::json!({"n":n,"k":k,"alpha":alpha,"lower":lower,"upper":upper}),
                );
            }
        }
    }
    println!("{}", serde_json::to_string(&rows)?);
    Ok(())
}
