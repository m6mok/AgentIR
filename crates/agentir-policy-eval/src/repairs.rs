//! Bounded compiler-owned typed repair descriptors.

use crate::{
    hashing::domain_hash_cleared,
    model::{EvaluationDiagnostic, EvaluationErrorCode, EvaluationResult},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// Typed repair identity domain.
pub const REPAIR_HASH_DOMAIN: &[u8] = b"agentir.evaluation.typed_repair.v1\0";

/// Stable compiler-owned repair taxonomy.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepairCode {
    /// Refresh a stale base revision or hash.
    StaleBase,
    /// Replace an invalid stable reference.
    InvalidReference,
    /// Supply a value of the exact inferred type.
    TypeMismatch,
    /// Supply a value with the exact inferred shape.
    ShapeMismatch,
    /// Close an open compiler obligation.
    OpenObligation,
    /// Use a supported exact rewrite.
    UnsupportedRewrite,
    /// Retain fresh allocation instead of unsafe reuse.
    UnsafeMemoryReuse,
    /// Retain or choose a legal schedule transform.
    IllegalScheduleTransform,
    /// Reduce bounded work below the stated limit.
    ResourceLimit,
    /// Select a supported backend lowering.
    UnsupportedBackendLowering,
    /// Refresh an incompatible ranking/schema/model binding.
    RankingSchemaModelMismatch,
    /// Restart an enumeration from the current anchor.
    StaleContinuationCursor,
}

/// Exact anchor that invalidates a repair after state changes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepairAnchor {
    /// Stable diagnostic code that caused the repair.
    pub diagnostic_code: String,
    /// Exact base revisions and independent hashes.
    pub exact_base: BTreeMap<String, String>,
}

/// One bounded typed repair that still traverses the production path.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RepairDescriptor {
    /// Stable taxonomy code.
    pub code: RepairCode,
    /// Exact rejection/base anchor.
    pub anchor: RepairAnchor,
    /// Ordinary production request; it carries no proof or certificate.
    pub production_request: Value,
    /// Stable bounded explanation.
    pub description: String,
    /// Maximum actions carried by this repair.
    pub maximum_actions: u64,
    /// Independent descriptor hash.
    pub repair_hash: String,
}

/// Constructs one validated typed repair descriptor.
pub fn typed_repair(
    code: RepairCode,
    anchor: RepairAnchor,
    production_request: Value,
    description: impl Into<String>,
    maximum_actions: u64,
) -> EvaluationResult<RepairDescriptor> {
    if maximum_actions == 0 || maximum_actions > 16 {
        return Err(EvaluationDiagnostic::new(
            EvaluationErrorCode::EvaluationTypedRepairInvalid,
            "typed repair action bound must be in 1..=16",
        ));
    }
    if contains_forbidden_proof_field(&production_request) {
        return Err(EvaluationDiagnostic::new(
            EvaluationErrorCode::EvaluationTypedRepairInvalid,
            "typed repair cannot contain agent-supplied proof, guard, or certificate fields",
        ));
    }
    let mut descriptor = RepairDescriptor {
        code,
        anchor,
        production_request,
        description: description.into(),
        maximum_actions,
        repair_hash: String::new(),
    };
    descriptor.repair_hash = repair_hash(&descriptor)?;
    Ok(descriptor)
}

/// Verifies a repair against the exact current anchor before production dispatch.
pub fn validate_repair(
    repair: &RepairDescriptor,
    current_base: &BTreeMap<String, String>,
) -> EvaluationResult<()> {
    if repair.repair_hash != repair_hash(repair)? {
        return Err(EvaluationDiagnostic::new(
            EvaluationErrorCode::EvaluationTypedRepairInvalid,
            "typed repair hash mismatch",
        ));
    }
    if &repair.anchor.exact_base != current_base {
        return Err(EvaluationDiagnostic::new(
            EvaluationErrorCode::EvaluationTypedRepairStale,
            "typed repair was invalidated by an anchor change",
        )
        .repair("query a fresh compiler-owned repair descriptor"));
    }
    Ok(())
}

/// Computes the independent repair hash without trusting its retained value.
pub fn repair_hash(repair: &RepairDescriptor) -> EvaluationResult<String> {
    domain_hash_cleared(REPAIR_HASH_DOMAIN, repair, |model| {
        model.repair_hash.clear();
    })
}

fn contains_forbidden_proof_field(value: &Value) -> bool {
    match value {
        Value::Object(fields) => fields.iter().any(|(name, value)| {
            matches!(
                name.as_str(),
                "proof" | "certificate" | "alias_proof" | "lifetime_proof" | "guard"
            ) || contains_forbidden_proof_field(value)
        }),
        Value::Array(values) => values.iter().any(contains_forbidden_proof_field),
        _ => false,
    }
}
