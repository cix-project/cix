//! Leased execution with current authentication at start and before publication.
use super::{
    auth::{Authenticator, VerifiedCaller},
    config::Config,
    contracts::*,
    engine::{Engine, Output},
    error::{Problem, Result},
    objects::Objects,
    repository::Repository,
    runtime::{authorize_request, authorize_request_with_gate, AdmissionGate},
};
use std::{sync::Arc, time::Duration};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
pub(crate) struct Jobs {
    repository: Repository,
    engine: Engine,
    objects: Objects,
    auth: Authenticator,
    config: Arc<Config>,
}
impl Jobs {
    pub(crate) fn new(
        repository: Repository,
        engine: Engine,
        objects: Objects,
        auth: Authenticator,
        config: Arc<Config>,
    ) -> Self {
        Self {
            repository,
            engine,
            objects,
            auth,
            config,
        }
    }
    pub(crate) async fn submit(
        &self,
        caller: &VerifiedCaller,
        request: TransformRequest,
    ) -> Result<Job> {
        // Resource/unsupported-option checks precede provider or object effects.
        self.engine.validate(&request)?;
        authorize_request(&self.auth, &self.objects, caller, &request).await?;
        let proof = self.auth.queued(caller)?;
        self.repository
            .submit(
                &proof,
                request,
                self.config.max_queued_jobs,
                self.config.max_json_bytes,
            )
            .await
    }
    pub(crate) fn start(&self, stop: CancellationToken) -> Vec<tokio::task::JoinHandle<()>> {
        (0..self.config.max_active_jobs).map(|_|{
            let this=self.clone();let stop=stop.clone();
            tokio::spawn(async move {let worker=new_id();while !stop.is_cancelled() {
                match this.repository.claim(&worker,this.config.worker_lease_seconds*1000).await {
                    Ok(Some(job))=>{let _=this.run(job,&worker,&stop).await;},
                    _=>tokio::select! {_=stop.cancelled()=>{},_=tokio::time::sleep(Duration::from_millis(250))=>{}},
                }
            }})
        }).collect()
    }
    async fn execute(&self, job: &Job, worker: &str, stop: &CancellationToken) -> Result<Output> {
        self.execute_with_start_gate(job, worker, stop, AdmissionGate::Ready)
            .await
    }
    async fn execute_with_start_gate(
        &self,
        job: &Job,
        worker: &str,
        stop: &CancellationToken,
        before_lookup: AdmissionGate,
    ) -> Result<Output> {
        // This short start decision is coherent across proof, credential, policy
        // and awaited metadata checks. A successful decision admits this run;
        // later reloads still govern cancellation and final publication.
        let (proof, artifact) = {
            let _admission = self.auth.operation().await;
            let proof = self.repository.authority(job).await?;
            let caller = self.auth.resume(&proof)?;
            let artifact = authorize_request_with_gate(
                &self.auth,
                &self.objects,
                &caller,
                &job.request,
                before_lookup,
            )
            .await?;
            (proof, artifact)
        }; // Release before creating/polling the long-running engine future.
        if job.attempt > self.config.job_attempts {
            return Err(Problem::new("attempts_exhausted", 503));
        }
        let (cancel_tx, cancel_rx) = watch::channel(job.state == JobState::Cancelling);
        let future = self.engine.execute(job, &artifact, cancel_rx);
        tokio::pin!(future);
        let mut interval = tokio::time::interval(Duration::from_millis(250));
        let mut last_heartbeat = tokio::time::Instant::now();
        let mut abort = None;
        loop {
            tokio::select! {
                output=&mut future=>break match abort {Some(error)=>Err(error),None=>output},
                _=stop.cancelled(),if abort.is_none()=>{let _=cancel_tx.send(true);abort=Some(Problem::new("worker_stopping",503));},
                _=interval.tick(),if abort.is_none()=>{
                    let checked=async {
                        let fresh=self.repository.job(&job.tenant,&job.id).await?;
                        if fresh.state==JobState::Cancelling {return Err(Problem::new("cancelled",409));}
                        self.auth.resume(&proof)?;
                        if last_heartbeat.elapsed()>=Duration::from_secs(self.config.worker_lease_seconds/3) {
                            if !self.repository.heartbeat(job,worker,self.config.worker_lease_seconds*1000).await? {return Err(Problem::new("lease_lost",409));}
                            last_heartbeat=tokio::time::Instant::now();
                        }Result::<()>::Ok(())
                    }.await;
                    if let Err(error)=checked {let _=cancel_tx.send(true);abort=Some(error);}
                }
            }
        }
    }
    async fn run(&self, job: Job, worker: &str, stop: &CancellationToken) -> Result<()> {
        let result = self.execute(&job, worker, stop).await;
        self.finish(job, worker, stop, result).await
    }
    async fn finish(
        &self,
        job: Job,
        worker: &str,
        stop: &CancellationToken,
        mut result: Result<Output>,
    ) -> Result<()> {
        let mut staged = None;
        for _ in 0..8 {
            let _publication = self.auth.operation().await;
            let mut fresh = self.repository.job(&job.tenant, &job.id).await?;
            if fresh.state.terminal() {
                break;
            }
            if stop.is_cancelled() {
                break;
            }
            let previous = fresh.revision.0;
            fresh.revision.0 += 1;
            fresh.updated_ms = Count(now_ms());
            if fresh.state == JobState::Cancelling {
                fresh.state = JobState::Cancelled;
                fresh.result = None;
                fresh.error = None;
            } else {
                // Read the private proof and current policy again immediately before
                // staging/publishing. Job JSON cannot supply this authority.
                let approval = async {
                    let proof = self.repository.authority(&fresh).await?;
                    let caller = self.auth.resume(&proof)?;
                    authorize_request(&self.auth, &self.objects, &caller, &fresh.request).await?;
                    Ok::<_, Problem>(caller)
                }
                .await;
                if let Err(error) = approval {
                    result = Err(error);
                }
                match &result {
                    Ok(output) => {
                        if staged.is_none() {
                            if let Some(bytes) = &output.bytes {
                                staged = Some(
                                    self.objects
                                        .stage(
                                            &fresh.owner(),
                                            bytes,
                                            if fresh.request.operation == Operation::Compress {
                                                "application/vnd.cix"
                                            } else {
                                                "application/octet-stream"
                                            },
                                        )
                                        .await?,
                                );
                            }
                        }
                        // The fence orders reloads; expiry can still advance during staging.
                        // Recheck immediately before the publication transaction.
                        let proof = self.repository.authority(&fresh).await?;
                        let recheck = async {
                            let caller = self.auth.resume(&proof)?;
                            authorize_request(&self.auth, &self.objects, &caller, &fresh.request)
                                .await?;
                            Result::<()>::Ok(())
                        }
                        .await;
                        if let Err(error) = recheck {
                            fresh.state = JobState::Failed;
                            fresh.error = Some(error);
                            fresh.result = None;
                        } else {
                            fresh.state = JobState::Succeeded;
                            fresh.result = Some(JobResult {
                                artifact: staged.as_ref().map(|s| s.artifact().clone()),
                                report: output.report.clone(),
                                applied_options: output.options.clone(),
                            });
                        }
                    }
                    Err(error) => {
                        fresh.state = if error.code == "cancelled" {
                            JobState::Cancelled
                        } else {
                            JobState::Failed
                        };
                        fresh.error = Some(error.clone());
                        fresh.result = None;
                    }
                }
            }
            if serde_json::to_vec(&fresh)?.len() > self.config.max_json_bytes {
                fresh.state = JobState::Failed;
                fresh.result = None;
                fresh.error = Some(Problem::limit("json_bytes"));
            }
            // Once commit starts its acknowledgement can be lost. Never delete
            // a file potentially referenced by a committed job transaction.
            let attempted = if fresh.state == JobState::Succeeded {
                staged.take().map(|value| value.commit())
            } else {
                None
            };
            let updated = self
                .repository
                .update_job(&fresh, previous, Some(worker))
                .await?;
            if updated {
                return Ok(());
            }
            // A definite revision/lease miss performs no metadata publication.
            if let Some(artifact) = attempted {
                self.objects.discard(&fresh.owner(), &artifact)?;
            }
        }
        if stop.is_cancelled() {
            Ok(())
        } else {
            Err(Problem::conflict())
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use crate::service::storage::Directory;
    struct Fixture {
        root: std::path::PathBuf,
        config: Config,
        jobs: Jobs,
        caller: VerifiedCaller,
    }
    impl Fixture {
        async fn new() -> Self {
            let root = std::env::temp_dir().join(format!("cix-queued-{}", new_id()));
            let mut config = Config::default();
            config.state_directory = root.clone();
            config.private_library_directory = Some(root.join("providers"));
            let auth = Authenticator::load(&config).unwrap();
            let caller = auth.local_host().unwrap();
            let state = Directory::open_private(&root).unwrap();
            state.child("providers", true).unwrap();
            let repository = Repository::connect("sqlite::memory:").await.unwrap();
            repository.migrate().await.unwrap();
            let objects = Objects::open(&config, repository.clone(), &state).unwrap();
            let shared = Arc::new(config.clone());
            let engine = Engine::new(shared.clone(), objects.clone(), state).unwrap();
            let jobs = Jobs::new(repository, engine, objects, auth, shared);
            Self {
                root,
                config,
                jobs,
                caller,
            }
        }
        async fn submit(&self) -> Job {
            let source = self
                .jobs
                .objects
                .put(
                    self.caller.identity(),
                    b"immutable input",
                    "application/octet-stream",
                )
                .await
                .unwrap();
            self.jobs
                .submit(
                    &self.caller,
                    TransformRequest {
                        operation: Operation::Compress,
                        source: InputRef::Artifact { id: source.id },
                        options: CompressionOptions::default(),
                        limits: ResourceLimits::default(),
                        idempotency_key: new_id(),
                    },
                )
                .await
                .unwrap()
        }
        async fn artifact_count(&self) -> usize {
            self.jobs
                .repository
                .list::<Artifact>(&self.caller.identity().tenant, "artifact", None)
                .await
                .unwrap()
                .items
                .len()
        }
        async fn revoke(&mut self) {
            self.config.stdio_scopes.clear();
            self.jobs.auth.reload(&self.config).await.unwrap();
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }
    fn output() -> Output {
        Output {
            bytes: Some(b"new output".to_vec()),
            report: serde_json::json!({"fixture":true}),
            options: CompressionOptions::default(),
        }
    }
    #[tokio::test]
    async fn queued_start_reauthorizes_and_does_not_launch_revoked_job() {
        let mut f = Fixture::new().await;
        let submitted = f.submit().await;
        let claimed = f
            .jobs
            .repository
            .claim("worker", 30_000)
            .await
            .unwrap()
            .unwrap();
        f.revoke().await;
        f.jobs
            .run(claimed, "worker", &CancellationToken::new())
            .await
            .unwrap();
        let result = f
            .jobs
            .repository
            .job(&submitted.tenant, &submitted.id)
            .await
            .unwrap();
        assert_eq!(result.state, JobState::Failed);
        assert_eq!(result.error.unwrap().code, "forbidden");
        assert_eq!(f.artifact_count().await, 1);
        assert!(!f.root.join("workers").exists());
    }
    #[tokio::test]
    async fn revocation_after_computation_prevents_output_publication() {
        let mut f = Fixture::new().await;
        let submitted = f.submit().await;
        let claimed = f
            .jobs
            .repository
            .claim("worker", 30_000)
            .await
            .unwrap()
            .unwrap();
        f.revoke().await;
        f.jobs
            .finish(claimed, "worker", &CancellationToken::new(), Ok(output()))
            .await
            .unwrap();
        let result = f
            .jobs
            .repository
            .job(&submitted.tenant, &submitted.id)
            .await
            .unwrap();
        assert_eq!(result.state, JobState::Failed);
        assert!(result.result.is_none());
        assert_eq!(f.artifact_count().await, 1);
    }
    #[tokio::test]
    async fn accepted_cancellation_wins_and_success_publishes_owned_metadata() {
        let f = Fixture::new().await;
        let submitted = f.submit().await;
        let claimed = f
            .jobs
            .repository
            .claim("worker", 30_000)
            .await
            .unwrap()
            .unwrap();
        let mut cancel = claimed.clone();
        let old = cancel.revision.0;
        cancel.revision.0 += 1;
        cancel.state = JobState::Cancelling;
        assert!(f
            .jobs
            .repository
            .update_job(&cancel, old, None)
            .await
            .unwrap());
        f.jobs
            .finish(claimed, "worker", &CancellationToken::new(), Ok(output()))
            .await
            .unwrap();
        assert_eq!(
            f.jobs
                .repository
                .job(&submitted.tenant, &submitted.id)
                .await
                .unwrap()
                .state,
            JobState::Cancelled
        );
        assert_eq!(f.artifact_count().await, 1);
        let next = f.submit().await;
        let claimed = f
            .jobs
            .repository
            .claim("worker", 30_000)
            .await
            .unwrap()
            .unwrap();
        f.jobs
            .finish(claimed, "worker", &CancellationToken::new(), Ok(output()))
            .await
            .unwrap();
        let completed = f.jobs.repository.job(&next.tenant, &next.id).await.unwrap();
        assert_eq!(completed.state, JobState::Succeeded);
        let artifact = completed.result.unwrap().artifact.unwrap();
        let stored = f
            .jobs
            .objects
            .describe(&next.tenant, &artifact.id)
            .await
            .unwrap();
        assert_eq!(stored.owner, f.caller.identity().clone());
    }
    #[tokio::test]
    async fn direct_zero_temp_execution_rejects_before_storage_or_provider_access() {
        let f = Fixture::new().await;
        let mut job = f.submit().await;
        job.request.limits.temporary_bytes = Count(0);
        let artifact = Artifact {
            id: new_id(),
            bytes: Count(1),
            sha256: "unused".into(),
            media_type: "application/octet-stream".into(),
            created_ms: Count(0),
            expires_ms: None,
        };
        let (_send, receive) = watch::channel(false);
        let error = f
            .jobs
            .engine
            .execute(&job, &artifact, receive)
            .await
            .err()
            .unwrap();
        assert_eq!(error.code, "resource_limit");
        assert_eq!(error.arguments["resource"], "temporary_bytes");
        assert!(!f.root.join("workers").exists());
    }
    #[tokio::test]
    async fn queued_start_cannot_combine_incompatible_reload_grants() {
        use std::{
            future::{poll_fn, Future},
            task::Poll,
        };
        let mut fixture = Fixture::new().await;
        let submitted = fixture.submit().await;
        let claimed = fixture
            .jobs
            .repository
            .claim("worker", 30_000)
            .await
            .unwrap()
            .unwrap();

        // Same credential in both snapshots. A permits the first two real
        // actions; B permits only the final real artifact read. Neither is a
        // valid complete admission for this queued job.
        fixture.config.stdio_scopes = vec!["jobs:write".into(), "codec:encode".into()];
        fixture.jobs.auth.reload(&fixture.config).await.unwrap();
        let mut policy_b = fixture.config.clone();
        policy_b.stdio_scopes = vec!["artifacts:read".into()];

        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        let jobs = fixture.jobs.clone();
        let running_job = claimed.clone();
        let execution = tokio::spawn(async move {
            jobs.execute_with_start_gate(
                &running_job,
                "worker",
                &CancellationToken::new(),
                AdmissionGate::Pause {
                    entered: entered_tx,
                    release: release_rx,
                },
            )
            .await
        });
        entered_rx.await.unwrap();
        let mut reload = Box::pin(fixture.jobs.auth.reload(&policy_b));
        let pending =
            poll_fn(|cx| Poll::Ready(matches!(reload.as_mut().poll(cx), Poll::Pending))).await;
        assert!(
            pending,
            "reload must wait for the complete queued-start decision"
        );
        release_tx.send(()).unwrap();
        let error = execution
            .await
            .unwrap()
            .err()
            .expect("A must deny artifact read");
        assert_eq!(error.code, "not_found");
        reload.await.unwrap();

        // Drive the ordinary queued path under B as well: it must reject the
        // missing jobs action, before any worker directory or output is made.
        let error = fixture
            .jobs
            .execute(&claimed, "worker", &CancellationToken::new())
            .await
            .err()
            .expect("B must deny the jobs action");
        assert_eq!(error.code, "forbidden");
        assert!(!fixture.root.join("workers").exists());
        assert_eq!(fixture.artifact_count().await, 1);
        let stored = fixture
            .jobs
            .repository
            .job(&submitted.tenant, &submitted.id)
            .await
            .unwrap();
        assert_eq!(stored.state, JobState::Running);
        assert!(stored.result.is_none());
    }
}
