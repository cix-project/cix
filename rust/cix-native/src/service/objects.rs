//! Private immutable objects, accessed through opened directory handles.
use super::{
    config::Config,
    contracts::*,
    error::{Problem, Result},
    repository::{Repository, Stored},
    storage::Directory,
};
use crate::managed_sdk::Identity;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[derive(Clone)]
pub(crate) struct Objects {
    root: Directory,
    repository: Repository,
    ttl_ms: u64,
    max_metadata_bytes: usize,
}
/// Unpublished files are removed until a metadata commit is attempted.
/// Files must survive an ambiguous/cancelled SQL commit: retaining an orphan is
/// safer than deleting bytes that committed metadata may already reference.
pub(crate) struct Staged {
    directory: Directory,
    artifact: Artifact,
    committed: bool,
}
impl Staged {
    pub(crate) fn artifact(&self) -> &Artifact {
        &self.artifact
    }
    pub(crate) fn commit(mut self) -> Artifact {
        self.committed = true;
        self.artifact.clone()
    }
}
impl Drop for Staged {
    fn drop(&mut self) {
        if !self.committed {
            let _ = self.directory.unlink_file(&self.artifact.id);
        }
    }
}
impl Objects {
    pub(crate) fn open(config: &Config, repository: Repository, state: &Directory) -> Result<Self> {
        Ok(Self {
            root: state.child("objects", true)?,
            repository,
            ttl_ms: config.artifact_ttl_seconds.saturating_mul(1000),
            max_metadata_bytes: config.max_json_bytes,
        })
    }
    /// File is private and unaddressable through the API until metadata commits.
    pub(crate) async fn stage(
        &self,
        p: &Identity,
        data: &[u8],
        media_type: &str,
    ) -> Result<Staged> {
        let now = now_ms();
        let artifact = Artifact {
            id: new_id(),
            bytes: Count(data.len() as u64),
            sha256: hash(data),
            media_type: media_type.into(),
            created_ms: Count(now),
            expires_ms: Some(Count(now.saturating_add(self.ttl_ms))),
        };
        if serde_json::to_vec(&artifact)?.len() > self.max_metadata_bytes {
            return Err(Problem::limit("json_bytes"));
        }
        let directory = self.root.tenant(&p.tenant, true)?;
        let file = directory.create_file(&artifact.id)?;
        let pending = Staged {
            directory,
            artifact,
            committed: false,
        };
        let mut file = tokio::fs::File::from_std(file);
        file.write_all(data).await?;
        file.sync_all().await?;
        drop(file);
        pending.directory.sync()?;
        Ok(pending)
    }
    pub(crate) async fn put(
        &self,
        p: &Identity,
        data: &[u8],
        media_type: &str,
    ) -> Result<Artifact> {
        let artifact = self.stage(p, data, media_type).await?.commit();
        // Keep the file across cancellation or an ambiguous database response.
        // A future owner-aware recovery pass may collect unreferenced files.
        self.repository
            .create(p, "artifact", &artifact.id, "", &artifact)
            .await?;
        Ok(artifact)
    }
    pub(crate) fn discard(
        &self,
        owner: &crate::managed_sdk::Identity,
        artifact: &Artifact,
    ) -> Result<()> {
        check_id(&artifact.id)?;
        self.root
            .tenant(&owner.tenant, false)?
            .unlink_file(&artifact.id)
    }
    pub(crate) async fn describe(&self, tenant: &str, id: &str) -> Result<Stored<Artifact>> {
        let stored: Stored<Artifact> = self.repository.get(tenant, "artifact", id).await?;
        if stored.value.id != id || stored.value.expires_ms.is_some_and(|at| now_ms() >= at.0) {
            return Err(Problem::missing());
        }
        Ok(stored)
    }
    pub(crate) async fn read_known(
        &self,
        tenant: &str,
        artifact: &Artifact,
        cap: usize,
    ) -> Result<Vec<u8>> {
        check_id(&artifact.id)?;
        if artifact.bytes.usize()? > cap {
            return Err(Problem::limit("input_bytes"));
        }
        if artifact.expires_ms.is_some_and(|at| now_ms() >= at.0) {
            return Err(Problem::missing());
        }
        let directory = self.root.tenant(tenant, false)?;
        let mut file = tokio::fs::File::from_std(directory.open_file(&artifact.id)?)
            .take((cap as u64).saturating_add(1));
        let mut data = Vec::new();
        file.read_to_end(&mut data).await?;
        if data.len() > cap {
            return Err(Problem::limit("input_bytes"));
        }
        if data.len() as u64 != artifact.bytes.0 || hash(&data) != artifact.sha256 {
            return Err(Problem::new("object_integrity", 422));
        }
        Ok(data)
    }
}
pub(crate) fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
