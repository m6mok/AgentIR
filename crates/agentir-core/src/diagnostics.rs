//! Stable machine-oriented diagnostics.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, error::Error, fmt};

/// Stable Stage 1 diagnostic codes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    /// Requested workspace does not exist.
    WorkspaceNotFound,
    /// Requested revision does not exist.
    RevisionNotFound,
    /// The transaction was based on a stale or disallowed revision.
    BaseRevisionConflict,
    /// A value, hole, dimension, or local binding could not be resolved.
    UnknownReference,
    /// A transaction-local binding was defined twice.
    DuplicateBinding,
    /// The opcode is not supported by this profile.
    UnknownOpcode,
    /// An operation received the wrong number of operands.
    ArityMismatch,
    /// Types are not compatible.
    TypeMismatch,
    /// Tensor shapes are incompatible.
    ShapeMismatch,
    /// A region is malformed or yields an incompatible type.
    InvalidRegion,
    /// A value cannot fill a hole of the requested type.
    HoleTypeMismatch,
    /// An operation requires every hole to be filled.
    OpenHole,
    /// The specification has no valid, complete outputs.
    SpecNotComplete,
    /// The frozen specification cannot be edited.
    SpecFrozen,
    /// The transaction failed atomically.
    TransactionRejected,
    /// Evaluation inputs do not match parameters.
    EvaluationInputMismatch,
    /// Arithmetic is defined to reject division by zero in Stage 1.
    DivisionByZero,
    /// JSON or request data is malformed.
    InvalidRequest,
    /// Workspace archive I/O failed.
    PersistenceIo,
    /// Workspace archive format or version is unsupported.
    PersistenceFormat,
    /// Workspace archive failed an integrity check.
    PersistenceIntegrity,
    /// Replayed events did not reproduce the archived revision graph.
    ReplayMismatch,
}

/// Structured compiler error suitable for agent repair loops.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentError {
    /// Stable error code.
    pub code: ErrorCode,
    /// Short machine-oriented explanation.
    pub message: String,
    /// Optional structured origin.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin: Option<Value>,
    /// Optional expected property.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected: Option<Value>,
    /// Optional actual property.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actual: Option<Value>,
    /// Legal or likely repair actions.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub repairs: Vec<String>,
    /// Additional deterministic fields.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub details: BTreeMap<String, Value>,
}

impl AgentError {
    /// Creates an error with no optional diagnostic fields.
    #[must_use]
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            origin: None,
            expected: None,
            actual: None,
            repairs: Vec::new(),
            details: BTreeMap::new(),
        }
    }

    /// Adds expected and actual values.
    #[must_use]
    pub fn with_types(mut self, expected: impl Into<Value>, actual: impl Into<Value>) -> Self {
        self.expected = Some(expected.into());
        self.actual = Some(actual.into());
        self
    }

    /// Adds one structured detail.
    #[must_use]
    pub fn with_detail(mut self, key: impl Into<String>, value: impl Into<Value>) -> Self {
        self.details.insert(key.into(), value.into());
        self
    }
}

impl fmt::Display for AgentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:?}: {}", self.code, self.message)
    }
}

impl Error for AgentError {}

/// Result returned by compiler-core operations.
pub type AgentResult<T> = Result<T, AgentError>;
