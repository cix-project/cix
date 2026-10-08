//! One verified operation boundary, shared by future transports.
use super::{
    auth::{Authenticator, VerifiedCaller},
    config::{managed_limits, Config},
    contracts::*,
    engine::Engine,
    error::{Problem, Result},
    jobs::Jobs,
    objects::Objects,
    repository::Repository,
    storage::Directory,
};
use crate::managed_sdk::{AccessRequest, Identity, ResourceCaps};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde_json::{json, Value};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
pub struct Runtime {
    config: Arc<Config>,
    auth: Authenticator,
    repository: Repository,
    objects: Objects,
    engine: Engine,
    jobs: Jobs,
    // Retain the directory descriptor used by SQLite and the object store.
    _state: Directory,
}
pub(crate) fn metadata_limits(input: usize, output: usize) -> ResourceCaps {
    ResourceCaps {
        input_bytes: input.max(1) as u64,
        output_bytes: output.max(1) as u64,
        memory_bytes: input.saturating_add(output).saturating_add(4096) as u64,
        temporary_bytes: 0,
        workers: 1,
        deadline_ms: 1000,
    }
}
pub(crate) fn authorize(
    auth: &Authenticator,
    caller: &VerifiedCaller,
    action: &str,
    namespace: &str,
    collection: Option<&str>,
    profile: Option<Profile>,
    codec: Option<&str>,
    owner: Option<&Identity>,
    limits: ResourceCaps,
) -> Result<()> {
    auth.authorize(
        caller,
        &AccessRequest {
            action,
            tenant: owner.map_or(caller.identity().tenant.as_str(), |p| p.tenant.as_str()),
            namespace: Some(namespace),
            collection,
            profile: profile.map(Profile::as_str),
            codec,
            root: Some("objects"),
            job_owner: owner,
            limits,
        },
    )?;
    Ok(())
}
pub(crate) fn job_authorize(
    auth: &Authenticator,
    caller: &VerifiedCaller,
    action: &str,
    job: &Job,
    limits: ResourceCaps,
) -> Result<()> {
    let collection = match &job.request.source {
        InputRef::EntryVersion { collection_id, .. } => Some(collection_id.as_str()),
        _ => None,
    };
    // CIX is the archive format; automatic selection has not fixed a codec.
    let codec = None;
    authorize(
        auth,
        caller,
        action,
        "jobs",
        collection,
        Some(job.request.options.profile),
        codec,
        Some(&job.owner()),
        limits,
    )
}
pub(crate) async fn authorize_request(
    auth: &Authenticator,
    objects: &Objects,
    caller: &VerifiedCaller,
    request: &TransformRequest,
) -> Result<Artifact> {
    authorize_request_with_gate(auth, objects, caller, request, AdmissionGate::Ready).await
}

/// Production has only an immediately ready gate. The controllable pause is
/// compiled exclusively for unit tests, never as a transport or host API.
pub(crate) enum AdmissionGate {
    Ready,
    #[cfg(test)]
    Pause {
        entered: tokio::sync::oneshot::Sender<()>,
        release: tokio::sync::oneshot::Receiver<()>,
    },
}
impl AdmissionGate {
    async fn wait(self) -> Result<()> {
        match self {
            Self::Ready => Ok(()),
            #[cfg(test)]
            Self::Pause { entered, release } => {
                entered.send(()).map_err(|_| Problem::internal())?;
                release.await.map_err(|_| Problem::internal())
            }
        }
    }
}

/// The enclosing operation/start/publication boundary owns the reload read fence.
pub(crate) async fn authorize_request_with_gate(
    auth: &Authenticator,
    objects: &Objects,
    caller: &VerifiedCaller,
    request: &TransformRequest,
    before_lookup: AdmissionGate,
) -> Result<Artifact> {
    let owner = caller.identity();
    let caps = managed_limits(&request.limits);
    let collection = match &request.source {
        InputRef::EntryVersion { collection_id, .. } => Some(collection_id.as_str()),
        _ => None,
    };
    // Do not turn an output-format name into a fabricated codec selector.
    let codec = None;
    authorize(
        auth,
        caller,
        "jobs:write",
        "jobs",
        collection,
        Some(request.options.profile),
        codec,
        Some(owner),
        caps,
    )?;
    let action = match request.operation {
        Operation::Compress => "codec:encode",
        Operation::Decompress => "codec:decode",
        _ => "codec:inspect",
    };
    authorize(
        auth,
        caller,
        action,
        "jobs",
        collection,
        Some(request.options.profile),
        codec,
        Some(owner),
        caps,
    )?;
    let id = match &request.source {
        InputRef::Artifact { id } => id,
        _ => return Err(Problem::new("unsupported_source", 422)),
    };
    before_lookup.wait().await?;
    let stored = objects.describe(&owner.tenant, id).await?;
    authorize(
        auth,
        caller,
        "artifacts:read",
        "artifacts",
        None,
        None,
        None,
        Some(&stored.owner),
        caps,
    )
    .map_err(conceal)?;
    if stored.value.bytes.0 > request.limits.input_bytes.0 {
        return Err(Problem::limit("input_bytes"));
    }
    Ok(stored.value)
}
fn conceal(error: Problem) -> Problem {
    if error.status == 403 {
        Problem::missing()
    } else {
        error
    }
}
impl Runtime {
    pub async fn open(config: Config, migrate: bool) -> Result<Self> {
        // Complete credential and policy validation precedes all persistent effects.
        let auth = Authenticator::load(&config)?;
        let state = Directory::open_private(&config.state_directory)?;
        let database = if let Some(variable) = &config.database_url_env {
            let url = zeroize::Zeroizing::new(
                std::env::var(variable).map_err(|_| Problem::new("credential_unavailable", 503))?,
            );
            // External SQLite paths would bypass the opened state-directory boundary.
            if !url.starts_with("postgres://") && !url.starts_with("postgresql://") {
                return Err(Problem::invalid("database_url"));
            }
            url
        } else {
            {
                if let Some(url) = state.sqlite_probe_url()? {
                    Repository::preflight_legacy(&url).await?;
                }
                zeroize::Zeroizing::new(state.sqlite_url()?)
            }
        };
        let repository = Repository::connect(&database).await?;
        if migrate {
            repository.migrate().await?;
        } else {
            repository.check_schema().await?;
        }
        let objects = Objects::open(&config, repository.clone(), &state)?;
        let config = Arc::new(config.redacted());
        let engine = Engine::new(config.clone(), objects.clone(), state.clone())?;
        let jobs = Jobs::new(
            repository.clone(),
            engine.clone(),
            objects.clone(),
            auth.clone(),
            config.clone(),
        );
        Ok(Self {
            config,
            auth,
            repository,
            objects,
            engine,
            jobs,
            _state: state,
        })
    }
    pub fn authenticate(&self, header: Option<&str>) -> Result<VerifiedCaller> {
        self.auth.authenticate(header)
    }
    pub async fn authenticate_password(
        &self,
        tenant: &str,
        subject: &str,
        password: String,
    ) -> Result<VerifiedCaller> {
        self.auth
            .authenticate_password(tenant, subject, password)
            .await
    }
    /// Trusted host administration, never an operation exposed to remote callers.
    pub async fn reload_security(&self, next: &Config) -> Result<()> {
        let mut old = serde_json::to_value(self.config.as_ref())?;
        let mut new = serde_json::to_value(next.redacted())?;
        for key in [
            "credentials",
            "local_users",
            "policy",
            "stdio_tenant",
            "stdio_subject",
            "stdio_scopes",
        ] {
            old.as_object_mut()
                .ok_or_else(Problem::internal)?
                .remove(key);
            new.as_object_mut()
                .ok_or_else(Problem::internal)?
                .remove(key);
        }
        if old != new {
            return Err(Problem::invalid("restart_required_configuration"));
        }
        self.auth.reload(next).await
    }
    /// Worker lifecycle belongs to the trusted host; no raw repository is exposed.
    pub fn start_workers(&self, stop: CancellationToken) -> Vec<tokio::task::JoinHandle<()>> {
        self.jobs.start(stop)
    }
    /// The host must stop and join workers before closing metadata. Awaiting
    /// closure keeps SQLite's directory descriptor alive through final I/O.
    pub async fn close(&self) {
        self.repository.close().await;
    }
    fn capabilities(&self) -> Value {
        json!({"api_version":"1","metadata_schema":"2","build_version":env!("CARGO_PKG_VERSION"),"default_profile":"fast","limits":self.config.limits,"full_engine_configured":self.engine.available(),"worker_protocol_version":crate::full_engine::job_protocol::JOB_PROTOCOL_VERSION,"native_sdk_process_free":true,"service_worker_isolated":true,"hard_process_rss_limit":false,"job_store":["sqlite","postgresql"],"object_store":["local"],"operations":super::schemas::operations().iter().map(|o|o.name).collect::<Vec<_>>()})
    }
    pub async fn call(&self, caller: &VerifiedCaller, name: &str, args: Value) -> Result<Value> {
        let _operation = self.auth.operation().await;
        self.auth.queued(caller)?;
        let input = serde_json::to_vec(&args)?.len();
        if input > self.config.max_json_bytes {
            return Err(Problem::limit("json_bytes"));
        }
        let operation = super::schemas::operations()
            .into_iter()
            .find(|o| o.name == name)
            .ok_or_else(|| Problem::new("unknown_operation", 404))?;
        let caps = metadata_limits(input, self.config.max_json_bytes);
        caps.within(managed_limits(&self.config.limits))
            .map_err(super::config::policy_problem)?;
        let tenant = &caller.identity().tenant;
        let value = match name {
            "cix_capabilities" => {
                authorize(
                    &self.auth,
                    caller,
                    operation.scope,
                    "service",
                    None,
                    None,
                    None,
                    None,
                    caps,
                )?;
                self.capabilities()
            }
            "cix_jobs_submit" => serde_json::to_value(
                self.jobs
                    .submit(caller, serde_json::from_value(args)?)
                    .await?,
            )?,
            "cix_jobs_get" | "cix_jobs_events" => {
                let job = self.repository.job(tenant, string(&args, "id")?).await?;
                job_authorize(&self.auth, caller, operation.scope, &job, caps).map_err(conceal)?;
                if name == "cix_jobs_get" {
                    serde_json::to_value(job)?
                } else {
                    serde_json::to_value(self.repository.events(&job, cursor(&args)?).await?)?
                }
            }
            "cix_jobs_cancel" => {
                let id = string(&args, "id")?;
                let mut answer = None;
                for _ in 0..8 {
                    let mut job = self.repository.job(tenant, id).await?;
                    job_authorize(&self.auth, caller, operation.scope, &job, caps)
                        .map_err(conceal)?;
                    if job.state.terminal() || job.state == JobState::Cancelling {
                        answer = Some(job);
                        break;
                    }
                    let old = job.revision.0;
                    job.revision.0 += 1;
                    job.updated_ms = Count(now_ms());
                    job.state = if job.state == JobState::Queued {
                        JobState::Cancelled
                    } else {
                        JobState::Cancelling
                    };
                    if self.repository.update_job(&job, old, None).await? {
                        answer = Some(job);
                        break;
                    }
                }
                serde_json::to_value(answer.ok_or_else(Problem::conflict)?)?
            }
            "cix_jobs_list" => {
                let mut page = self.repository.jobs(tenant, cursor(&args)?).await?;
                page.items.retain(|job| {
                    job_authorize(&self.auth, caller, operation.scope, job, caps).is_ok()
                });
                serde_json::to_value(page)?
            }
            "cix_artifacts_get" => {
                let stored = self.objects.describe(tenant, string(&args, "id")?).await?;
                authorize(
                    &self.auth,
                    caller,
                    operation.scope,
                    "artifacts",
                    None,
                    None,
                    None,
                    Some(&stored.owner),
                    caps,
                )
                .map_err(conceal)?;
                serde_json::to_value(stored.value)?
            }
            "cix_artifacts_upload" => {
                only_fields(&args, &["base64"])?;
                let encoded = string(&args, "base64")?;
                if encoded.len() > self.config.max_inline_bytes.div_ceil(3) * 4 {
                    return Err(Problem::limit("inline_bytes"));
                }
                authorize(
                    &self.auth,
                    caller,
                    operation.scope,
                    "artifacts",
                    None,
                    None,
                    None,
                    Some(caller.identity()),
                    caps,
                )?;
                let bytes = STANDARD
                    .decode(encoded)
                    .map_err(|_| Problem::invalid("base64"))?;
                if bytes.len() > self.config.max_inline_bytes {
                    return Err(Problem::limit("inline_bytes"));
                }
                serde_json::to_value(
                    self.objects
                        .put(caller.identity(), &bytes, "application/octet-stream")
                        .await?,
                )?
            }
            "cix_collections_create" => {
                let input: CreateCollection = serde_json::from_value(args)?;
                if input.name.is_empty() || input.name.len() > 256 {
                    return Err(Problem::invalid("name"));
                }
                let collection = Collection {
                    id: new_id(),
                    name: input.name,
                    revision: Count(1),
                    created_ms: Count(now_ms()),
                    profile: input.profile,
                };
                authorize(
                    &self.auth,
                    caller,
                    operation.scope,
                    "collections",
                    Some(&collection.id),
                    Some(collection.profile),
                    None,
                    Some(caller.identity()),
                    caps,
                )?;
                if serde_json::to_vec(&collection)?.len() > self.config.max_json_bytes {
                    return Err(Problem::limit("json_bytes"));
                }
                self.repository
                    .create(
                        caller.identity(),
                        "collection",
                        &collection.id,
                        "",
                        &collection,
                    )
                    .await?;
                serde_json::to_value(collection)?
            }
            "cix_collections_get" => {
                let stored = self
                    .repository
                    .get::<Collection>(tenant, "collection", string(&args, "id")?)
                    .await?;
                authorize(
                    &self.auth,
                    caller,
                    operation.scope,
                    "collections",
                    Some(&stored.value.id),
                    Some(stored.value.profile),
                    None,
                    Some(&stored.owner),
                    caps,
                )
                .map_err(conceal)?;
                serde_json::to_value(stored.value)?
            }
            "cix_collections_list" => {
                let page = self
                    .repository
                    .list::<Collection>(tenant, "collection", cursor(&args)?)
                    .await?;
                let items = page
                    .items
                    .into_iter()
                    .filter(|s| {
                        authorize(
                            &self.auth,
                            caller,
                            operation.scope,
                            "collections",
                            Some(&s.value.id),
                            Some(s.value.profile),
                            None,
                            Some(&s.owner),
                            caps,
                        )
                        .is_ok()
                    })
                    .map(|s| s.value)
                    .collect::<Vec<_>>();
                serde_json::to_value(Page {
                    items,
                    next_cursor: page.next_cursor,
                })?
            }
            _ => return Err(Problem::new("unknown_operation", 404)),
        };
        if serde_json::to_vec(&value)?.len() > self.config.max_json_bytes {
            return Err(Problem::limit("json_bytes"));
        }
        self.auth.queued(caller)?;
        Ok(value)
    }
}
fn string<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| Problem::invalid(key))
}
fn cursor(value: &Value) -> Result<Option<&str>> {
    only_fields(value, &["id", "cursor"])?;
    match value.get("cursor") {
        None => Ok(None),
        Some(Value::String(s)) => Ok(Some(s)),
        _ => Err(Problem::invalid("cursor")),
    }
}
fn only_fields(value: &Value, fields: &[&str]) -> Result<()> {
    let object = value.as_object().ok_or_else(|| Problem::invalid("json"))?;
    if object.keys().any(|key| !fields.contains(&key.as_str())) {
        return Err(Problem::invalid("unknown_field"));
    }
    Ok(())
}
