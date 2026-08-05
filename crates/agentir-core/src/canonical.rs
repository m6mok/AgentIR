//! Deterministic serialization and content hashing.

use crate::{
    diagnostics::{AgentError, AgentResult, ErrorCode},
    ir::Program,
    resources::{BudgetCheck, ResourceKind, ResourceLimits},
};
use sha2::{Digest, Sha256};
use std::fmt::Write;

/// Serializes canonical program state to deterministic compact JSON bytes.
pub fn canonical_bytes(program: &Program) -> AgentResult<Vec<u8>> {
    canonical_bytes_with_limit(
        program,
        ResourceLimits::hard_safety_caps().canonical_output_bytes,
    )
}

/// Serializes exact state and rejects output larger than `max_bytes`.
pub fn canonical_bytes_with_limit(program: &Program, max_bytes: u64) -> AgentResult<Vec<u8>> {
    let bytes = serde_json::to_vec(program).map_err(|error| {
        AgentError::new(
            ErrorCode::TransactionRejected,
            format!("canonical serialization failed: {error}"),
        )
    })?;
    BudgetCheck::ensure(
        ResourceKind::CanonicalOutputBytes,
        max_bytes,
        u64::try_from(bytes.len()).unwrap_or(u64::MAX),
        "exact-state canonical serialization",
    )?;
    Ok(bytes)
}

/// Computes a lowercase SHA-256 content hash of canonical program state.
pub fn content_hash(program: &Program) -> AgentResult<String> {
    let digest = Sha256::digest(canonical_bytes(program)?);
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(output, "{byte:02x}").expect("writing to String cannot fail");
    }
    Ok(output)
}

/// Computes a content hash while enforcing a caller-selected canonical byte limit.
pub fn content_hash_with_limit(program: &Program, max_bytes: u64) -> AgentResult<String> {
    let digest = Sha256::digest(canonical_bytes_with_limit(program, max_bytes)?);
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(output, "{byte:02x}").expect("writing to String cannot fail");
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::{canonical_bytes, content_hash};
    use crate::ir::Program;

    #[test]
    fn serialization_and_hash_are_stable() {
        let program = Program::default();
        assert_eq!(
            canonical_bytes(&program).expect("serializes"),
            canonical_bytes(&program).expect("serializes")
        );
        assert_eq!(
            content_hash(&program).expect("hashes"),
            content_hash(&program).expect("hashes")
        );
    }
}
