//! Transport-independent v1 contracts; large counters are decimal JSON strings.
use super::error::{Problem, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Ord, PartialOrd)]
pub struct Count(pub u64);
impl Serialize for Count {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&self.0.to_string())
    }
}
impl<'de> Deserialize<'de> for Count {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let value = String::deserialize(d)?;
        if value.is_empty()
            || (value.len() > 1 && value.starts_with('0'))
            || !value.bytes().all(|c| c.is_ascii_digit())
        {
            return Err(serde::de::Error::custom(
                "expected canonical unsigned decimal string",
            ));
        }
        value.parse().map(Self).map_err(serde::de::Error::custom)
    }
}
impl JsonSchema for Count {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "UInt64Decimal".into()
    }
    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({"type":"string", "pattern":"^(?:0|[1-9][0-9]{0,18}|1[0-7][0-9]{18}|18[0-3][0-9]{17}|184[0-3][0-9]{16}|1844[0-5][0-9]{15}|18446[0-6][0-9]{14}|184467[0-3][0-9]{13}|1844674[0-3][0-9]{12}|184467440[0-6][0-9]{10}|1844674407[0-2][0-9]{9}|18446744073[0-6][0-9]{8}|1844674407370[0-8][0-9]{6}|18446744073709[0-4][0-9]{5}|184467440737095[0-4][0-9]{4}|18446744073709550[0-9]{3}|18446744073709551[0-5][0-9]{2}|1844674407370955160[0-9]{1}|1844674407370955161[0-4]|18446744073709551615)(?![\\s\\S])", "maxLength":20, "description":"Unsigned 64-bit decimal integer; maximum 18446744073709551615."})
    }
}
impl Count {
    pub fn usize(self) -> Result<usize> {
        usize::try_from(self.0).map_err(|_| Problem::limit("address_space"))
    }
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Profile {
    #[default]
    Fast,
    Default,
    Best,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct ResourceLimits {
    pub input_bytes: Count,
    pub output_bytes: Count,
    pub memory_bytes: Count,
    pub temporary_bytes: Count,
    pub workers: u32,
    pub deadline_ms: Count,
}
impl Default for ResourceLimits {
    fn default() -> Self {
        Self {
            input_bytes: Count(64 << 20),
            output_bytes: Count(256 << 20),
            memory_bytes: Count(512 << 20),
            temporary_bytes: Count(512 << 20),
            workers: 1,
            deadline_ms: Count(60_000),
        }
    }
}
impl ResourceLimits {
    pub fn within(&self, cap: &Self) -> Result<()> {
        // Zero temporary storage is a prohibition; other resources must be positive.
        for (name, n, max) in [
            ("input_bytes", self.input_bytes, cap.input_bytes),
            ("output_bytes", self.output_bytes, cap.output_bytes),
            ("memory_bytes", self.memory_bytes, cap.memory_bytes),
            ("temporary_bytes", self.temporary_bytes, cap.temporary_bytes),
            ("deadline_ms", self.deadline_ms, cap.deadline_ms),
        ] {
            if (n.0 == 0 && name != "temporary_bytes") || n > max {
                return Err(Problem::limit(name));
            }
            n.usize()?;
        }
        if self.workers == 0 || self.workers > cap.workers {
            return Err(Problem::limit("workers"));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct CompressionOptions {
    pub profile: Profile,
    /// CIX or a genuine standard stream, never an HTTP Content-Encoding alias.
    pub output_format: Option<String>,
    pub container: Option<String>,
    pub route: Option<String>,
    pub backend: Option<String>,
    pub dictionary_id: Option<String>,
    pub block_bytes: Option<Count>,
    pub history_bytes: Option<Count>,
    pub independent_blocks: Option<bool>,
    pub streaming: bool,
    pub flush_interval_ms: Option<Count>,
    pub parallelism: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum InputRef {
    Artifact {
        id: String,
    },
    EntryVersion {
        collection_id: String,
        entry_id: String,
        version_id: String,
    },
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    Compress,
    Decompress,
    Verify,
    Inspect,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TransformRequest {
    pub operation: Operation,
    pub source: InputRef,
    #[serde(default)]
    pub options: CompressionOptions,
    #[serde(default)]
    pub limits: ResourceLimits,
    pub idempotency_key: String,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    Queued,
    Running,
    Cancelling,
    Succeeded,
    Failed,
    Cancelled,
}
impl JobState {
    pub fn terminal(self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed | Self::Cancelled)
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Cancelling => "cancelling",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct Artifact {
    pub id: String,
    pub bytes: Count,
    pub sha256: String,
    pub media_type: String,
    pub created_ms: Count,
    pub expires_ms: Option<Count>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct JobResult {
    pub artifact: Option<Artifact>,
    pub report: Value,
    pub applied_options: CompressionOptions,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct Job {
    /// Verified authentication provider; part of durable ownership.
    pub provider: String,
    pub id: String,
    pub tenant: String,
    pub subject: String,
    pub state: JobState,
    pub revision: Count,
    pub attempt: u32,
    pub created_ms: Count,
    pub updated_ms: Count,
    pub request: TransformRequest,
    pub result: Option<JobResult>,
    pub error: Option<Problem>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct Event {
    pub id: String,
    pub kind: String,
    pub subject: String,
    pub created_ms: Count,
    pub data: Value,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct Collection {
    pub id: String,
    pub name: String,
    pub revision: Count,
    pub created_ms: Count,
    pub profile: Profile,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateCollection {
    pub name: String,
    #[serde(default)]
    pub profile: Profile,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct Entry {
    pub id: String,
    pub collection_id: String,
    pub path: String,
    pub revision: Count,
    pub version_id: String,
    pub deleted: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct Chunk {
    pub artifact_id: String,
    pub decoded_bytes: Count,
    pub decoded_sha256: String,
    pub encoding: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct EntryVersion {
    pub id: String,
    pub entry_id: String,
    pub collection_id: String,
    pub created_ms: Count,
    pub bytes: Count,
    pub sha256: String,
    pub chunks: Vec<Chunk>,
    pub metadata: BTreeMap<String, String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PutEntry {
    pub path: String,
    pub artifact_id: String,
    pub expected_revision: Count,
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct Snapshot {
    pub id: String,
    pub collection_id: String,
    pub created_ms: Count,
    pub entries: Vec<Entry>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next_cursor: Option<String>,
}

pub fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}
pub fn check_id(value: &str) -> Result<()> {
    uuid::Uuid::parse_str(value)
        .map(|_| ())
        .map_err(|_| Problem::invalid("id"))
}
pub fn check_key(value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.:".contains(&b))
    {
        return Err(Problem::invalid("idempotency_key"));
    }
    Ok(())
}
pub fn check_path(value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 4096
        || value.starts_with('/')
        || value.contains(['\\', '\0'])
        || value
            .split('/')
            .any(|s| s.is_empty() || s == "." || s == "..")
    {
        return Err(Problem::invalid("path"));
    }
    Ok(())
}

impl Profile {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Fast => "fast",
            Self::Default => "default",
            Self::Best => "best",
        }
    }
}
impl Job {
    pub(crate) fn owner(&self) -> crate::managed_sdk::Identity {
        crate::managed_sdk::Identity {
            provider: self.provider.clone(),
            tenant: self.tenant.clone(),
            subject: self.subject.clone(),
        }
    }
}
