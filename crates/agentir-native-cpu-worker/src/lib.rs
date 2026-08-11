//! Internal Stage 9A native CPU worker protocol and process launcher.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use agentir_core::cpu::{CpuArtifactHash, CpuArtifactPackage};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    fmt,
    io::Write,
    path::Path,
    process::{Command, Stdio},
};

/// Version of the bounded internal worker request/response envelope.
pub const NATIVE_WORKER_PROTOCOL_VERSION: u32 = 1;
/// Version of the worker-local packed-buffer call ABI.
pub const NATIVE_CALL_ABI_VERSION: u32 = 1;
/// Exact pinned Cranelift family used by this worker.
pub const CRANELIFT_VERSION: &str = "0.116.1";
/// Maximum encoded request or response accepted by the internal worker.
pub const MAX_WORKER_FRAME_BYTES: usize = 16 * 1024 * 1024;

/// One bounded internal request. This type is not exposed by JSONL.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeWorkerRequest {
    /// Internal protocol version.
    pub protocol_version: u32,
    /// Unchanged compiler-published Stage 8A package.
    pub package: CpuArtifactPackage,
    /// Exact retained package hash expected by the launcher.
    pub expected_cpu_artifact_hash: CpuArtifactHash,
    /// Ordinary runtime inputs using the unchanged Stage 8A binding interface.
    pub inputs: BTreeMap<String, Value>,
}

/// Observation-only identity of the worker code-generation runtime.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CpuNativeRuntimeIdentity {
    /// Domain-separated runtime identity.
    pub cpu_native_runtime_hash: String,
    /// Internal worker protocol version.
    pub worker_protocol_version: u32,
    /// AgentIR native worker build identity.
    pub runtime_build: String,
    /// Exact pinned Cranelift family.
    pub cranelift_version: String,
    /// Server-detected host target triple.
    pub target_triple: String,
    /// ISA features actually enabled by host detection.
    pub enabled_isa_features: Vec<String>,
    /// Fixed server-owned code-generation settings.
    pub codegen_settings: Vec<String>,
    /// Internal packed-buffer ABI version.
    pub abi_version: u32,
}

/// Successful bounded native execution response.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NativeWorkerSuccess {
    /// Internal protocol version.
    pub protocol_version: u32,
    /// Observation-only native runtime identity.
    pub runtime: CpuNativeRuntimeIdentity,
    /// Exact Stage 8A package consumed by the worker.
    pub cpu_artifact_hash: CpuArtifactHash,
    /// Named finite outputs produced by one native call.
    pub outputs: BTreeMap<String, Value>,
}

/// One typed worker failure.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeWorkerFailure {
    /// Stable internal diagnostic label.
    pub code: String,
    /// Human-readable failure description.
    pub message: String,
}

/// Exactly one bounded response emitted before worker exit.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum NativeWorkerResponse {
    /// Native lowering and one call completed.
    Ok(NativeWorkerSuccess),
    /// Validation, lowering, process, or response validation failed.
    Error {
        /// Typed internal failure.
        error: NativeWorkerFailure,
    },
}

/// Internal Stage 9A worker error.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeWorkerError {
    code: &'static str,
    message: String,
}

impl NativeWorkerError {
    #[doc(hidden)]
    #[must_use]
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    fn failure(&self) -> NativeWorkerFailure {
        NativeWorkerFailure {
            code: self.code.to_owned(),
            message: self.message.clone(),
        }
    }

    /// Creates a typed error for the one-request worker binary.
    #[doc(hidden)]
    #[must_use]
    pub fn internal(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(code, message)
    }

    /// Wraps worker I/O without exposing transport details to compiler crates.
    #[doc(hidden)]
    #[must_use]
    pub fn from_io(code: &'static str, error: &std::io::Error) -> Self {
        Self::new(code, error.to_string())
    }
}

impl fmt::Display for NativeWorkerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for NativeWorkerError {}

/// Internal result used by the worker implementation and launcher.
pub type NativeWorkerResult<T> = Result<T, NativeWorkerError>;

/// Encodes a typed failure response.
#[must_use]
pub fn failure_response(error: &NativeWorkerError) -> NativeWorkerResponse {
    NativeWorkerResponse::Error {
        error: error.failure(),
    }
}

/// Launches one fresh worker process for one internal request and requires its exit.
///
/// The executable path is supplied only by an internal/test caller. No production
/// JSONL command or client-controlled worker-path field exists in Stage 9A.
pub fn launch_worker_once(
    worker_executable: &Path,
    request: &NativeWorkerRequest,
) -> NativeWorkerResult<NativeWorkerSuccess> {
    let encoded = serde_json::to_vec(request).map_err(|error| {
        NativeWorkerError::new(
            "WORKER_REQUEST_ENCODE",
            format!("request encoding failed: {error}"),
        )
    })?;
    if encoded.len() > MAX_WORKER_FRAME_BYTES {
        return Err(NativeWorkerError::new(
            "WORKER_REQUEST_TOO_LARGE",
            "encoded worker request exceeds the internal frame limit",
        ));
    }
    let mut child = Command::new(worker_executable)
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| {
            NativeWorkerError::new("WORKER_SPAWN", format!("worker spawn failed: {error}"))
        })?;
    child
        .stdin
        .take()
        .ok_or_else(|| NativeWorkerError::new("WORKER_STDIN", "worker stdin is unavailable"))?
        .write_all(&encoded)
        .map_err(|error| {
            NativeWorkerError::new(
                "WORKER_WRITE",
                format!("worker request write failed: {error}"),
            )
        })?;
    let output = child.wait_with_output().map_err(|error| {
        NativeWorkerError::new("WORKER_WAIT", format!("worker wait failed: {error}"))
    })?;
    if !output.status.success() {
        return Err(NativeWorkerError::new(
            "WORKER_EXIT",
            format!("worker exited unsuccessfully: {}", output.status),
        ));
    }
    if !output.stderr.is_empty() {
        return Err(NativeWorkerError::new(
            "WORKER_STDERR",
            "worker emitted unexpected stderr output",
        ));
    }
    if output.stdout.len() > MAX_WORKER_FRAME_BYTES {
        return Err(NativeWorkerError::new(
            "WORKER_RESPONSE_TOO_LARGE",
            "worker response exceeds the internal frame limit",
        ));
    }
    let response: NativeWorkerResponse =
        serde_json::from_slice(&output.stdout).map_err(|error| {
            NativeWorkerError::new(
                "WORKER_RESPONSE_MALFORMED",
                format!("worker response decoding failed: {error}"),
            )
        })?;
    match response {
        NativeWorkerResponse::Ok(success) => {
            if success.protocol_version != NATIVE_WORKER_PROTOCOL_VERSION
                || success.cpu_artifact_hash != request.expected_cpu_artifact_hash
            {
                return Err(NativeWorkerError::new(
                    "WORKER_RESPONSE_STALE",
                    "worker response protocol or artifact identity is stale",
                ));
            }
            Ok(success)
        }
        NativeWorkerResponse::Error { error } => Err(NativeWorkerError::new(
            "WORKER_REJECTED",
            format!("{}: {}", error.code, error.message),
        )),
    }
}
