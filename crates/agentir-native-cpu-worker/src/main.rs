//! One-request Stage 9A native CPU worker process.

#![deny(unsafe_op_in_unsafe_fn)]

mod bridge;
mod lowering;

pub(crate) use agentir_native_cpu_worker::{
    CRANELIFT_VERSION, CpuNativeRuntimeIdentity, NATIVE_CALL_ABI_VERSION,
    NATIVE_WORKER_PROTOCOL_VERSION, NativeWorkerResult, NativeWorkerSuccess,
};
use agentir_native_cpu_worker::{
    MAX_WORKER_FRAME_BYTES, NativeWorkerError, NativeWorkerRequest, NativeWorkerResponse,
    failure_response,
};
use std::io::{Read, Write};

fn run() -> NativeWorkerResponse {
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
        Ok(success) => NativeWorkerResponse::Ok(success),
        Err(error) => failure_response(&error),
    }
}

fn main() {
    let mut encoded = serde_json::to_vec(&run()).unwrap_or_else(|error| {
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
