use crate::model::{Metric, Policy, Status};
use anyhow::{Result, ensure};
use statrs::distribution::{Beta, ContinuousCDF};

pub const LOOKS: [u32; 6] = [20, 40, 80, 160, 320, 640];

pub fn interval(errors: u32, samples: u32, alpha: f64) -> Result<(f64, f64)> {
    ensure!(errors <= samples && samples > 0, "invalid binomial counts");
    ensure!(alpha > 0.0 && alpha < 1.0, "invalid interval alpha");
    let tail = alpha / 2.0;
    let lower = if errors == 0 {
        0.0
    } else {
        Beta::new(f64::from(errors), f64::from(samples - errors + 1))?.inverse_cdf(tail)
    };
    let upper = if errors == samples {
        1.0
    } else {
        Beta::new(f64::from(errors + 1), f64::from(samples - errors))?.inverse_cdf(1.0 - tail)
    };
    ensure!(
        lower.is_finite() && upper.is_finite() && lower <= upper,
        "invalid statistical result"
    );
    Ok((lower, upper))
}

pub fn decide(metric: &mut Metric, policy: &Policy, metrics: usize) -> Result<()> {
    ensure!(metrics > 0, "no metrics");
    if !LOOKS.contains(&metric.samples) {
        return Ok(());
    }
    let groups = f64::from(u32::try_from(metrics)?);
    let alpha = (1.0 - policy.confidence) / (groups * 6.0);
    (metric.lower, metric.upper) = interval(metric.errors, metric.samples, alpha)?;
    metric.decision = if metric.upper <= policy.tolerance {
        Status::Pass
    } else if metric.lower > policy.tolerance {
        Status::Fail
    } else {
        Status::Inconclusive
    };
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_endpoints_and_known_reference() -> Result<()> {
        let (lo, hi) = interval(0, 20, 0.05)?;
        assert_eq!(lo, 0.0);
        assert!((hi - (1.0 - 0.025_f64.powf(1.0 / 20.0))).abs() < 1e-10);
        let (lo, hi) = interval(5, 10, 0.05)?;
        assert!((lo - 0.1870860284473985).abs() < 1e-10);
        assert!((hi - 0.8129139715526015).abs() < 1e-10);
        Ok(())
    }
    #[test]
    fn clean_default_run_requires_160_per_metric() -> Result<()> {
        let p = Policy::default();
        let mut m = Metric {
            id: "misses".into(),
            population: vec![0],
            samples: 80,
            errors: 0,
            lower: 0.0,
            upper: 1.0,
            decision: Status::Inconclusive,
        };
        decide(&mut m, &p, 2)?;
        assert_eq!(m.decision, Status::Inconclusive);
        m.samples = 160;
        decide(&mut m, &p, 2)?;
        assert_eq!(m.decision, Status::Pass);
        Ok(())
    }
    #[test]
    fn additional_critical_metrics_raise_evidence_requirement() -> Result<()> {
        let mut m = Metric {
            id: "critical".into(),
            population: vec![0],
            samples: 160,
            errors: 0,
            lower: 0.0,
            upper: 1.0,
            decision: Status::Inconclusive,
        };
        decide(&mut m, &Policy::default(), 4)?;
        assert_eq!(m.decision, Status::Inconclusive);
        Ok(())
    }
}
