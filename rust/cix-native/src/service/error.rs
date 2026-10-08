//! Stable machine errors. Presentation can translate `message_id` and `arguments`.
use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub type Result<T> = std::result::Result<T, Problem>;

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct Problem {
    pub code: String,
    pub message_id: String,
    pub arguments: Value,
    pub status: u16,
    pub retryable: bool,
}

impl Problem {
    pub fn new(code: &str, status: u16) -> Self {
        Self {
            code: code.into(),
            message_id: format!("service.error.{code}"),
            arguments: json!({}),
            status,
            retryable: matches!(status, 429 | 503),
        }
    }
    pub fn argument(mut self, key: &str, value: impl Into<Value>) -> Self {
        self.arguments[key] = value.into();
        self
    }
    pub fn invalid(field: &str) -> Self {
        Self::new("invalid_argument", 400).argument("field", field)
    }
    pub fn limit(resource: &str) -> Self {
        Self::new("resource_limit", 413).argument("resource", resource)
    }
    pub fn missing() -> Self {
        Self::new("not_found", 404)
    }
    pub fn conflict() -> Self {
        Self::new("revision_conflict", 409)
    }
    pub fn internal() -> Self {
        Self::new("internal", 500)
    }
}
impl std::fmt::Display for Problem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.code)
    }
}
impl std::error::Error for Problem {}
impl From<std::io::Error> for Problem {
    fn from(_: std::io::Error) -> Self {
        Self::new("storage_io", 503)
    }
}
impl From<sqlx::Error> for Problem {
    fn from(error: sqlx::Error) -> Self {
        if let sqlx::Error::Database(ref db) = error {
            if db.is_unique_violation() {
                return Self::conflict();
            }
        }
        Self::new("metadata_unavailable", 503)
    }
}
impl From<serde_json::Error> for Problem {
    fn from(_: serde_json::Error) -> Self {
        Self::invalid("json")
    }
}
impl IntoResponse for Problem {
    fn into_response(self) -> Response {
        let status = StatusCode::from_u16(self.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        (
            status,
            [("content-type", "application/problem+json")],
            Json(self),
        )
            .into_response()
    }
}
