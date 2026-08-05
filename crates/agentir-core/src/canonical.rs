//! Deterministic serialization and content hashing.

use crate::{
    diagnostics::{AgentError, AgentResult, ErrorCode},
    ir::Program,
};
use sha2::{Digest, Sha256};
use std::fmt::Write;

/// Serializes canonical program state to deterministic compact JSON bytes.
pub fn canonical_bytes(program: &Program) -> AgentResult<Vec<u8>> {
    serde_json::to_vec(program).map_err(|error| {
        AgentError::new(
            ErrorCode::TransactionRejected,
            format!("canonical serialization failed: {error}"),
        )
    })
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
