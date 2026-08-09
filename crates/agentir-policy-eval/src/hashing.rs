//! Domain-separated evaluation hashing.

use crate::model::{
    EvaluationAggregate, EvaluationArchive, EvaluationCorpus, EvaluationDiagnostic,
    EvaluationEpisode, EvaluationErrorCode, EvaluationObservation, EvaluationResult, EvaluationRun,
    PolicyDescriptor,
};
use serde::Serialize;
use sha2::{Digest, Sha256};

/// Corpus canonical hash domain.
pub const CORPUS_HASH_DOMAIN: &[u8] = b"agentir.evaluation.corpus.v1\0";
/// Policy canonical hash domain.
pub const POLICY_HASH_DOMAIN: &[u8] = b"agentir.evaluation.policy.v1\0";
/// Observation canonical hash domain.
pub const OBSERVATION_HASH_DOMAIN: &[u8] = b"agentir.evaluation.observation.v1\0";
/// Episode canonical hash domain.
pub const EPISODE_HASH_DOMAIN: &[u8] = b"agentir.evaluation.episode.v1\0";
/// Aggregate/evaluation canonical hash domain.
pub const AGGREGATE_HASH_DOMAIN: &[u8] = b"agentir.evaluation.aggregate.v1\0";
/// Separate evaluation archive hash domain.
pub const ARCHIVE_HASH_DOMAIN: &[u8] = b"agentir.evaluation.archive.v1\0";

fn canonical<T: Serialize>(value: &T) -> EvaluationResult<Vec<u8>> {
    serde_json::to_vec(value).map_err(|error| {
        EvaluationDiagnostic::new(
            EvaluationErrorCode::EvaluationHashMismatch,
            format!("canonical evaluation encoding failed: {error}"),
        )
    })
}

/// Hashes deterministic JSON bytes using a distinct domain prefix.
pub fn domain_hash<T: Serialize>(domain: &[u8], value: &T) -> EvaluationResult<String> {
    let bytes = canonical(value)?;
    let mut hasher = Sha256::new();
    hasher.update(domain);
    hasher.update(bytes);
    Ok(format!("{:x}", hasher.finalize()))
}

pub(crate) fn corpus_hash(corpus: &EvaluationCorpus) -> EvaluationResult<String> {
    let mut model = corpus.clone();
    model.corpus_hash.clear();
    domain_hash(CORPUS_HASH_DOMAIN, &model)
}

pub(crate) fn policy_hash(policy: &PolicyDescriptor) -> EvaluationResult<String> {
    let mut model = policy.clone();
    model.policy_hash.clear();
    domain_hash(POLICY_HASH_DOMAIN, &model)
}

pub(crate) fn observation_hash(observation: &EvaluationObservation) -> EvaluationResult<String> {
    let mut model = observation.clone();
    model.observation_hash.clear();
    domain_hash(OBSERVATION_HASH_DOMAIN, &model)
}

pub(crate) fn episode_hash(episode: &EvaluationEpisode) -> EvaluationResult<String> {
    let mut model = episode.clone();
    model.episode_hash = None;
    domain_hash(EPISODE_HASH_DOMAIN, &model)
}

pub(crate) fn evaluation_hash(run: &EvaluationRun) -> EvaluationResult<String> {
    let model = (
        &run.corpus_hash,
        &run.policy.policy_hash,
        &run.compiler_build_hash,
        &run.seeds,
        run.episodes
            .iter()
            .map(|episode| episode.episode_hash.as_deref())
            .collect::<Vec<_>>(),
    );
    domain_hash(AGGREGATE_HASH_DOMAIN, &model)
}

pub(crate) fn aggregate_hash(aggregate: &EvaluationAggregate) -> EvaluationResult<String> {
    let mut model = aggregate.clone();
    model.aggregate_hash.clear();
    domain_hash(AGGREGATE_HASH_DOMAIN, &model)
}

pub(crate) fn archive_hash(archive: &EvaluationArchive) -> EvaluationResult<String> {
    let mut model = archive.clone();
    model.archive_hash.clear();
    domain_hash(ARCHIVE_HASH_DOMAIN, &model)
}
