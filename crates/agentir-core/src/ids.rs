//! Persistent compiler-assigned identifiers.

use serde::{Deserialize, Serialize};
use std::fmt;

macro_rules! identifier {
    ($name:ident) => {
        #[doc = concat!("A persistent `", stringify!($name), "` assigned by the compiler core.")]
        #[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            /// Creates an identifier from its canonical string representation.
            #[must_use]
            pub fn new(value: impl Into<String>) -> Self {
                Self(value.into())
            }

            /// Returns the canonical string representation.
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(formatter)
            }
        }
    };
}

identifier!(WorkspaceId);
identifier!(RevisionId);
identifier!(TransactionId);
identifier!(ActionId);
identifier!(OperationId);
identifier!(ValueId);
identifier!(DimensionId);
identifier!(HoleId);
identifier!(ObligationId);
identifier!(ContinuationFrameId);
identifier!(CandidateId);
identifier!(CandidateRevisionId);
identifier!(ImplOperationId);
identifier!(ImplValueId);
identifier!(EvidenceId);
identifier!(CandidateObligationId);
identifier!(ProposalId);
identifier!(EqualitySpaceId);
identifier!(EqualityRevisionId);
identifier!(EqualityNodeId);
identifier!(EqualityEdgeId);
identifier!(MemoryPlanId);
identifier!(MemoryRevisionId);
identifier!(BufferId);
identifier!(MemoryOperationId);
identifier!(AliasDomainId);
identifier!(MemoryObligationId);
identifier!(MemoryEvidenceId);
identifier!(MemoryGuardId);
identifier!(TargetManifestId);
identifier!(TargetManifestRevisionId);
identifier!(TargetCapabilityId);
identifier!(SchedulePlanId);
identifier!(ScheduleRevisionId);
identifier!(ScheduleNodeId);
identifier!(ScheduleAxisId);
identifier!(ScheduleOperationId);
identifier!(ScheduleObligationId);
identifier!(ScheduleEvidenceId);
identifier!(BackendPlanId);
identifier!(BackendRevisionId);
identifier!(BackendKernelId);
identifier!(BackendValueId);
identifier!(BackendObligationId);
identifier!(BackendEvidenceId);
identifier!(ArtifactId);
identifier!(ArtifactModuleId);
identifier!(MeasurementId);
identifier!(CpuArtifactId);
identifier!(CpuMeasurementId);

/// Monotonic identifier allocator owned by one workspace.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdAllocator {
    revision: u64,
    transaction: u64,
    action: u64,
    operation: u64,
    value: u64,
    dimension: u64,
    hole: u64,
    obligation: u64,
    frame: u64,
}

macro_rules! allocator_method {
    ($method:ident, $field:ident, $prefix:literal, $kind:ident) => {
        #[doc = concat!("Allocates the next `", stringify!($kind), "`.")]
        pub fn $method(&mut self) -> $kind {
            self.$field += 1;
            $kind::new(format!(concat!($prefix, "{}"), self.$field))
        }
    };
}

impl IdAllocator {
    allocator_method!(revision, revision, "r", RevisionId);
    allocator_method!(transaction, transaction, "tx", TransactionId);
    allocator_method!(action, action, "a", ActionId);
    allocator_method!(operation, operation, "op", OperationId);
    allocator_method!(value, value, "v", ValueId);
    allocator_method!(dimension, dimension, "d", DimensionId);
    allocator_method!(hole, hole, "h", HoleId);
    allocator_method!(obligation, obligation, "o", ObligationId);
    allocator_method!(frame, frame, "cf", ContinuationFrameId);

    /// Compares counters that affect persistent graph and revision identities.
    #[must_use]
    pub fn same_persistent_state(&self, other: &Self) -> bool {
        self.revision == other.revision
            && self.transaction == other.transaction
            && self.action == other.action
            && self.operation == other.operation
            && self.value == other.value
            && self.dimension == other.dimension
            && self.hole == other.hole
            && self.obligation == other.obligation
    }
}
