//! Versioned resumable pagination for deterministic production enumerations.

use crate::{
    hashing::domain_hash,
    model::{EvaluationDiagnostic, EvaluationErrorCode, EvaluationResult},
    ranking::{EvaluationChoice, EvaluationChoiceSet},
    work::WorkUnitCounters,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::BTreeMap;

/// Continuation digest domain, independent from choice-set identity.
pub const CONTINUATION_DIGEST_DOMAIN: &[u8] = b"agentir.evaluation.continuation.v1\0";
const CURSOR_PREFIX: &str = "agentir-cursor-v1.";

/// Stable kind of compiler-owned enumeration being paged.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EnumerationKind {
    /// A complete Stage 6B production choice set.
    RankingChoices,
    /// Compiler-owned typed repairs.
    TypedRepairs,
    /// Dataset extraction from retained ranking frames.
    DatasetExamples,
}

/// Whether the returned frame is complete or a bounded prefix/middle page.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FrameCompleteness {
    /// Every item in the exact enumeration is present.
    Complete,
    /// More exact items exist and require resumption.
    Bounded,
}

/// Stable page exhaustion state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContinuationStatus {
    /// The exact enumeration was exhausted by this page.
    Exhausted,
    /// A later page remains available.
    NotExhausted,
}

/// Exact immutable anchors required to resume an enumeration.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContinuationAnchors {
    /// Stable workspace/run/layer locator.
    pub locator: String,
    /// Exact revisions and independent hashes in deterministic key order.
    pub revisions_and_hashes: BTreeMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct CursorPayload {
    version: u32,
    kind: EnumerationKind,
    anchors: ContinuationAnchors,
    choice_set_hash: String,
    next_offset: u64,
    total_count: u64,
}

/// Opaque compiler-owned continuation token.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ContinuationCursor(pub String);

/// One bounded page over an exact ordered Stage 6B choice set.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChoiceContinuationPage {
    /// Exact immutable anchors.
    pub anchors: ContinuationAnchors,
    /// Stable enumeration kind.
    pub kind: EnumerationKind,
    /// Configured total-item limit.
    pub total_limit: u64,
    /// Configured per-page work limit.
    pub work_limit: u64,
    /// Number of choices returned in this page.
    pub returned_count: u64,
    /// Complete versus bounded frame semantics.
    pub completeness: FrameCompleteness,
    /// Whether the exact enumeration is exhausted.
    pub status: ContinuationStatus,
    /// Opaque cursor for the next page.
    pub cursor: Option<ContinuationCursor>,
    /// Cursor codec version.
    pub cursor_version: u32,
    /// Independent digest of the page envelope.
    pub continuation_digest: String,
    /// Stable non-semantic work counts.
    pub work_units: WorkUnitCounters,
    /// Exact choices, retaining their one-shot identities and compiler order.
    pub choices: Vec<EvaluationChoice>,
}

#[derive(Serialize)]
struct PageDigest<'a> {
    anchors: &'a ContinuationAnchors,
    kind: EnumerationKind,
    total_limit: u64,
    work_limit: u64,
    returned_count: u64,
    completeness: FrameCompleteness,
    status: ContinuationStatus,
    cursor: &'a Option<ContinuationCursor>,
    cursor_version: u32,
    choices: &'a [EvaluationChoice],
}

/// Returns one deterministic page. Failed resumption publishes no page.
pub fn paginate_choice_set(
    choice_set: &EvaluationChoiceSet,
    anchors: ContinuationAnchors,
    page_size: u64,
    total_limit: u64,
    work_limit: u64,
    cursor: Option<&ContinuationCursor>,
) -> EvaluationResult<ChoiceContinuationPage> {
    let total_count = u64::try_from(choice_set.choices.len()).unwrap_or(u64::MAX);
    if total_count > total_limit {
        return Err(EvaluationDiagnostic::new(
            EvaluationErrorCode::EvaluationContinuationLimitExceeded,
            "continuation total-item limit exceeded",
        )
        .expected_actual(json!(total_limit), json!(total_count)));
    }
    if page_size > work_limit {
        return Err(EvaluationDiagnostic::new(
            EvaluationErrorCode::EvaluationContinuationLimitExceeded,
            "continuation page work limit exceeded",
        )
        .expected_actual(json!(work_limit), json!(page_size)));
    }
    let start = if let Some(cursor) = cursor {
        let payload = decode_cursor(cursor)?;
        if payload.kind != EnumerationKind::RankingChoices
            || payload.anchors != anchors
            || payload.choice_set_hash != choice_set.choice_set_hash
            || payload.total_count != total_count
        {
            return Err(EvaluationDiagnostic::new(
                EvaluationErrorCode::EvaluationContinuationStale,
                "continuation cursor does not match the exact enumeration anchors",
            )
            .repair("restart the query from its current exact anchor"));
        }
        payload.next_offset
    } else {
        0
    };
    if start > total_count {
        return Err(EvaluationDiagnostic::new(
            EvaluationErrorCode::EvaluationContinuationCorrupt,
            "continuation cursor offset is outside the exact enumeration",
        ));
    }
    let end = start.saturating_add(page_size).min(total_count);
    let start_index = usize::try_from(start).unwrap_or(usize::MAX);
    let end_index = usize::try_from(end).unwrap_or(usize::MAX);
    let choices = choice_set.choices[start_index..end_index].to_vec();
    let exhausted = end == total_count;
    let next_cursor = if exhausted {
        None
    } else {
        Some(encode_cursor(&CursorPayload {
            version: 1,
            kind: EnumerationKind::RankingChoices,
            anchors: anchors.clone(),
            choice_set_hash: choice_set.choice_set_hash.clone(),
            next_offset: end,
            total_count,
        })?)
    };
    let mut work_units = WorkUnitCounters {
        descriptor_query: 1,
        stable_id_assignment: 0,
        canonical_encoding: u64::from(next_cursor.is_some()),
        hashing: u64::from(next_cursor.is_some()),
        ..WorkUnitCounters::default()
    };
    work_units.sorting_deduplication = u64::try_from(choices.len()).unwrap_or(u64::MAX);
    work_units.validate_limit(work_limit.saturating_add(2))?;
    let status = if exhausted {
        ContinuationStatus::Exhausted
    } else {
        ContinuationStatus::NotExhausted
    };
    let completeness = if start == 0 && exhausted {
        FrameCompleteness::Complete
    } else {
        FrameCompleteness::Bounded
    };
    let digest_model = PageDigest {
        anchors: &anchors,
        kind: EnumerationKind::RankingChoices,
        total_limit,
        work_limit,
        returned_count: u64::try_from(choices.len()).unwrap_or(u64::MAX),
        completeness,
        status,
        cursor: &next_cursor,
        cursor_version: 1,
        choices: &choices,
    };
    let continuation_digest = domain_hash(CONTINUATION_DIGEST_DOMAIN, &digest_model)?;
    Ok(ChoiceContinuationPage {
        anchors,
        kind: EnumerationKind::RankingChoices,
        total_limit,
        work_limit,
        returned_count: u64::try_from(choices.len()).unwrap_or(u64::MAX),
        completeness,
        status,
        cursor: next_cursor,
        cursor_version: 1,
        continuation_digest,
        work_units,
        choices,
    })
}

fn encode_cursor(payload: &CursorPayload) -> EvaluationResult<ContinuationCursor> {
    let bytes = serde_json::to_vec(payload).map_err(|error| {
        EvaluationDiagnostic::new(
            EvaluationErrorCode::EvaluationContinuationCorrupt,
            format!("continuation cursor encoding failed: {error}"),
        )
    })?;
    let digest = domain_hash(CONTINUATION_DIGEST_DOMAIN, payload)?;
    Ok(ContinuationCursor(format!(
        "{CURSOR_PREFIX}{}.{}",
        hex_encode(&bytes),
        digest
    )))
}

fn decode_cursor(cursor: &ContinuationCursor) -> EvaluationResult<CursorPayload> {
    let Some(rest) = cursor.0.strip_prefix(CURSOR_PREFIX) else {
        return Err(cursor_error(
            "unsupported or future continuation cursor version",
        ));
    };
    let Some((encoded, retained_digest)) = rest.rsplit_once('.') else {
        return Err(cursor_error("malformed continuation cursor"));
    };
    let bytes = hex_decode(encoded)?;
    let payload: CursorPayload = serde_json::from_slice(&bytes)
        .map_err(|_| cursor_error("malformed continuation cursor payload"))?;
    if payload.version != 1 {
        return Err(cursor_error(
            "unsupported or future continuation cursor version",
        ));
    }
    if domain_hash(CONTINUATION_DIGEST_DOMAIN, &payload)? != retained_digest {
        return Err(cursor_error("continuation cursor digest mismatch"));
    }
    Ok(payload)
}

fn cursor_error(message: &str) -> EvaluationDiagnostic {
    EvaluationDiagnostic::new(EvaluationErrorCode::EvaluationContinuationCorrupt, message)
}

fn hex_encode(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        output.push(char::from(DIGITS[usize::from(byte >> 4)]));
        output.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    output
}

fn hex_decode(encoded: &str) -> EvaluationResult<Vec<u8>> {
    if encoded.len() % 2 != 0 {
        return Err(cursor_error(
            "continuation cursor has invalid hexadecimal bytes",
        ));
    }
    encoded
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let high = hex_digit(pair[0])?;
            let low = hex_digit(pair[1])?;
            Ok((high << 4) | low)
        })
        .collect()
}

fn hex_digit(value: u8) -> EvaluationResult<u8> {
    match value {
        b'0'..=b'9' => Ok(value - b'0'),
        b'a'..=b'f' => Ok(value - b'a' + 10),
        _ => Err(cursor_error(
            "continuation cursor has invalid hexadecimal bytes",
        )),
    }
}
