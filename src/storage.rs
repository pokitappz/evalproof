use crate::{VERSION, model::Suite};
use std::sync::atomic::{AtomicU64, Ordering};
static SERIALIZE_NS: AtomicU64 = AtomicU64::new(0);
static WRITE_NS: AtomicU64 = AtomicU64::new(0);
static SYNC_NS: AtomicU64 = AtomicU64::new(0);
static READ_NS: AtomicU64 = AtomicU64::new(0);

pub fn profile() -> serde_json::Value {
    serde_json::json!({"serialize_ms": SERIALIZE_NS.load(Ordering::Relaxed)/1_000_000,
        "write_ms": WRITE_NS.load(Ordering::Relaxed)/1_000_000,
        "sync_ms": SYNC_NS.load(Ordering::Relaxed)/1_000_000,
        "read_ms": READ_NS.load(Ordering::Relaxed)/1_000_000})
}
use anyhow::{Context, Result, ensure};
use fs2::FileExt;
use serde::{Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{BufReader, Read, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

pub fn now() -> Result<u64> {
    Ok(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs())
}

pub fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let start = std::time::Instant::now();
    let file = File::open(path).with_context(|| format!("cannot open {}", path.display()))?;
    ensure!(
        file.metadata()?.len() <= 128 * 1024 * 1024,
        "JSON file exceeds 128 MiB limit"
    );
    // Profiling showed unbuffered JSON reads dominated cached runs (1.31 seconds).
    let result = serde_json::from_reader(BufReader::with_capacity(
        64 * 1024,
        file.take(128 * 1024 * 1024 + 1),
    ))
    .context("invalid JSON or unsupported configuration field");
    READ_NS.fetch_add(
        u64::try_from(start.elapsed().as_nanos()).unwrap_or(u64::MAX),
        Ordering::Relaxed,
    );
    result
}

pub fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let start = std::time::Instant::now();
    let bytes = serde_json::to_vec_pretty(value)?;
    SERIALIZE_NS.fetch_add(
        u64::try_from(start.elapsed().as_nanos()).unwrap_or(u64::MAX),
        Ordering::Relaxed,
    );
    write_atomic(path, &bytes)
}
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let start = std::time::Instant::now();
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    temp.write_all(bytes)?;
    let sync_start = std::time::Instant::now();
    temp.as_file().sync_all()?;
    SYNC_NS.fetch_add(
        u64::try_from(sync_start.elapsed().as_nanos()).unwrap_or(u64::MAX),
        Ordering::Relaxed,
    );
    temp.persist(path).map_err(|e| e.error)?;
    WRITE_NS.fetch_add(
        u64::try_from(start.elapsed().as_nanos()).unwrap_or(u64::MAX),
        Ordering::Relaxed,
    );
    Ok(())
}

pub fn lock(dir: &Path) -> Result<File> {
    fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    }
    let f = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(dir.join("run.lock"))?;
    f.try_lock_exclusive()
        .context("another evalproof run is active in this state directory")?;
    Ok(f)
}

pub fn executable(command: &str, root: &Path) -> Result<PathBuf> {
    let p = Path::new(command);
    if p.is_absolute() || p.components().count() > 1 {
        return fs::canonicalize(root.join(p)).context("adapter executable not found");
    }
    let path = std::env::var_os("PATH").context("PATH is missing")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(command);
        if candidate.is_file() {
            return Ok(fs::canonicalize(candidate)?);
        }
        #[cfg(windows)]
        {
            let candidate = dir.join(format!("{command}.exe"));
            if candidate.is_file() {
                return Ok(fs::canonicalize(candidate)?);
            }
        }
    }
    anyhow::bail!("adapter executable {command} is not on PATH")
}

fn hash_file(hasher: &mut Sha256, path: &Path) -> Result<()> {
    let mut f = File::open(path)?;
    ensure!(
        f.metadata()?.len() <= 256 * 1024 * 1024,
        "dependency file exceeds 256 MiB"
    );
    let mut buf = [0u8; 65536];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(())
}
fn ignored(entry: &walkdir::DirEntry) -> bool {
    matches!(
        entry.file_name().to_str(),
        Some(
            ".git"
                | ".evalproof"
                | "node_modules"
                | "target"
                | ".venv"
                | "__pycache__"
                | "dist"
                | "build"
        )
    )
}
pub fn fingerprint(suite: &Suite, root: &Path) -> Result<String> {
    let mut h = Sha256::new();
    h.update(VERSION);
    hash_file(&mut h, &std::env::current_exe()?)?;
    // Raising execution budgets can resume a run, but never changes its statistical plan.
    let mut stable = suite.clone();
    stable.policy.budget_microusd = 0;
    stable.policy.time_limit_seconds = 0;
    h.update(serde_json::to_vec(&stable)?);
    hash_file(&mut h, &executable(&suite.adapter.command[0], root)?)?;
    let mut paths = std::collections::BTreeSet::new();
    for dep in &suite.adapter.dependencies {
        let path = root.join(dep);
        ensure!(path.exists(), "declared dependency does not exist: {dep}");
        if path.is_dir() {
            for entry in walkdir::WalkDir::new(&path)
                .sort_by_file_name()
                .into_iter()
                .filter_entry(|e| !ignored(e))
            {
                let entry = entry?;
                ensure!(
                    !entry.file_type().is_symlink(),
                    "symlink dependency requires an explicit real path"
                );
                if entry.file_type().is_file() {
                    paths.insert(entry.into_path());
                }
                ensure!(
                    paths.len() <= 20_000,
                    "too many dependency files; narrow dependency directories"
                );
            }
        } else {
            paths.insert(path);
        }
    }
    for path in paths {
        h.update(
            path.strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .as_bytes(),
        );
        hash_file(&mut h, &path)?;
    }
    for env in &suite.adapter.environment {
        h.update(env.as_bytes());
        h.update([0]);
        h.update(std::env::var(env).unwrap_or_default().as_bytes());
        h.update([0]);
    }
    Ok(hex::encode(h.finalize()))
}

pub fn validate_run_id(id: &str) -> Result<()> {
    ensure!(
        !id.is_empty()
            && id.len() <= 100
            && id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
        "run ID must be 1-100 letters, numbers, hyphens or underscores"
    );
    Ok(())
}

pub fn sample(seed: &str, metric: &str, index: u32, population: usize) -> Result<usize> {
    let n = u64::try_from(population)?;
    ensure!(n > 0, "empty sampling population");
    let limit = u64::MAX - (u64::MAX % n);
    for nonce in 0u32..1000 {
        let mut h = Sha256::new();
        h.update(seed);
        h.update([0]);
        h.update(metric);
        h.update(index.to_le_bytes());
        h.update(nonce.to_le_bytes());
        let digest = h.finalize();
        let value = u64::from_le_bytes(digest[..8].try_into()?);
        if value < limit {
            return Ok(usize::try_from(value % n)?);
        }
    }
    anyhow::bail!("unable to draw a sample")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_traversal_run_ids() {
        assert!(validate_run_id("../../key").is_err());
        assert!(validate_run_id("pr-123").is_ok());
    }
    #[test]
    fn sampling_is_reproducible() -> Result<()> {
        assert_eq!(sample("seed", "m", 7, 13)?, sample("seed", "m", 7, 13)?);
        Ok(())
    }
}
