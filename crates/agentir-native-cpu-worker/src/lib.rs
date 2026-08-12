//! Isolated Stage 9 native CPU worker implementation.

#![deny(unsafe_op_in_unsafe_fn)]
#![warn(missing_docs)]

mod bridge;
mod lowering;

pub use agentir_runtime_native_cpu::{
    CRANELIFT_VERSION, CpuNativeRuntimeIdentity, MAX_WORKER_FRAME_BYTES, NATIVE_CALL_ABI_VERSION,
    NATIVE_WORKER_PROTOCOL_VERSION, NativeWorkerError, NativeWorkerRequest, NativeWorkerResponse,
    NativeWorkerResult, NativeWorkerSuccess, failure_response, launch_worker_once,
};
use std::io::{Read, Write};

fn response_from_stdin() -> NativeWorkerResponse {
    let mut bytes = Vec::new();
    if let Err(error) = std::io::stdin()
        .take(u64::try_from(MAX_WORKER_FRAME_BYTES).unwrap_or(u64::MAX) + 1)
        .read_to_end(&mut bytes)
    {
        return failure_response(&NativeWorkerError::from_io("WORKER_READ", &error));
    }
    if bytes.len() > MAX_WORKER_FRAME_BYTES {
        return failure_response(&NativeWorkerError::internal(
            "WORKER_REQUEST_TOO_LARGE",
            "worker request exceeds the internal frame limit",
        ));
    }
    let request: NativeWorkerRequest = match serde_json::from_slice(&bytes) {
        Ok(request) => request,
        Err(error) => {
            return failure_response(&NativeWorkerError::internal(
                "WORKER_REQUEST_MALFORMED",
                format!("worker request decoding failed: {error}"),
            ));
        }
    };
    match lowering::execute(&request) {
        Ok(success) => NativeWorkerResponse::Ok(Box::new(success)),
        Err(error) => failure_response(&error),
    }
}

/// Runs the one-request worker on standard I/O and then returns for immediate process exit.
pub fn run_worker_stdio() {
    let mut encoded = serde_json::to_vec(&response_from_stdin()).unwrap_or_else(|error| {
        format!(
            "{{\"status\":\"error\",\"error\":{{\"code\":\"WORKER_RESPONSE_ENCODE\",\"message\":\"{error}\"}}}}"
        )
        .into_bytes()
    });
    if encoded.len() > MAX_WORKER_FRAME_BYTES {
        encoded = br#"{"status":"error","error":{"code":"WORKER_RESPONSE_TOO_LARGE","message":"encoded worker response exceeds the internal frame limit"}}"#.to_vec();
    }
    let _ = std::io::stdout().write_all(&encoded);
}
