//! Immutable compiler-owned target capability manifests for ScheduleIR.

use crate::{
    diagnostics::{AgentError, AgentResult, ErrorCode},
    ids::{TargetCapabilityId, TargetManifestId, TargetManifestRevisionId},
    memory_ir::AddressSpace,
    types::ScalarType,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, fmt, fmt::Write as _};

/// Target-manifest semantics version.
pub const TARGET_SEMANTICS_VERSION: u32 = 1;
/// Target-manifest canonical codec version.
pub const TARGET_CANONICAL_VERSION: u32 = 1;
/// Compiler-owned target validator version.
pub const TARGET_VALIDATOR_VERSION: u32 = 1;
/// Target event replay semantics version.
pub const TARGET_EVENT_SEMANTICS_VERSION: u32 = 1;
/// Domain separator for target capability identities.
pub const TARGET_HASH_DOMAIN: &[u8] = b"agentir.target.manifest.v1\0";
/// Stable name of the deterministic abstract GPU profile.
pub const GENERIC_GPU_V1: &str = "generic_gpu_v1";
/// Stable name of the executable WebGPU/WGSL v1 profile.
pub const WEBGPU_WGSL_V1: &str = "webgpu_wgsl_v1";

/// SHA-256 identity of one immutable target-manifest revision.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TargetHash(String);

impl TargetHash {
    /// Creates a hash from lowercase hexadecimal text.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Returns lowercase hexadecimal text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for TargetHash {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// Compiler-owned profile selector accepted by `target.create`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetProfile {
    /// Deterministic abstract GPU capability profile, not a real device claim.
    GenericGpuV1,
    /// Deterministic minimum WebGPU compute contract for WGSL v1 artifacts.
    #[serde(rename = "webgpu_wgsl_v1")]
    WebGpuWgslV1,
}

/// Broad execution-target category.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetKind {
    /// Abstract GPU-like hierarchy used by the deterministic simulator.
    GenericGpu,
    /// Portable WebGPU compute target emitting WGSL source.
    WebGpu,
}

/// Work-distribution hierarchy exposed by a manifest.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionHierarchy {
    /// Maximum logical grid dimensions.
    pub max_grid_dimensions: [u64; 3],
    /// Maximum local workgroup dimensions.
    pub max_workgroup_dimensions: [u64; 3],
    /// Maximum logical threads in one workgroup.
    pub max_threads_per_workgroup: u64,
    /// Whether grid/block spellings are supported.
    pub supports_grid_block: bool,
    /// Whether workgroup spellings are supported.
    pub supports_workgroup: bool,
}

/// Deterministic subgroup capability.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubgroupModel {
    /// Fixed abstract subgroup width.
    pub width: u64,
    /// Whether one axis may bind to subgroup lanes.
    pub lane_binding: bool,
}

/// Supported exact vector widths and element types.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VectorCapability {
    /// Supported vector lane counts.
    pub widths: Vec<u64>,
    /// Supported scalar element types.
    pub element_types: Vec<ScalarType>,
}

/// One supported abstract memory address space.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemorySpaceCapability {
    /// Address-space identity.
    pub address_space: AddressSpace,
    /// Minimum byte alignment accepted in the space.
    pub minimum_alignment: u64,
}

/// Conservative capacities used only by the analytical simulator.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceCapacity {
    /// Maximum abstract shared bytes per workgroup.
    pub max_shared_bytes_per_workgroup: u64,
    /// Maximum abstract private bytes per logical thread.
    pub max_private_bytes_per_thread: u64,
    /// Maximum logical iteration rank.
    pub maximum_rank: u64,
}

/// One compiler-owned capability record.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TargetCapability {
    /// Compiler-assigned capability identity.
    pub id: TargetCapabilityId,
    /// Stable capability name.
    pub name: String,
    /// Stable normalized parameters.
    pub parameters: BTreeMap<String, String>,
}

/// Lifecycle state of a target manifest.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetStatus {
    /// Compiler verification completed successfully.
    Verified,
    /// Immutable manifest is available as a schedule anchor.
    Sealed,
    /// Integrity verification rejected serialized state.
    Rejected,
}

/// Compiler-owned validation certificate for a manifest.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TargetCertificate {
    /// Stable validation method.
    pub method: String,
    /// Target semantics version.
    pub semantics_version: u32,
    /// Validator version.
    pub validator_version: u32,
    /// Ordered validated conditions.
    pub conditions: Vec<String>,
}

/// Immutable target capability contract.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TargetManifest {
    /// Manifest identity.
    pub id: TargetManifestId,
    /// Immutable revision identity.
    pub revision: TargetManifestRevisionId,
    /// Compiler-owned profile name.
    pub profile: String,
    /// Broad target category.
    pub kind: TargetKind,
    /// Execution hierarchy limits.
    pub hierarchy: ExecutionHierarchy,
    /// Subgroup behavior.
    pub subgroup: SubgroupModel,
    /// Exact vector support.
    pub vector: VectorCapability,
    /// Supported abstract memory spaces.
    pub memory_spaces: Vec<MemorySpaceCapability>,
    /// Conservative resource capacities.
    pub resources: ResourceCapacity,
    /// Compiler-owned capabilities.
    pub capabilities: Vec<TargetCapability>,
    /// Lifecycle state.
    pub status: TargetStatus,
    /// Compiler-owned validation certificate.
    pub certificate: TargetCertificate,
    /// Exact manifest hash.
    pub target_hash: TargetHash,
}

/// Immutable manifest revision wrapper retained for an explicit revision API.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TargetManifestRevision {
    /// Revision identity.
    pub id: TargetManifestRevisionId,
    /// Complete immutable manifest.
    pub manifest: TargetManifest,
}

/// Target-local monotonic allocator.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TargetAllocator {
    manifest: u64,
    revision: u64,
    capability: u64,
}

impl TargetAllocator {
    fn manifest(&mut self) -> TargetManifestId {
        self.manifest = self.manifest.saturating_add(1);
        TargetManifestId::new(format!("tm{}", self.manifest))
    }

    fn revision(&mut self) -> TargetManifestRevisionId {
        self.revision = self.revision.saturating_add(1);
        TargetManifestRevisionId::new(format!("tmr{}", self.revision))
    }

    fn capability(&mut self) -> TargetCapabilityId {
        self.capability = self.capability.saturating_add(1);
        TargetCapabilityId::new(format!("tc{}", self.capability))
    }
}

/// Replayable immutable target event.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TargetEvent {
    /// A compiler-owned target profile was instantiated.
    Created {
        /// Selected built-in profile.
        profile: TargetProfile,
        /// Expected manifest ID.
        target_manifest: TargetManifestId,
        /// Expected revision ID.
        target_revision: TargetManifestRevisionId,
        /// Expected exact hash.
        target_hash: TargetHash,
    },
}

/// Target event with its independent semantics version.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionedTargetEvent {
    /// Target event semantics version.
    pub semantics_version: u32,
    /// Replayable event.
    pub event: TargetEvent,
}

/// Read-only target summary.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TargetQuery {
    /// Manifest identity.
    pub target_manifest: TargetManifestId,
    /// Revision identity.
    pub target_revision: TargetManifestRevisionId,
    /// Exact target hash.
    pub target_hash: TargetHash,
    /// Profile name.
    pub profile: String,
    /// Lifecycle state.
    pub status: TargetStatus,
    /// Capability count.
    pub capability_count: usize,
}

/// Full compiler-owned target validation report.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TargetCheckReport {
    /// Read-only manifest summary.
    pub query: TargetQuery,
    /// Whether the capability contract is structurally valid.
    pub valid: bool,
}

/// Persistent immutable target manifests and replay history.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TargetManifestStore {
    /// Manifests by compiler-assigned identity.
    pub manifests: BTreeMap<TargetManifestId, TargetManifestRevision>,
    /// Target-local allocator.
    pub allocator: TargetAllocator,
    /// Ordered target event log.
    pub events: Vec<VersionedTargetEvent>,
}

#[derive(Serialize)]
struct TargetHashModel<'a> {
    codec: &'static str,
    version: u32,
    semantics_version: u32,
    event_semantics_version: u32,
    validator_version: u32,
    id: &'a TargetManifestId,
    revision: &'a TargetManifestRevisionId,
    profile: &'a str,
    kind: TargetKind,
    hierarchy: &'a ExecutionHierarchy,
    subgroup: &'a SubgroupModel,
    vector: &'a VectorCapability,
    memory_spaces: &'a [MemorySpaceCapability],
    resources: &'a ResourceCapacity,
    capabilities: &'a [TargetCapability],
    status: TargetStatus,
    certificate: &'a TargetCertificate,
}

fn target_error(code: ErrorCode, message: impl Into<String>) -> AgentError {
    AgentError::new(code, message)
}

fn hash_bytes(bytes: &[u8]) -> TargetHash {
    let digest = Sha256::digest(bytes);
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(output, "{byte:02x}").expect("writing to String cannot fail");
    }
    TargetHash(output)
}

/// Returns deterministic canonical bytes for one manifest.
pub fn canonical_target_bytes(manifest: &TargetManifest) -> AgentResult<Vec<u8>> {
    serde_json::to_vec(&TargetHashModel {
        codec: "agentir.target.manifest",
        version: TARGET_CANONICAL_VERSION,
        semantics_version: TARGET_SEMANTICS_VERSION,
        event_semantics_version: TARGET_EVENT_SEMANTICS_VERSION,
        validator_version: TARGET_VALIDATOR_VERSION,
        id: &manifest.id,
        revision: &manifest.revision,
        profile: &manifest.profile,
        kind: manifest.kind,
        hierarchy: &manifest.hierarchy,
        subgroup: &manifest.subgroup,
        vector: &manifest.vector,
        memory_spaces: &manifest.memory_spaces,
        resources: &manifest.resources,
        capabilities: &manifest.capabilities,
        status: manifest.status,
        certificate: &manifest.certificate,
    })
    .map_err(|error| {
        target_error(
            ErrorCode::CanonicalizationFailed,
            format!("target manifest canonicalization failed: {error}"),
        )
    })
}

/// Recomputes the exact target hash.
pub fn target_hash(manifest: &TargetManifest) -> AgentResult<TargetHash> {
    let bytes = canonical_target_bytes(manifest)?;
    let mut input = Vec::with_capacity(TARGET_HASH_DOMAIN.len().saturating_add(bytes.len()));
    input.extend_from_slice(TARGET_HASH_DOMAIN);
    input.extend_from_slice(&bytes);
    Ok(hash_bytes(&input))
}

fn build_generic_gpu_v1(
    id: TargetManifestId,
    revision: TargetManifestRevisionId,
    allocator: &mut TargetAllocator,
) -> AgentResult<TargetManifest> {
    let capability_names = [
        "serial_execution",
        "grid_block_binding",
        "workgroup_binding",
        "subgroup_lane_binding",
        "compiler_remainder",
        "exact_vectorization",
    ];
    let capabilities = capability_names
        .into_iter()
        .map(|name| TargetCapability {
            id: allocator.capability(),
            name: name.to_owned(),
            parameters: BTreeMap::new(),
        })
        .collect();
    let mut manifest = TargetManifest {
        id,
        revision,
        profile: GENERIC_GPU_V1.to_owned(),
        kind: TargetKind::GenericGpu,
        hierarchy: ExecutionHierarchy {
            max_grid_dimensions: [2_147_483_647, 65_535, 65_535],
            max_workgroup_dimensions: [1_024, 1_024, 64],
            max_threads_per_workgroup: 1_024,
            supports_grid_block: true,
            supports_workgroup: true,
        },
        subgroup: SubgroupModel {
            width: 32,
            lane_binding: true,
        },
        vector: VectorCapability {
            widths: vec![1, 2, 4, 8],
            element_types: vec![ScalarType::Bool, ScalarType::I32, ScalarType::F32],
        },
        memory_spaces: vec![
            MemorySpaceCapability {
                address_space: AddressSpace::Global,
                minimum_alignment: 4,
            },
            MemorySpaceCapability {
                address_space: AddressSpace::Shared,
                minimum_alignment: 4,
            },
            MemorySpaceCapability {
                address_space: AddressSpace::Private,
                minimum_alignment: 4,
            },
            MemorySpaceCapability {
                address_space: AddressSpace::Constant,
                minimum_alignment: 4,
            },
        ],
        resources: ResourceCapacity {
            max_shared_bytes_per_workgroup: 49_152,
            max_private_bytes_per_thread: 16_384,
            maximum_rank: 8,
        },
        capabilities,
        status: TargetStatus::Sealed,
        certificate: TargetCertificate {
            method: "compiler_owned_generic_gpu_v1".to_owned(),
            semantics_version: TARGET_SEMANTICS_VERSION,
            validator_version: TARGET_VALIDATOR_VERSION,
            conditions: vec![
                "abstract profile; no real-device conformance claim".to_owned(),
                "all capacities are deterministic compiler-owned constants".to_owned(),
            ],
        },
        target_hash: TargetHash::new("pending"),
    };
    manifest.target_hash = target_hash(&manifest)?;
    Ok(manifest)
}

fn build_webgpu_wgsl_v1(
    id: TargetManifestId,
    revision: TargetManifestRevisionId,
    allocator: &mut TargetAllocator,
) -> AgentResult<TargetManifest> {
    let capability_names = [
        "webgpu_compute",
        "wgsl_v1_source",
        "serial_execution",
        "grid_workgroup_binding",
        "global_storage_buffers",
        "uniform_parameter_block",
        "compiler_bounds_checks",
        "compiler_remainder",
        "exact_vectorization_1_2_4",
        "ordered_dispatch_graph",
    ];
    let capabilities = capability_names
        .into_iter()
        .map(|name| TargetCapability {
            id: allocator.capability(),
            name: name.to_owned(),
            parameters: BTreeMap::new(),
        })
        .collect();
    let mut manifest = TargetManifest {
        id,
        revision,
        profile: WEBGPU_WGSL_V1.to_owned(),
        kind: TargetKind::WebGpu,
        hierarchy: ExecutionHierarchy {
            max_grid_dimensions: [65_535, 65_535, 65_535],
            max_workgroup_dimensions: [256, 256, 64],
            max_threads_per_workgroup: 256,
            supports_grid_block: false,
            supports_workgroup: true,
        },
        subgroup: SubgroupModel {
            width: 1,
            lane_binding: false,
        },
        vector: VectorCapability {
            widths: vec![1, 2, 4],
            element_types: vec![ScalarType::I32, ScalarType::F32],
        },
        memory_spaces: vec![
            MemorySpaceCapability {
                address_space: AddressSpace::Global,
                minimum_alignment: 4,
            },
            MemorySpaceCapability {
                address_space: AddressSpace::Constant,
                minimum_alignment: 4,
            },
        ],
        resources: ResourceCapacity {
            max_shared_bytes_per_workgroup: 0,
            max_private_bytes_per_thread: 16_384,
            maximum_rank: 1,
        },
        capabilities,
        status: TargetStatus::Sealed,
        certificate: TargetCertificate {
            method: "compiler_owned_webgpu_wgsl_v1".to_owned(),
            semantics_version: TARGET_SEMANTICS_VERSION,
            validator_version: TARGET_VALIDATOR_VERSION,
            conditions: vec![
                "portable WebGPU minimum capability contract".to_owned(),
                "device discovery is separate from target_hash".to_owned(),
                "no subgroup, atomics, matrix, texture, or shared-cache features".to_owned(),
            ],
        },
        target_hash: TargetHash::new("pending"),
    };
    manifest.target_hash = target_hash(&manifest)?;
    Ok(manifest)
}

fn verify_manifest(manifest: &TargetManifest) -> AgentResult<()> {
    let profile_valid = match manifest.profile.as_str() {
        GENERIC_GPU_V1 => manifest.kind == TargetKind::GenericGpu,
        WEBGPU_WGSL_V1 => {
            manifest.kind == TargetKind::WebGpu
                && manifest.vector.widths == [1, 2, 4]
                && !manifest.subgroup.lane_binding
                && !manifest.hierarchy.supports_grid_block
                && manifest.hierarchy.supports_workgroup
                && manifest.resources.maximum_rank == 1
        }
        _ => false,
    };
    if !profile_valid
        || manifest.status != TargetStatus::Sealed
        || manifest.hierarchy.max_threads_per_workgroup == 0
        || manifest.subgroup.width == 0
        || manifest.resources.maximum_rank == 0
        || manifest.vector.widths.is_empty()
        || manifest
            .vector
            .widths
            .iter()
            .any(|width| *width == 0 || !width.is_power_of_two())
        || manifest
            .memory_spaces
            .iter()
            .any(|space| space.minimum_alignment == 0 || !space.minimum_alignment.is_power_of_two())
        || manifest.certificate.semantics_version != TARGET_SEMANTICS_VERSION
        || manifest.certificate.validator_version != TARGET_VALIDATOR_VERSION
    {
        return Err(target_error(
            ErrorCode::TargetCapabilityUnsupported,
            "target manifest is not a valid compiler-owned built-in capability contract",
        )
        .with_repair("create a fresh compiler-owned target manifest"));
    }
    let actual = target_hash(manifest)?;
    if actual != manifest.target_hash {
        return Err(target_error(
            ErrorCode::TargetHashMismatch,
            "target manifest hash failed recomputation",
        )
        .with_types(manifest.target_hash.to_string(), actual.to_string()));
    }
    Ok(())
}

impl TargetManifestStore {
    /// Instantiates one immutable compiler-owned target profile atomically.
    pub fn create(&mut self, profile: TargetProfile) -> AgentResult<TargetCheckReport> {
        let mut staged = self.clone();
        let id = staged.allocator.manifest();
        let revision = staged.allocator.revision();
        let manifest = match profile {
            TargetProfile::GenericGpuV1 => {
                build_generic_gpu_v1(id.clone(), revision.clone(), &mut staged.allocator)?
            }
            TargetProfile::WebGpuWgslV1 => {
                build_webgpu_wgsl_v1(id.clone(), revision.clone(), &mut staged.allocator)?
            }
        };
        verify_manifest(&manifest)?;
        let hash = manifest.target_hash.clone();
        staged.manifests.insert(
            id.clone(),
            TargetManifestRevision {
                id: revision.clone(),
                manifest,
            },
        );
        staged.events.push(VersionedTargetEvent {
            semantics_version: TARGET_EVENT_SEMANTICS_VERSION,
            event: TargetEvent::Created {
                profile,
                target_manifest: id.clone(),
                target_revision: revision.clone(),
                target_hash: hash,
            },
        });
        let report = staged.check(&id, &revision)?;
        *self = staged;
        Ok(report)
    }

    /// Lists immutable manifests in compiler-assigned order.
    #[must_use]
    pub fn list(&self) -> Vec<TargetQuery> {
        self.manifests
            .values()
            .map(|revision| query(&revision.manifest))
            .collect()
    }

    /// Returns one immutable manifest.
    pub fn manifest(
        &self,
        id: &TargetManifestId,
        revision: &TargetManifestRevisionId,
    ) -> AgentResult<&TargetManifest> {
        let value = self.manifests.get(id).ok_or_else(|| {
            target_error(
                ErrorCode::TargetManifestNotFound,
                format!("target manifest `{id}` does not exist"),
            )
        })?;
        if &value.id != revision {
            return Err(target_error(
                ErrorCode::TargetRevisionNotFound,
                format!("target revision `{revision}` does not exist for `{id}`"),
            ));
        }
        Ok(&value.manifest)
    }

    /// Returns one immutable target summary.
    pub fn query(
        &self,
        id: &TargetManifestId,
        revision: &TargetManifestRevisionId,
    ) -> AgentResult<TargetQuery> {
        Ok(query(self.manifest(id, revision)?))
    }

    /// Recomputes and verifies one manifest and its hash.
    pub fn check(
        &self,
        id: &TargetManifestId,
        revision: &TargetManifestRevisionId,
    ) -> AgentResult<TargetCheckReport> {
        let manifest = self.manifest(id, revision)?;
        verify_manifest(manifest)?;
        Ok(TargetCheckReport {
            query: query(manifest),
            valid: true,
        })
    }

    /// Verifies allocator-independent state and deterministic event replay.
    pub fn verify_all(&self) -> AgentResult<()> {
        let mut replayed = Self::default();
        for versioned in &self.events {
            if versioned.semantics_version != TARGET_EVENT_SEMANTICS_VERSION {
                return Err(target_error(
                    ErrorCode::ScheduleEventOrderInvalid,
                    "unsupported target event semantics version",
                ));
            }
            let TargetEvent::Created {
                profile,
                target_manifest,
                target_revision,
                target_hash,
            } = &versioned.event;
            let actual = replayed.create(*profile)?;
            if actual.query.target_manifest != *target_manifest
                || actual.query.target_revision != *target_revision
                || actual.query.target_hash != *target_hash
            {
                return Err(target_error(
                    ErrorCode::ReplayMismatch,
                    "target event replay diverged",
                ));
            }
        }
        if &replayed != self {
            return Err(target_error(
                ErrorCode::ReplayMismatch,
                "replayed TargetManifestStore differs from snapshot",
            ));
        }
        for revision in self.manifests.values() {
            verify_manifest(&revision.manifest)?;
        }
        Ok(())
    }
}

fn query(manifest: &TargetManifest) -> TargetQuery {
    TargetQuery {
        target_manifest: manifest.id.clone(),
        target_revision: manifest.revision.clone(),
        target_hash: manifest.target_hash.clone(),
        profile: manifest.profile.clone(),
        status: manifest.status,
        capability_count: manifest.capabilities.len(),
    }
}
