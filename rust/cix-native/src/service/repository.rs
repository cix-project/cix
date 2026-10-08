//! Private durable metadata. Authorization belongs to the verified dispatcher.
use super::{
    auth::QueuedAuthority,
    contracts::*,
    error::{Problem, Result},
};
use crate::managed_sdk::Identity;
use serde::{de::DeserializeOwned, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{
    any::{AnyPoolOptions, AnyRow},
    AnyPool, Row,
};

const LEGACY_ROW_COUNTS: [&str; 4] = [
    "SELECT COUNT(*) AS n FROM cix_entities",
    "SELECT COUNT(*) AS n FROM cix_jobs",
    "SELECT COUNT(*) AS n FROM cix_events",
    "SELECT COUNT(*) AS n FROM cix_admission",
];

#[derive(Clone)]
pub(crate) struct Repository {
    pool: AnyPool,
}
pub(crate) struct Stored<T> {
    pub owner: Identity,
    pub value: T,
}
fn stored<T: DeserializeOwned>(tenant: &str, row: AnyRow) -> Result<Stored<T>> {
    Ok(Stored {
        owner: Identity {
            tenant: tenant.into(),
            provider: row.try_get("provider")?,
            subject: row.try_get("owner")?,
        },
        value: serde_json::from_str(&row.try_get::<String, _>("body")?)?,
    })
}
fn decoded_job(row: AnyRow) -> Result<Job> {
    let job: Job = serde_json::from_str(&row.try_get::<String, _>("body")?)?;
    if job.provider != row.try_get::<String, _>("provider")?
        || job.subject != row.try_get::<String, _>("owner")?
        || job.tenant != row.try_get::<String, _>("tenant")?
    {
        return Err(Problem::new("metadata_integrity", 503));
    }
    Ok(job)
}
impl Repository {
    pub(crate) async fn connect(url: &str) -> Result<Self> {
        sqlx::any::install_default_drivers();
        let sqlite = url.starts_with("sqlite:");
        let pool = AnyPoolOptions::new()
            .max_connections(if sqlite { 1 } else { 8 })
            .after_connect(move |c, _| {
                Box::pin(async move {
                    if sqlite {
                        for statement in [
                            "PRAGMA synchronous=FULL",
                            "PRAGMA busy_timeout=5000",
                            "PRAGMA foreign_keys=ON",
                        ] {
                            sqlx::query(statement).execute(&mut *c).await?;
                        }
                    }
                    Ok(())
                })
            })
            .connect(url)
            .await?;
        Ok(Self { pool })
    }
    pub(crate) async fn close(&self) {
        self.pool.close().await;
    }
    /// Called through an immutable read-only SQLite handle before any writable
    /// connection. The state directory must have exclusive service ownership.
    pub(crate) async fn preflight_legacy(url: &str) -> Result<()> {
        let repository = Self::connect(url).await?;
        let checked = async {
            let row = sqlx::query("SELECT version FROM cix_schema WHERE id=1")
                .fetch_optional(&repository.pool)
                .await?;
            if row
                .as_ref()
                .map(|r| r.try_get::<i32, _>("version"))
                .transpose()?
                == Some(1)
            {
                for statement in LEGACY_ROW_COUNTS {
                    let count: i64 = sqlx::query(statement)
                        .fetch_one(&repository.pool)
                        .await?
                        .try_get("n")?;
                    if count != 0 {
                        return Err(Problem::new("owner_provider_migration_required", 503));
                    }
                }
            }
            Result::<()>::Ok(())
        }
        .await;
        repository.pool.close().await;
        checked
    }
    /// An explicit migration only. Nonempty v1 is refused without inventing a provider.
    pub(crate) async fn migrate(&self) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("CREATE TABLE IF NOT EXISTS cix_schema (id INTEGER PRIMARY KEY, version INTEGER NOT NULL)").execute(&mut *tx).await?;
        let version = sqlx::query("SELECT version FROM cix_schema WHERE id=1")
            .fetch_optional(&mut *tx)
            .await?
            .map(|r| r.try_get::<i32, _>("version"))
            .transpose()?;
        match version {
            Some(2) => {
                tx.commit().await?;
                return self.check_schema().await;
            }
            Some(1) => {
                for statement in LEGACY_ROW_COUNTS {
                    let count: i64 = sqlx::query(statement)
                        .fetch_one(&mut *tx)
                        .await?
                        .try_get("n")?;
                    if count != 0 {
                        tx.rollback().await?;
                        return Err(Problem::new("owner_provider_migration_required", 503));
                    }
                }
                for statement in [
                    "DROP TABLE cix_entities",
                    "DROP TABLE cix_jobs",
                    "DROP TABLE cix_events",
                    "DROP TABLE cix_admission",
                ] {
                    sqlx::query(statement).execute(&mut *tx).await?;
                }
            }
            None => {}
            _ => {
                tx.rollback().await?;
                return Err(Problem::new("migration_required", 503));
            }
        }
        for statement in [
            "CREATE TABLE cix_entities (tenant TEXT NOT NULL, kind TEXT NOT NULL, id TEXT NOT NULL, provider TEXT NOT NULL, owner TEXT NOT NULL, parent TEXT NOT NULL, revision BIGINT NOT NULL, body TEXT NOT NULL, PRIMARY KEY(tenant,kind,id))",
            "CREATE INDEX cix_entity_parent ON cix_entities (tenant,kind,parent,id)",
            "CREATE TABLE cix_jobs (tenant TEXT NOT NULL, id TEXT NOT NULL, provider TEXT NOT NULL, owner TEXT NOT NULL, authority TEXT NOT NULL, idem TEXT NOT NULL, fingerprint TEXT NOT NULL, state TEXT NOT NULL, revision BIGINT NOT NULL, attempt INTEGER NOT NULL, lease_owner TEXT NOT NULL, lease_until BIGINT NOT NULL, created_ms BIGINT NOT NULL, body TEXT NOT NULL, PRIMARY KEY(tenant,id), UNIQUE(tenant,provider,owner,idem))",
            "CREATE INDEX cix_job_queue ON cix_jobs(state,lease_until,created_ms)",
            "CREATE TABLE cix_admission (tenant TEXT PRIMARY KEY, queued BIGINT NOT NULL)",
            "CREATE TABLE cix_events (tenant TEXT NOT NULL, id TEXT NOT NULL, provider TEXT NOT NULL, owner TEXT NOT NULL, subject TEXT NOT NULL, body TEXT NOT NULL, PRIMARY KEY(tenant,id))",
            "INSERT INTO cix_schema (id,version) VALUES (1,2) ON CONFLICT (id) DO UPDATE SET version=2",
        ] {sqlx::query(statement).execute(&mut *tx).await?;}
        tx.commit().await?;
        self.check_schema().await
    }
    pub(crate) async fn check_schema(&self) -> Result<()> {
        let row = sqlx::query("SELECT version FROM cix_schema WHERE id=1")
            .fetch_optional(&self.pool)
            .await
            .map_err(|_| Problem::new("migration_required", 503))?;
        if row.and_then(|r| r.try_get::<i32, _>("version").ok()) != Some(2) {
            return Err(Problem::new("migration_required", 503));
        }
        Ok(())
    }
    pub(crate) async fn create<T: Serialize>(
        &self,
        p: &Identity,
        kind: &str,
        id: &str,
        parent: &str,
        body: &T,
    ) -> Result<()> {
        sqlx::query("INSERT INTO cix_entities (tenant,kind,id,provider,owner,parent,revision,body) VALUES ($1,$2,$3,$4,$5,$6,1,$7)")
            .bind(&p.tenant).bind(kind).bind(id).bind(&p.provider).bind(&p.subject).bind(parent).bind(serde_json::to_string(body)?).execute(&self.pool).await?;
        Ok(())
    }
    pub(crate) async fn get<T: DeserializeOwned>(
        &self,
        tenant: &str,
        kind: &str,
        id: &str,
    ) -> Result<Stored<T>> {
        check_id(id)?;
        let row = sqlx::query(
            "SELECT provider,owner,body FROM cix_entities WHERE tenant=$1 AND kind=$2 AND id=$3",
        )
        .bind(tenant)
        .bind(kind)
        .bind(id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(Problem::missing)?;
        stored(tenant, row)
    }
    pub(crate) async fn list<T: DeserializeOwned>(
        &self,
        tenant: &str,
        kind: &str,
        cursor: Option<&str>,
    ) -> Result<Page<Stored<T>>> {
        let cursor = cursor.unwrap_or("");
        if !cursor.is_empty() {
            check_id(cursor)?;
        }
        let rows=sqlx::query("SELECT id,provider,owner,body FROM cix_entities WHERE tenant=$1 AND kind=$2 AND id>$3 ORDER BY id LIMIT 101").bind(tenant).bind(kind).bind(cursor).fetch_all(&self.pool).await?;
        let next_cursor = if rows.len() > 100 {
            Some(rows[99].try_get("id")?)
        } else {
            None
        };
        Ok(Page {
            items: rows
                .into_iter()
                .take(100)
                .map(|r| stored(tenant, r))
                .collect::<Result<_>>()?,
            next_cursor,
        })
    }
    pub(crate) async fn submit(
        &self,
        proof: &QueuedAuthority,
        request: TransformRequest,
        max_queued: u32,
        max_json_bytes: usize,
    ) -> Result<Job> {
        let p = &proof.identity;
        check_key(&request.idempotency_key)?;
        let fingerprint = format!("{:x}", Sha256::digest(serde_json::to_vec(&request)?));
        let now = Count(now_ms());
        let job = Job {
            provider: p.provider.clone(),
            id: new_id(),
            tenant: p.tenant.clone(),
            subject: p.subject.clone(),
            state: JobState::Queued,
            revision: Count(1),
            attempt: 0,
            created_ms: now,
            updated_ms: now,
            request,
            result: None,
            error: None,
        };
        let body = serde_json::to_string(&job)?;
        // Reserve bounded state/error envelope growth before queue mutation.
        if body.len().saturating_add(1024) > max_json_bytes {
            return Err(Problem::limit("json_bytes"));
        }
        let mut tx = self.pool.begin().await?;
        sqlx::query("INSERT INTO cix_admission (tenant,queued) VALUES ($1,0) ON CONFLICT (tenant) DO NOTHING").bind(&p.tenant).execute(&mut *tx).await?;
        sqlx::query("UPDATE cix_admission SET queued=queued WHERE tenant=$1")
            .bind(&p.tenant)
            .execute(&mut *tx)
            .await?;
        let previous=sqlx::query("SELECT fingerprint,body FROM cix_jobs WHERE tenant=$1 AND provider=$2 AND owner=$3 AND idem=$4").bind(&p.tenant).bind(&p.provider).bind(&p.subject).bind(&job.request.idempotency_key).fetch_optional(&mut *tx).await?;
        if let Some(row) = previous {
            if row.try_get::<String, _>("fingerprint")? != fingerprint {
                return Err(Problem::new("idempotency_conflict", 409));
            }
            return Ok(serde_json::from_str(&row.try_get::<String, _>("body")?)?);
        }
        let admitted =
            sqlx::query("UPDATE cix_admission SET queued=queued+1 WHERE tenant=$1 AND queued<$2")
                .bind(&p.tenant)
                .bind(i64::from(max_queued))
                .execute(&mut *tx)
                .await?;
        if admitted.rows_affected() != 1 {
            return Err(Problem::new("queue_full", 429));
        }
        sqlx::query("INSERT INTO cix_jobs (tenant,id,provider,owner,authority,idem,fingerprint,state,revision,attempt,lease_owner,lease_until,created_ms,body) VALUES ($1,$2,$3,$4,$5,$6,$7,'queued',1,0,'',0,$8,$9)")
            .bind(&p.tenant).bind(&job.id).bind(&p.provider).bind(&p.subject).bind(serde_json::to_string(proof)?).bind(&job.request.idempotency_key).bind(fingerprint).bind(now.0 as i64).bind(body).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(job)
    }
    pub(crate) async fn job(&self, tenant: &str, id: &str) -> Result<Job> {
        check_id(id)?;
        let row = sqlx::query(
            "SELECT tenant,provider,owner,body FROM cix_jobs WHERE tenant=$1 AND id=$2",
        )
        .bind(tenant)
        .bind(id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(Problem::missing)?;
        decoded_job(row)
    }
    pub(crate) async fn jobs(&self, tenant: &str, cursor: Option<&str>) -> Result<Page<Job>> {
        let cursor = cursor.unwrap_or("");
        if !cursor.is_empty() {
            check_id(cursor)?;
        }
        let rows=sqlx::query("SELECT id,tenant,provider,owner,body FROM cix_jobs WHERE tenant=$1 AND id>$2 ORDER BY id LIMIT 101").bind(tenant).bind(cursor).fetch_all(&self.pool).await?;
        let next_cursor = if rows.len() > 100 {
            Some(rows[99].try_get("id")?)
        } else {
            None
        };
        Ok(Page {
            items: rows
                .into_iter()
                .take(100)
                .map(decoded_job)
                .collect::<Result<_>>()?,
            next_cursor,
        })
    }
    pub(crate) async fn authority(&self, job: &Job) -> Result<QueuedAuthority> {
        let row = sqlx::query("SELECT authority FROM cix_jobs WHERE tenant=$1 AND id=$2")
            .bind(&job.tenant)
            .bind(&job.id)
            .fetch_one(&self.pool)
            .await?;
        let proof: QueuedAuthority = serde_json::from_str(&row.try_get::<String, _>("authority")?)?;
        if proof.identity != job.owner() {
            return Err(Problem::new("metadata_integrity", 503));
        }
        Ok(proof)
    }
    pub(crate) async fn claim(&self, worker: &str, lease_ms: u64) -> Result<Option<Job>> {
        let now = now_ms();
        let rows=sqlx::query("SELECT tenant,provider,owner,body FROM cix_jobs WHERE state='queued' OR ((state='running' OR state='cancelling') AND lease_until<$1) ORDER BY created_ms LIMIT 8").bind(now as i64).fetch_all(&self.pool).await?;
        for row in rows {
            let mut job = decoded_job(row)?;
            let old = job.revision.0;
            if job.state != JobState::Cancelling {
                job.state = JobState::Running;
            }
            job.attempt = job.attempt.checked_add(1).ok_or_else(Problem::internal)?;
            job.revision.0 += 1;
            job.updated_ms = Count(now);
            let result=sqlx::query("UPDATE cix_jobs SET state=$1,revision=$2,attempt=$3,lease_owner=$4,lease_until=$5,body=$6 WHERE tenant=$7 AND id=$8 AND revision=$9")
                .bind(job.state.as_str()).bind(job.revision.0 as i64).bind(i32::try_from(job.attempt).map_err(|_|Problem::internal())?).bind(worker).bind(now.saturating_add(lease_ms) as i64).bind(serde_json::to_string(&job)?).bind(&job.tenant).bind(&job.id).bind(old as i64).execute(&self.pool).await?;
            if result.rows_affected() == 1 {
                return Ok(Some(job));
            }
        }
        Ok(None)
    }
    pub(crate) async fn heartbeat(&self, job: &Job, worker: &str, lease_ms: u64) -> Result<bool> {
        Ok(sqlx::query("UPDATE cix_jobs SET lease_until=$1 WHERE tenant=$2 AND id=$3 AND lease_owner=$4 AND (state='running' OR state='cancelling') AND lease_until>=$5")
            .bind(now_ms().saturating_add(lease_ms) as i64).bind(&job.tenant).bind(&job.id).bind(worker).bind(now_ms() as i64).execute(&self.pool).await?.rows_affected()==1)
    }
    /// Output metadata and terminal state commit together under revision and live lease fences.
    pub(crate) async fn update_job(
        &self,
        job: &Job,
        old_revision: u64,
        worker: Option<&str>,
    ) -> Result<bool> {
        let mut tx = self.pool.begin().await?;
        let result=sqlx::query("UPDATE cix_jobs SET state=$1,revision=$2,body=$3 WHERE tenant=$4 AND id=$5 AND revision=$6 AND ($7='' OR (lease_owner=$7 AND lease_until>=$8)) AND state NOT IN ('succeeded','failed','cancelled')")
            .bind(job.state.as_str()).bind(job.revision.0 as i64).bind(serde_json::to_string(job)?).bind(&job.tenant).bind(&job.id).bind(old_revision as i64).bind(worker.unwrap_or("")).bind(now_ms() as i64).execute(&mut *tx).await?;
        if result.rows_affected() != 1 {
            return Ok(false);
        }
        if job.state == JobState::Succeeded {
            if let Some(artifact) = job.result.as_ref().and_then(|r| r.artifact.as_ref()) {
                sqlx::query("INSERT INTO cix_entities (tenant,kind,id,provider,owner,parent,revision,body) VALUES ($1,'artifact',$2,$3,$4,'',1,$5)")
                    .bind(&job.tenant).bind(&artifact.id).bind(&job.provider).bind(&job.subject).bind(serde_json::to_string(artifact)?).execute(&mut *tx).await?;
            }
        }
        if job.state.terminal() {
            sqlx::query("UPDATE cix_admission SET queued=queued-1 WHERE tenant=$1 AND queued>0")
                .bind(&job.tenant)
                .execute(&mut *tx)
                .await?;
        }
        let event = Event {
            id: new_id(),
            kind: format!("job.{}", job.state.as_str()),
            subject: job.id.clone(),
            created_ms: Count(now_ms()),
            data: serde_json::json!({"revision":job.revision}),
        };
        sqlx::query("INSERT INTO cix_events (tenant,id,provider,owner,subject,body) VALUES ($1,$2,$3,$4,$5,$6)").bind(&job.tenant).bind(&event.id).bind(&job.provider).bind(&job.subject).bind(&job.id).bind(serde_json::to_string(&event)?).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(true)
    }
    pub(crate) async fn events(&self, job: &Job, cursor: Option<&str>) -> Result<Page<Event>> {
        let cursor = cursor.unwrap_or("");
        if !cursor.is_empty() {
            check_id(cursor)?;
        }
        let rows=sqlx::query("SELECT id,body FROM cix_events WHERE tenant=$1 AND subject=$2 AND provider=$3 AND owner=$4 AND id>$5 ORDER BY id LIMIT 101").bind(&job.tenant).bind(&job.id).bind(&job.provider).bind(&job.subject).bind(cursor).fetch_all(&self.pool).await?;
        let next_cursor = if rows.len() > 100 {
            Some(rows[99].try_get("id")?)
        } else {
            None
        };
        Ok(Page {
            items: rows
                .into_iter()
                .take(100)
                .map(|r| Ok(serde_json::from_str(&r.try_get::<String, _>("body")?)?))
                .collect::<Result<_>>()?,
            next_cursor,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::{
        auth::Authenticator,
        config::{Config, Credential},
    };
    fn authority() -> Authenticator {
        let mut config = Config::default();
        config.state_directory = std::env::temp_dir().join("cix-repository-fixture-unused");
        config.stdio_tenant = "team".into();
        config.stdio_subject = "alice".into();
        for (subject, token) in [
            ("alice", "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
            ("bob", "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"),
        ] {
            config.credentials.push(Credential {
                subject: subject.into(),
                tenant: "team".into(),
                token_env: String::new(),
                key_sha256: Some(format!("{:x}", Sha256::digest(token))),
                expires_unix_seconds: None,
                scopes: vec!["*".into()],
                collections: vec![],
            });
        }
        Authenticator::load(&config).unwrap()
    }
    fn request(key: &str) -> TransformRequest {
        TransformRequest {
            operation: Operation::Compress,
            source: InputRef::Artifact { id: new_id() },
            options: CompressionOptions::default(),
            limits: ResourceLimits::default(),
            idempotency_key: key.into(),
        }
    }
    async fn repository() -> Repository {
        let r = Repository::connect("sqlite::memory:").await.unwrap();
        r.migrate().await.unwrap();
        r
    }
    fn proof(auth: &Authenticator, token: &str) -> QueuedAuthority {
        auth.queued(&auth.authenticate(Some(&format!("Bearer {token}"))).unwrap())
            .unwrap()
    }
    #[tokio::test]
    async fn idempotency_includes_provider_tenant_subject_and_key() {
        let r = repository().await;
        let auth = authority();
        let a = proof(&auth, "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
        let b = proof(&auth, "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb");
        let local = auth.queued(&auth.local_host().unwrap()).unwrap();
        let request = request("stable");
        let job = r.submit(&a, request.clone(), 8, 1 << 20).await.unwrap();
        assert_eq!(
            r.submit(&a, request.clone(), 8, 1 << 20).await.unwrap().id,
            job.id
        );
        let mut changed = request.clone();
        changed.options.profile = Profile::Best;
        assert_eq!(
            r.submit(&a, changed, 8, 1 << 20).await.unwrap_err().code,
            "idempotency_conflict"
        );
        assert_ne!(
            r.submit(&b, request.clone(), 8, 1 << 20).await.unwrap().id,
            job.id
        );
        assert_ne!(
            r.submit(&local, request, 8, 1 << 20).await.unwrap().id,
            job.id
        );
        assert_eq!(r.job("other", &job.id).await.unwrap_err().code, "not_found");
        let stored = r.job("team", &job.id).await.unwrap();
        assert_eq!(stored.owner(), a.identity);
        assert!(!serde_json::to_string(&stored)
            .unwrap()
            .contains("credential_id"));
    }
    async fn cancel(r: &Repository, mut job: Job) {
        let old = job.revision.0;
        job.revision.0 += 1;
        job.state = if job.state == JobState::Queued {
            JobState::Cancelled
        } else {
            JobState::Cancelling
        };
        assert!(r.update_job(&job, old, None).await.unwrap());
    }
    #[tokio::test]
    async fn cancellation_releases_admission_and_fences_stale_publication() {
        let r = repository().await;
        let auth = authority();
        let p = proof(&auth, "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
        let job = r.submit(&p, request("one"), 1, 1 << 20).await.unwrap();
        assert_eq!(
            r.submit(&p, request("two"), 1, 1 << 20)
                .await
                .unwrap_err()
                .code,
            "queue_full"
        );
        cancel(&r, job).await;
        r.submit(&p, request("two"), 1, 1 << 20).await.unwrap();
        let mut claimed = r.claim("worker-a", 30_000).await.unwrap().unwrap();
        let old = claimed.revision.0;
        cancel(&r, claimed.clone()).await;
        claimed.revision.0 += 1;
        claimed.state = JobState::Succeeded;
        assert!(!r.update_job(&claimed, old, Some("worker-a")).await.unwrap());
        assert_eq!(
            r.job("team", &claimed.id).await.unwrap().state,
            JobState::Cancelling
        );
    }
    #[tokio::test]
    async fn only_one_worker_claims_a_revision_and_expired_lease_cannot_publish() {
        let r = repository().await;
        let auth = authority();
        let p = proof(&auth, "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
        r.submit(&p, request("one"), 2, 1 << 20).await.unwrap();
        let (a, b) = tokio::join!(r.claim("a", 30_000), r.claim("b", 30_000));
        let a = a.unwrap();
        let b = b.unwrap();
        assert_eq!(usize::from(a.is_some()) + usize::from(b.is_some()), 1);
        let worker = if a.is_some() { "a" } else { "b" };
        let mut job = a.or(b).unwrap();
        let old = job.revision.0;
        sqlx::query("UPDATE cix_jobs SET lease_until=0")
            .execute(&r.pool)
            .await
            .unwrap();
        job.revision.0 += 1;
        job.state = JobState::Succeeded;
        assert!(!r.update_job(&job, old, Some(worker)).await.unwrap());
    }
    #[tokio::test]
    async fn populated_v1_is_refused_transactionally_and_empty_v1_migrates() {
        for populated in [false, true] {
            let r = Repository::connect("sqlite::memory:").await.unwrap();
            for sql in [
                "CREATE TABLE cix_schema (id INTEGER PRIMARY KEY, version INTEGER NOT NULL)",
                "INSERT INTO cix_schema VALUES (1,1)",
                "CREATE TABLE cix_entities (id TEXT)",
                "CREATE TABLE cix_jobs (id TEXT)",
                "CREATE TABLE cix_events (id TEXT)",
                "CREATE TABLE cix_admission (id TEXT)",
            ] {
                sqlx::query(sql).execute(&r.pool).await.unwrap();
            }
            if populated {
                sqlx::query("INSERT INTO cix_entities VALUES ('legacy-owner-without-provider')")
                    .execute(&r.pool)
                    .await
                    .unwrap();
            }
            let result = r.migrate().await;
            let version: i32 = sqlx::query("SELECT version FROM cix_schema")
                .fetch_one(&r.pool)
                .await
                .unwrap()
                .try_get("version")
                .unwrap();
            if populated {
                assert_eq!(
                    result.unwrap_err().code,
                    "owner_provider_migration_required"
                );
                assert_eq!(version, 1);
                assert_eq!(
                    sqlx::query("SELECT id FROM cix_entities")
                        .fetch_one(&r.pool)
                        .await
                        .unwrap()
                        .try_get::<String, _>("id")
                        .unwrap(),
                    "legacy-owner-without-provider"
                );
            } else {
                result.unwrap();
                assert_eq!(version, 2);
                r.check_schema().await.unwrap();
            }
        }
    }
}
