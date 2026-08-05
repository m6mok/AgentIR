//! Common response envelope.

use agentir_core::diagnostics::{AgentError, AgentResult, ErrorCode};
use serde::Serialize;
use serde_json::Value;

/// Exactly one response emitted for one JSONL request.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Response {
    /// Whether request processing succeeded.
    pub ok: bool,
    /// Request correlation ID.
    pub request_id: String,
    /// Successful structured result.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    /// Structured failure.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<AgentError>,
    /// Non-fatal diagnostics; empty in the initial reference implementation.
    pub diagnostics: Vec<AgentError>,
}

impl Response {
    /// Wraps a successful result.
    #[must_use]
    pub fn success(request_id: impl Into<String>, result: Value) -> Self {
        Self {
            ok: true,
            request_id: request_id.into(),
            result: Some(result),
            error: None,
            diagnostics: Vec::new(),
        }
    }

    /// Wraps a structured compiler or protocol error.
    #[must_use]
    pub fn failure(request_id: impl Into<String>, error: AgentError) -> Self {
        Self {
            ok: false,
            request_id: request_id.into(),
            result: None,
            error: Some(error),
            diagnostics: Vec::new(),
        }
    }

    /// Deterministically serializes this envelope as one compact JSON object.
    pub fn to_json_line(&self) -> AgentResult<String> {
        serde_json::to_string(self).map_err(|error| {
            AgentError::new(
                ErrorCode::InvalidRequest,
                format!("response serialization failed: {error}"),
            )
        })
    }
}
