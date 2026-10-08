//! Same-executable full-engine isolation; no executable or provider path from a request.
use super::{
    config::Config,
    contracts::*,
    error::{Problem, Result},
    objects::Objects,
    storage::Directory,
};
use crate::full_engine::job_protocol::{
    self, ControlRecord, JobLimits, JobOperation, JobProfile, JobRequest,
};
use serde_json::{json, Value};
use std::{path::PathBuf, process::Stdio, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::watch,
};

#[derive(Clone)]
pub(crate) struct Engine {
    config: Arc<Config>,
    objects: Objects,
    executable: PathBuf,
    state: Directory,
}
pub(crate) struct Output {
    pub bytes: Option<Vec<u8>>,
    pub report: Value,
    pub options: CompressionOptions,
}
impl Engine {
    pub(crate) fn new(config: Arc<Config>, objects: Objects, state: Directory) -> Result<Self> {
        Ok(Self {
            config,
            objects,
            state,
            executable: std::env::current_exe()?,
        })
    }
    pub(crate) fn available(&self) -> bool {
        self.config
            .private_library_directory
            .as_ref()
            .is_some_and(|p| p.is_dir())
    }
    pub(crate) fn validate(&self, request: &TransformRequest) -> Result<()> {
        request.limits.within(&self.config.limits)?;
        // This worker always creates temporary files. Refuse before queue
        // admission, provider lookup, object access or process/filesystem effects.
        if request.limits.temporary_bytes.0 == 0 {
            return Err(Problem::limit("temporary_bytes"));
        }
        let o = &request.options;
        // Until mapped to the full-engine protocol, expert controls must not be silently ignored.
        if o.output_format.as_deref().is_some_and(|f| f != "cix")
            || o.container.is_some()
            || o.route.is_some()
            || o.backend.is_some()
            || o.dictionary_id.is_some()
            || o.block_bytes.is_some()
            || o.history_bytes.is_some()
            || o.independent_blocks.is_some()
            || o.flush_interval_ms.is_some()
            || o.parallelism.is_some()
        {
            return Err(Problem::new("unsupported_options", 422));
        }
        if !self.available() {
            return Err(Problem::new("providers_not_configured", 503));
        }
        Ok(())
    }
    pub(crate) async fn execute(
        &self,
        job: &Job,
        input_artifact: &Artifact,
        mut cancel: watch::Receiver<bool>,
    ) -> Result<Output> {
        self.validate(&job.request)?;
        if input_artifact.bytes.0 > job.request.limits.input_bytes.0 {
            return Err(Problem::limit("input_bytes"));
        }
        if input_artifact.bytes.0 >= job.request.limits.temporary_bytes.0 {
            return Err(Problem::limit("temporary_bytes"));
        }
        if input_artifact.bytes.0 > job.request.limits.memory_bytes.0 {
            return Err(Problem::limit("memory_bytes"));
        }
        if *cancel.borrow() {
            return Err(Problem::new("cancelled", 409));
        }
        // Hash the opened immutable source, then stage those exact bytes. The worker
        // receives a parent-owned directory descriptor path, never an object path.
        let bytes = self
            .objects
            .read_known(
                &job.tenant,
                input_artifact,
                job.request.limits.input_bytes.usize()?,
            )
            .await?;
        let workers = self.state.child("workers", true)?;
        let id = new_id();
        let temporary = workers.child(&id, true)?;
        let result = async {
            let mut staged = tokio::fs::File::from_std(temporary.create_file("input")?);
            staged.write_all(&bytes).await?;
            staged.sync_all().await?;
            drop(staged);
            drop(bytes);
            temporary.sync()?;
            self.execute_at(job, &temporary, input_artifact.bytes.0, &mut cancel)
                .await
        }
        .await;
        let cleanup = workers.remove_child(&id);
        match (result, cleanup) {
            (Ok(output), Ok(())) => Ok(output),
            (Err(error), _) => Err(error),
            (_, Err(error)) => Err(error),
        }
    }
    async fn execute_at(
        &self,
        job: &Job,
        directory: &Directory,
        staged_bytes: u64,
        cancel: &mut watch::Receiver<bool>,
    ) -> Result<Output> {
        let temporary = directory.access_path()?;
        let input = temporary.join("input");
        if *cancel.borrow() {
            return Err(Problem::new("cancelled", 409));
        }
        let options = &job.request.options;
        let limits = &job.request.limits;
        let output = temporary.join("result");
        let request = JobRequest {
            request_id: 1,
            operation: match job.request.operation {
                Operation::Compress if options.streaming => JobOperation::SelectIndependentWindows,
                Operation::Compress => JobOperation::SelectWholeInput,
                _ => JobOperation::DecodeArchive,
            },
            profile: match options.profile {
                Profile::Fast => JobProfile::Fast,
                Profile::Default => JobProfile::Default,
                Profile::Best => JobProfile::Best,
            },
            limits: JobLimits {
                input_bytes: limits.input_bytes.usize()?,
                archive_bytes: limits.output_bytes.usize()?,
                output_bytes: limits.output_bytes.usize()?,
                memory_bytes: limits.memory_bytes.usize()?,
                intermediate_bytes: limits.output_bytes.usize()?,
                temporary_bytes: usize::try_from(limits.temporary_bytes.0 - staged_bytes)
                    .map_err(|_| Problem::limit("temporary_bytes"))?,
                workers: limits.workers as usize,
                deadline_millis: Some(limits.deadline_ms.0),
            },
            window_bytes: if options.streaming {
                Some(1 << 20)
            } else {
                None
            },
            diagnostics: true,
        };
        let mut wire = Vec::new();
        job_protocol::write_control(&mut wire, &ControlRecord::Request(request))
            .map_err(|_| Problem::invalid("worker_request"))?;
        let mut command = tokio::process::Command::new(&self.executable);
        command
            .args(["--cix-job-v1", "--input"])
            .arg(input)
            .arg("--output")
            .arg(&output)
            .arg("--private-library-dir")
            .arg(
                self.config
                    .private_library_directory
                    .as_ref()
                    .ok_or_else(|| Problem::new("providers_not_configured", 503))?,
            )
            .arg("--temporary-root")
            .arg(&temporary)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        // No inherited cloud/API credentials or executable search paths in a codec worker.
        command.env_clear();
        #[cfg(unix)]
        unsafe {
            // Async-signal-safe child-only setup; no parent umask change.
            command.pre_exec(|| {
                libc::umask(0o077);
                Ok(())
            });
        }
        #[cfg(windows)]
        if let Some(root) = std::env::var_os("SystemRoot") {
            command.env("SystemRoot", root);
        }
        let mut child = command.spawn()?;
        let mut stdin = child.stdin.take().ok_or_else(Problem::internal)?;
        stdin.write_all(&wire).await?;
        stdin.shutdown().await?;
        drop(stdin);
        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(Problem::internal)?
            .take((job_protocol::MAX_CONTROL_RECORD_BYTES * 18) as u64);
        let output_task = tokio::spawn(async move {
            let mut bytes = Vec::new();
            stdout.read_to_end(&mut bytes).await.map(|_| bytes)
        });
        let deadline = tokio::time::sleep(Duration::from_millis(limits.deadline_ms.0));
        tokio::pin!(deadline);
        let status = tokio::select! {
            result=child.wait()=>result.map_err(Problem::from),
            _=&mut deadline=>{let _=child.kill().await;Err(Problem::new("deadline_exceeded",408))},
            _=cancel.changed()=>{let _=child.kill().await;Err(Problem::new("cancelled",409))},
        };
        let bytes = output_task.await.map_err(|_| Problem::internal())??;
        let status = status?;
        let mut cursor = std::io::Cursor::new(&bytes);
        let mut report = None;
        while (cursor.position() as usize) < bytes.len() {
            match job_protocol::read_control(&mut cursor)
                .map_err(|_| Problem::new("worker_protocol", 502))?
            {
                ControlRecord::Result(result) => {
                    report = Some(
                        json!({"output_bytes":Count(result.output_bytes as u64),"decode_path":result.decode_path,"selection":result.selection.map(|s|json!({"selected":s.selected,"profile":format!("{:?}",s.profile),"input_bytes":Count(s.input_bytes as u64),"trials":s.trials.into_iter().map(|t|json!({"id":t.id,"status":format!("{:?}",t.status),"reason":t.reason,"provider_omissions":t.provider_omissions})).collect::<Vec<Value>>()})),"windows":result.windows.map(|w|json!({"window_bytes":Count(w.window_bytes as u64),"windows":Count(w.windows),"whole_only_omissions":w.whole_only_omissions}))}),
                    );
                }
                ControlRecord::Error(error) => {
                    return Err(Problem::new("codec_failed", 422)
                        .argument("worker_code", format!("{:?}", error.code)))
                }
                ControlRecord::Progress(_) => {}
                _ => return Err(Problem::new("worker_protocol", 502)),
            }
        }
        if !status.success() {
            return Err(Problem::new("worker_failed", 502));
        }
        let report = report.ok_or_else(|| Problem::new("worker_protocol", 502))?;
        if *cancel.borrow() {
            return Err(Problem::new("cancelled", 409));
        }
        let bytes = if matches!(
            job.request.operation,
            Operation::Inspect | Operation::Verify
        ) {
            None
        } else {
            let file = directory.open_file("result")?;
            let size = file.metadata()?.len();
            if size > limits.output_bytes.0 {
                return Err(Problem::limit("output_bytes"));
            }
            if size > limits.memory_bytes.0 {
                return Err(Problem::limit("memory_bytes"));
            }
            let mut reader =
                tokio::fs::File::from_std(file).take(limits.output_bytes.0.saturating_add(1));
            let mut bytes = Vec::new();
            reader.read_to_end(&mut bytes).await?;
            if bytes.len() as u64 > limits.output_bytes.0 || bytes.len() as u64 != size {
                return Err(Problem::new("object_integrity", 422));
            }
            Some(bytes)
        };
        Ok(Output {
            bytes,
            report,
            options: options.clone(),
        })
    }
}
