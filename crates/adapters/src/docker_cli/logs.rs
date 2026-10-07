//! Read-only streaming uses its own process group and never holds the Runner CLI gate.
use super::*;
use composenest_application::logs::{LogBuffer, LogDecoder};
use tokio::sync::watch;

/// Cancels only the log CLI when dropped; the supervisor remains alive to reap it.
pub struct LogSubscription {
    cancel: watch::Sender<bool>,
    buffer: Arc<StdMutex<LogBuffer>>,
    termination_confirmed: Arc<AtomicBool>,
    task: Option<tokio::task::JoinHandle<Result<(), CliError>>>,
}
impl LogSubscription {
    /// Signals read-only cancellation while retaining the handle for shutdown confirmation.
    pub fn cancel(&self) {
        let _ = self.cancel.send(true);
    }
    /// Reads bounded masked state without retaining a queue for slow consumers.
    pub fn snapshot<T>(&self, read: impl FnOnce(&LogBuffer) -> T) -> T {
        read(&self.buffer.lock().unwrap_or_else(|p| p.into_inner()))
    }
    /// Reports confirmed cleanup separately from a supervisor that ended with an error.
    pub fn termination_confirmed(&self) -> bool {
        self.termination_confirmed.load(Ordering::Acquire)
    }
    /// Confirms termination within the supervisor's fixed cleanup deadline.
    /// An unconfirmed result stays unconfirmed on every subsequent call.
    pub async fn close(&mut self) -> Result<(), CliError> {
        self.cancel();
        if let Some(task) = self.task.as_mut() {
            let result = task
                .await
                .map_err(|_| CliError::TerminationUnconfirmed("log supervisor"))
                .and_then(|result| result);
            self.task.take();
            match result {
                // A failed read still has confirmed cleanup; it remains visible in the snapshot.
                Err(CliError::InvalidConfiguration("log read failed")) => Ok(()),
                result => result,
            }
        } else if self.termination_confirmed() {
            Ok(())
        } else {
            Err(CliError::TerminationUnconfirmed("log supervisor"))
        }
    }
    #[cfg(test)]
    pub(crate) fn unconfirmed_for_test() -> Self {
        let (cancel, _) = watch::channel(false);
        let mut buffer = LogBuffer::default();
        buffer.finished = true;
        buffer.failed = true;
        Self {
            cancel,
            buffer: Arc::new(StdMutex::new(buffer)),
            termination_confirmed: Arc::new(AtomicBool::new(false)),
            task: Some(tokio::spawn(async {
                Err(CliError::TerminationUnconfirmed("test"))
            })),
        }
    }
}
impl Drop for LogSubscription {
    fn drop(&mut self) {
        let _ = self.cancel.send(true);
    }
}
impl DockerCli {
    /// Follows one immutable container ID with known secrets held only in the backend.
    pub fn follow_logs(
        &self,
        container_id: &str,
        secrets: &[String],
    ) -> Result<LogSubscription, CliError> {
        if container_id.len() != 64 || !container_id.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(CliError::InvalidConfiguration("invalid log container ID"));
        }
        let out = LogDecoder::new(secrets).map_err(CliError::InvalidConfiguration)?;
        let err = LogDecoder::new(secrets).map_err(CliError::InvalidConfiguration)?;
        let (cancel, receiver) = watch::channel(false);
        let buffer = Arc::new(StdMutex::new(LogBuffer::default()));
        let output = Arc::clone(&buffer);
        let termination_confirmed = Arc::new(AtomicBool::new(false));
        let confirmation = Arc::clone(&termination_confirmed);
        let cli = self.clone();
        let id = container_id.to_owned();
        let task = tokio::spawn(async move {
            let result = cli
                .stream_logs(&id, receiver, out, err, Arc::clone(&output))
                .await;
            confirmation.store(
                matches!(
                    &result,
                    Ok(()) | Err(CliError::InvalidConfiguration("log read failed"))
                ),
                Ordering::Release,
            );
            let mut buffer = output.lock().unwrap_or_else(|p| p.into_inner());
            buffer.finished = true;
            buffer.failed = result.is_err();
            result
        });
        Ok(LogSubscription {
            cancel,
            buffer,
            termination_confirmed,
            task: Some(task),
        })
    }
    async fn stream_logs(
        &self,
        id: &str,
        mut cancel: watch::Receiver<bool>,
        out: LogDecoder,
        err: LogDecoder,
        buffer: Arc<StdMutex<LogBuffer>>,
    ) -> Result<(), CliError> {
        if *cancel.borrow() {
            return Ok(());
        }
        let args = [
            "logs".into(),
            "--follow".into(),
            "--timestamps".into(),
            "--tail".into(),
            "2000".into(),
            id.into(),
        ];
        let mut child = self.command(&args, true)?.spawn()?;
        let pid = child
            .id()
            .ok_or(CliError::InvalidConfiguration("missing log PID"))?;
        let group = match ProcessGroup::attach(pid) {
            Ok(group) => group,
            Err(error) => {
                let _ = child.kill().await;
                return Err(error);
            }
        };
        let stdout = child
            .stdout
            .take()
            .ok_or(CliError::InvalidConfiguration("missing stdout"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or(CliError::InvalidConfiguration("missing stderr"))?;
        let mut out_task = tokio::spawn(drain_logs(stdout, out, Arc::clone(&buffer)));
        let mut err_task = tokio::spawn(drain_logs(stderr, err, buffer));
        let ended = tokio::select! {
            result = child.wait() => Some(result),
            _ = cancel.changed() => None,
        };
        // Terminate this read-only group on every path, including a child that left pipes open.
        let terminated = group.terminate();
        let reaped = timeout(REAP_TIMEOUT, child.wait()).await;
        let drained = timeout(REAP_TIMEOUT, async {
            tokio::join!(&mut out_task, &mut err_task)
        })
        .await;
        if drained.is_err() {
            out_task.abort();
            err_task.abort();
        }
        terminated?;
        if !matches!(reaped, Ok(Ok(_))) || !matches!(drained, Ok((Ok(Ok(())), Ok(Ok(()))))) {
            return Err(CliError::TerminationUnconfirmed("log cleanup"));
        }
        if !matches!(
            timeout(REAP_TIMEOUT, async {
                loop {
                    if group.is_empty()? {
                        return Ok::<_, CliError>(());
                    }
                    sleep(Duration::from_millis(20)).await;
                }
            })
            .await,
            Ok(Ok(()))
        ) {
            return Err(CliError::TerminationUnconfirmed("log process group"));
        }
        if let Some(result) = ended
            && !result?.success()
        {
            return Err(CliError::InvalidConfiguration("log read failed"));
        }
        Ok(())
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[tokio::test]
    async fn repeated_close_preserves_unconfirmed_cleanup() {
        let mut logs = LogSubscription::unconfirmed_for_test();
        assert!(logs.close().await.is_err());
        assert!(logs.close().await.is_err());
        assert!(!logs.termination_confirmed());
    }
    #[tokio::test]
    async fn flood_is_masked_bounded_and_never_blocks_changes_or_stops_container() {
        let root = tempfile::tempdir().unwrap();
        let executable = root.path().join("docker");
        let marker = root.path().join("log-pid");
        std::fs::write(
            &executable,
            format!(
                r#"#!/bin/sh
if [ "$3" = logs ]; then
  echo $$ > '{}'
  i=0
  while [ "$i" -lt 4000 ]; do printf 'secret-value line %s\n' "$i"; i=$((i+1)); done
  printf 'secret-value stderr\n' >&2
  exec /bin/sleep 60
fi
if [ "$3" != version ]; then exit 9; fi
i=0
while [ "$i" -lt 7000 ]; do printf 'diagnostic-line\n'; i=$((i+1)); done
"#,
                marker.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        let cli = DockerCli::new(
            executable,
            root.path().into(),
            root.path().into(),
            "unix:///tmp/log-test.sock".into(),
        )
        .unwrap();
        let mut logs = cli
            .follow_logs(&"a".repeat(64), &["secret-value".into()])
            .unwrap();
        timeout(Duration::from_secs(5), async {
            while logs.snapshot(|b| b.dropped_lines) < 2000 {
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert!(
            cli.termination_verified(),
            "logs do not hold the changing CLI gate"
        );
        let outcome = timeout(
            Duration::from_secs(2),
            cli.run(
                CommandKind::Change,
                &["version".into()],
                Duration::from_secs(1),
            ),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(outcome.status.unwrap().success());
        assert_eq!(outcome.stdout.bytes.len(), OUTPUT_LIMIT);
        assert!(outcome.stdout.truncated);
        logs.snapshot(|b| {
            assert_eq!(b.lines().len(), 2000);
            assert!(!b.lines().join("\n").contains("secret-value"));
            assert!(!b.finished);
        });
        let pid: i32 = std::fs::read_to_string(marker)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        logs.close().await.unwrap();
        // The direct child has been reaped, not merely detached from the UI.
        assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
        assert!(cli.termination_verified());
    }
}
async fn drain_logs(
    mut stream: impl AsyncRead + Unpin,
    mut decoder: LogDecoder,
    buffer: Arc<StdMutex<LogBuffer>>,
) -> io::Result<()> {
    let mut chunk = [0_u8; 8192];
    loop {
        let count = stream.read(&mut chunk).await?;
        let bytes = chunk[..count].to_vec();
        let output = Arc::clone(&buffer);
        // At most one bounded chunk per pipe reaches the CPU worker, never an unbounded queue.
        decoder = tokio::task::spawn_blocking(move || {
            decoder.feed(
                &bytes,
                count == 0,
                &mut output.lock().unwrap_or_else(|p| p.into_inner()),
            );
            decoder
        })
        .await
        .map_err(|_| io::Error::other("log masking worker failed"))?;
        if count == 0 {
            return Ok(());
        }
        tokio::task::yield_now().await;
    }
}
