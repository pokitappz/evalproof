use crate::{
    PROTOCOL,
    model::{Adapter, Case, Grade, GradeRequest, GradeResponse},
};
use anyhow::{Context, Result, ensure};
use std::{path::Path, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
};

pub struct Worker {
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
}

async fn line(reader: &mut BufReader<ChildStdout>) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    loop {
        let buf = reader.fill_buf().await?;
        ensure!(
            !buf.is_empty(),
            "adapter exited before returning a response"
        );
        let end = buf.iter().position(|b| *b == b'\n');
        let n = end.map_or(buf.len(), |i| i + 1);
        ensure!(
            out.len() + n <= 2 * 1024 * 1024,
            "adapter response exceeds 2 MiB"
        );
        out.extend_from_slice(&buf[..n]);
        reader.consume(n);
        if end.is_some() {
            return Ok(out);
        }
    }
}

impl Worker {
    pub async fn spawn(adapter: &Adapter, root: &Path, timeout: Duration) -> Result<Self> {
        let executable = crate::storage::executable(&adapter.command[0], root)?;
        let mut child = Command::new(executable)
            .args(&adapter.command[1..])
            .current_dir(root)
            .env(
                "EVALPROOF_MODE",
                match adapter.mode {
                    crate::model::Mode::Llm => "llm",
                    _ => "deterministic",
                },
            )
            .env("PROMPTFOO_DISABLE_TELEMETRY", "1")
            .env("PROMPTFOO_DISABLE_UPDATE_CHECK", "1")
            .env("PROMPTFOO_CONFIG_DIR", root.join(".evalproof/promptfoo"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .context("failed to start adapter")?;
        let input = child.stdin.take().context("missing adapter stdin")?;
        let output = BufReader::new(child.stdout.take().context("missing adapter stdout")?);
        let mut worker = Self {
            child,
            input,
            output,
        };
        let hello = tokio::time::timeout(timeout, line(&mut worker.output))
            .await
            .context("adapter startup timed out")??;
        let hello: serde_json::Value =
            serde_json::from_slice(&hello).context("adapter did not send a valid handshake")?;
        if hello["ready"] == false {
            anyhow::bail!(
                "adapter configuration: {}",
                hello["reason"].as_str().unwrap_or("initialization failed")
            );
        }
        ensure!(
            hello["version"] == PROTOCOL && hello["ready"] == true,
            "unsupported adapter handshake"
        );
        Ok(worker)
    }
    pub async fn grade(&mut self, id: usize, case: &Case, timeout: Duration) -> Result<Grade> {
        let request = GradeRequest {
            version: PROTOCOL,
            id,
            output: case.output.clone(),
            output_text: case.output_text.clone(),
            context: case.context.clone(),
        };
        let mut bytes = serde_json::to_vec(&request)?;
        ensure!(bytes.len() <= 1024 * 1024, "grading request exceeds 1 MiB");
        bytes.push(b'\n');
        tokio::time::timeout(timeout, async {
            self.input.write_all(&bytes).await?;
            self.input.flush().await?;
            let response = line(&mut self.output).await?;
            let response: GradeResponse =
                serde_json::from_slice(&response).context("invalid grading result")?;
            ensure!(
                response.version == PROTOCOL && response.id == id,
                "adapter response ID or version mismatch"
            );
            ensure!(
                response.result.reason.len() <= 16_384,
                "grader reason exceeds 16 KiB"
            );
            ensure!(
                response.result.score.is_none_or(f64::is_finite),
                "grader score must be finite"
            );
            Ok(response.result)
        })
        .await
        .context("grader timed out; no verdict recorded")?
    }
    pub async fn shutdown(mut self) -> Result<()> {
        if self.child.try_wait()?.is_none() {
            self.child.kill().await?;
        }
        self.child.wait().await?;
        Ok(())
    }
}
