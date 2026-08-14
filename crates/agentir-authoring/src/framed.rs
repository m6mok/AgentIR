//! Task-specific framed staged authoring with compiler-owned addressing.

use super::{
    AuthoringError, AuthoringErrorCode, AuthoringGateway, AuthoringTask, ExecutionMode,
    GraphOpcode, GraphOperand, GraphProposal, MAX_OPERATIONS, StagedOperand, StagedOperation,
    StagedProposal, compile_staged, failure, require_keys, require_object, valid_binding,
    with_repair_hint,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

/// Exact schema identifier for a compiler-owned public authoring frame.
pub const AUTHORING_FRAME_SCHEMA: &str = "agentir.elementwise_authoring_frame.v2";

/// Exact schema identifier for the model-authored framed staged response.
pub const FRAMED_STAGED_SCHEMA: &str = "agentir.elementwise_framed_staged.v2";

/// Generic machine-readable schema for framed staged responses.
///
/// A concrete frame additionally produces a task-specific schema with exact
/// slot and role enums through [`AuthoringFrame::response_json_schema`].
pub const FRAMED_STAGED_JSON_SCHEMA: &str =
    include_str!("../../../schemas/agentir-elementwise-framed-staged-v2.schema.json");

/// Machine-readable schema for compiler-owned public frames.
pub const AUTHORING_FRAME_JSON_SCHEMA: &str =
    include_str!("../../../schemas/agentir-elementwise-authoring-frame-v2.schema.json");

/// Short instruction for a model filling one compiler-owned frame.
pub const FRAMED_STAGED_MODEL_INSTRUCTION: &str = "Return exactly one JSON object matching the supplied task-specific JSON Schema. Copy task_id and frame_hash exactly. choices contains exactly one entry for every frame slot; each entry selects one exact add/mul/fma opcode and ordered named operand roles from that slot's menu. state is one allowed slot ID. Do not expand stages, calculate graph indices, repeat cycle parameters or warmup values, invent roles, use aliases, decompose fma, add extra text, or return any server inputs, compiler IDs, hashes other than frame_hash, guards, certificates, source, or backend settings.";

/// Public declarations available while a frame is constructed.
///
/// This deliberately excludes runtime inputs and hidden exact intent, making
/// the frame capability boundary explicit and independently testable.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublicAuthoringDeclarations {
    /// Stable public task identity.
    pub task_id: String,
    /// Ordered public scalar capture names.
    pub scalars: Vec<String>,
    /// Ordered public tensor capture names.
    pub tensors: Vec<String>,
}

impl From<&AuthoringTask> for PublicAuthoringDeclarations {
    fn from(task: &AuthoringTask) -> Self {
        Self {
            task_id: task.task_id.clone(),
            scalars: task.scalars.clone(),
            tensors: task.tensors.clone(),
        }
    }
}

/// One compiler-owned operand role exposed by a public frame.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum FrameRole {
    /// One declared scalar capture.
    Scalar {
        /// Exact capture name.
        name: String,
    },
    /// One declared tensor capture.
    Tensor {
        /// Exact capture name.
        name: String,
    },
    /// Result of an earlier named body slot in the same stage.
    StageLocal {
        /// Earlier slot ID without a `$` prefix.
        slot: String,
    },
    /// Previous stage state, using the seed at stage zero.
    StatePrev,
    /// Fixed state lag with an explicit compiler-owned warmup prefix.
    StateLag {
        /// Positive recurrence lag.
        stages: usize,
        /// Complete public warmup prefix; its length may exceed the lag.
        initial: Vec<GraphOperand>,
    },
    /// Compiler-owned scalar capture cycle.
    ScalarCycle {
        /// Exact public prefix.
        prefix: String,
        /// Positive public capture count.
        count: usize,
        /// Public stage multiplier.
        stride: usize,
        /// Public cycle offset.
        offset: usize,
    },
    /// Compiler-owned tensor capture cycle.
    TensorCycle {
        /// Exact public prefix.
        prefix: String,
        /// Positive public capture count.
        count: usize,
        /// Public stage multiplier.
        stride: usize,
        /// Public cycle offset.
        offset: usize,
    },
}

/// One opcode-specific menu for a body slot.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrameOpcodeMenu {
    /// Opcode available in this menu branch.
    pub op: GraphOpcode,
    /// Allowed role IDs for each ordered operand position.
    pub operand_roles: Vec<Vec<String>>,
}

/// One mechanically positioned body slot in a public frame.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrameSlot {
    /// Stable slot ID without a `$` prefix.
    pub id: String,
    /// Opcode and ordered operand-role menus.
    pub menus: Vec<FrameOpcodeMenu>,
}

/// Public, unhashed source used to construct an immutable frame.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthoringFrameBlueprint {
    /// Positive stage count fixed by the public task.
    pub stages: usize,
    /// Scalar or tensor role used as the stage-zero state.
    pub seed_role: String,
    /// Named public roles. `BTreeMap` fixes canonical ordering.
    pub roles: BTreeMap<String, FrameRole>,
    /// One to eight ordered body slots.
    pub slots: Vec<FrameSlot>,
    /// Slot IDs permitted as the per-stage state and final yield.
    pub state_candidates: Vec<String>,
}

/// Immutable compiler-owned public authoring frame.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthoringFrame {
    /// Must equal [`AUTHORING_FRAME_SCHEMA`].
    pub schema: String,
    /// Exact public task identity.
    pub task_id: String,
    /// Deterministic SHA-256 identity of every other frame field.
    pub frame_hash: String,
    /// Positive public stage count.
    pub stages: usize,
    /// Scalar or tensor role used before stage zero.
    pub seed_role: String,
    /// Named public roles with compiler-owned addressing details.
    pub roles: BTreeMap<String, FrameRole>,
    /// Mechanically ordered body slots.
    pub slots: Vec<FrameSlot>,
    /// Allowed state slot IDs.
    pub state_candidates: Vec<String>,
    /// Hard cap on expanded ordinary graph operations.
    pub expanded_operation_limit: usize,
}

/// One model choice for one compiler-owned body slot.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FramedOperationChoice {
    /// Exact slot ID from the frame.
    pub slot: String,
    /// Selected exact opcode.
    pub op: GraphOpcode,
    /// Ordered role IDs, one for each operand.
    pub operands: Vec<String>,
}

/// Compact model-authored response bound to one immutable frame.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FramedStagedProposal {
    /// Must equal [`FRAMED_STAGED_SCHEMA`].
    pub schema: String,
    /// Exact task identity copied from the frame.
    pub task_id: String,
    /// Exact immutable frame identity copied from the frame.
    pub frame_hash: String,
    /// Exactly one semantic choice per frame slot.
    pub choices: Vec<FramedOperationChoice>,
    /// One allowed slot ID used as every stage state and the final yield.
    pub state: String,
}

/// Typed deterministic surface recommendation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SurfaceRecommendation {
    /// A short, auditable explicit DAG should use graph v1.
    Graph,
    /// A long irregular DAG should use incremental batch v1.
    IncrementalBatch,
    /// A regular recurrence should use one generated framed staged v2 surface.
    FramedStaged(AuthoringFrame),
}

/// Public problem shape used before any model call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PublicProblemShape {
    /// A public exact DAG with a known operation count.
    Dag {
        /// Exact prescribed operation count.
        operations: usize,
        /// Whether the complete DAG is short enough for direct index audit.
        auditable: bool,
        /// Whether public intent fixes exact operation and operand order.
        exact_order_prescribed: bool,
    },
    /// A regular public recurrence represented by a frame blueprint.
    RegularRecurrence {
        /// Public frame source.
        blueprint: AuthoringFrameBlueprint,
        /// Whether public intent fixes exact operation and operand order.
        exact_order_prescribed: bool,
    },
}

/// Typed pre-model-call capability rejection.
#[derive(Clone, Debug, PartialEq)]
pub enum RecommendationError {
    /// Public intent does not prescribe one exact structural program.
    AmbiguousIntent,
    /// The public problem exceeds the bounded authoring capability.
    UnsupportedCapability(AuthoringError),
}

impl std::fmt::Display for RecommendationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AmbiguousIntent => write!(formatter, "public intent is structurally ambiguous"),
            Self::UnsupportedCapability(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for RecommendationError {}

impl AuthoringFrame {
    /// Returns a task-specific strict response schema with exact slot/role enums.
    #[must_use]
    pub fn response_json_schema(&self) -> Value {
        let choices = self
            .slots
            .iter()
            .map(|slot| {
                let branches = slot
                    .menus
                    .iter()
                    .map(|menu| {
                        let operands = menu
                            .operand_roles
                            .iter()
                            .map(|roles| json!({"type":"string","enum":roles}))
                            .collect::<Vec<_>>();
                        json!({
                            "type":"object",
                            "additionalProperties":false,
                            "required":["slot","op","operands"],
                            "properties":{
                                "slot":{"type":"string","const":slot.id},
                                "op":{"type":"string","const":opcode_name(menu.op)},
                                "operands":{
                                    "type":"array",
                                    "prefixItems":operands,
                                    "items":false,
                                    "minItems":menu.operand_roles.len(),
                                    "maxItems":menu.operand_roles.len()
                                }
                            }
                        })
                    })
                    .collect::<Vec<_>>();
                json!({"oneOf":branches})
            })
            .collect::<Vec<_>>();
        json!({
            "$schema":"https://json-schema.org/draft/2020-12/schema",
            "$id":format!("agentir-elementwise-framed-staged-v2-{}.schema.json",self.frame_hash),
            "title":"AgentIR task-specific framed staged response v2",
            "type":"object",
            "additionalProperties":false,
            "required":["schema","task_id","frame_hash","choices","state"],
            "properties":{
                "schema":{"type":"string","const":FRAMED_STAGED_SCHEMA},
                "task_id":{"type":"string","const":self.task_id},
                "frame_hash":{"type":"string","const":self.frame_hash},
                "choices":{
                    "type":"array",
                    "prefixItems":choices,
                    "items":false,
                    "minItems":self.slots.len(),
                    "maxItems":self.slots.len()
                },
                "state":{"type":"string","enum":self.state_candidates}
            }
        })
    }
}

/// Builds and hashes one frame using public declarations and a public blueprint.
pub fn build_authoring_frame(
    declarations: &PublicAuthoringDeclarations,
    blueprint: &AuthoringFrameBlueprint,
) -> Result<AuthoringFrame, AuthoringError> {
    validate_blueprint(declarations, blueprint)?;
    let frame_hash = frame_hash(
        &declarations.task_id,
        blueprint.stages,
        &blueprint.seed_role,
        &blueprint.roles,
        &blueprint.slots,
        &blueprint.state_candidates,
    );
    Ok(AuthoringFrame {
        schema: AUTHORING_FRAME_SCHEMA.to_owned(),
        task_id: declarations.task_id.clone(),
        frame_hash,
        stages: blueprint.stages,
        seed_role: blueprint.seed_role.clone(),
        roles: blueprint.roles.clone(),
        slots: blueprint.slots.clone(),
        state_candidates: blueprint.state_candidates.clone(),
        expanded_operation_limit: MAX_OPERATIONS,
    })
}

/// Verifies that a supplied frame is exactly the compiler-generated frame for
/// its public declarations and embedded blueprint.
pub fn verify_authoring_frame(
    declarations: &PublicAuthoringDeclarations,
    frame: &AuthoringFrame,
) -> Result<(), AuthoringError> {
    if frame.schema != AUTHORING_FRAME_SCHEMA || frame.expanded_operation_limit != MAX_OPERATIONS {
        return Err(framed_failure(
            AuthoringErrorCode::SchemaRejected,
            "$.frame.schema",
            json!({"schema":AUTHORING_FRAME_SCHEMA,"expanded_operation_limit":MAX_OPERATIONS}),
            json!({"schema":frame.schema,"expanded_operation_limit":frame.expanded_operation_limit}),
        ));
    }
    if frame.task_id != declarations.task_id {
        return Err(framed_failure(
            AuthoringErrorCode::ValidationRejected,
            "$.frame.task_id",
            json!(declarations.task_id),
            json!(frame.task_id),
        ));
    }
    let blueprint = AuthoringFrameBlueprint {
        stages: frame.stages,
        seed_role: frame.seed_role.clone(),
        roles: frame.roles.clone(),
        slots: frame.slots.clone(),
        state_candidates: frame.state_candidates.clone(),
    };
    let expected = build_authoring_frame(declarations, &blueprint)?;
    if &expected != frame {
        return Err(framed_failure(
            AuthoringErrorCode::ValidationRejected,
            "$.frame.frame_hash",
            json!(expected.frame_hash),
            json!(frame.frame_hash),
        ));
    }
    Ok(())
}

/// Chooses one authoring surface from public facts before a model call.
pub fn recommend_surface(
    declarations: &PublicAuthoringDeclarations,
    shape: &PublicProblemShape,
) -> Result<SurfaceRecommendation, RecommendationError> {
    match shape {
        PublicProblemShape::Dag {
            operations,
            auditable,
            exact_order_prescribed,
        } => {
            if !exact_order_prescribed {
                return Err(RecommendationError::AmbiguousIntent);
            }
            if *operations == 0 || *operations > MAX_OPERATIONS {
                return Err(RecommendationError::UnsupportedCapability(failure(
                    AuthoringErrorCode::ValidationRejected,
                    "$.operations",
                    json!({"min":1,"max":MAX_OPERATIONS}),
                    json!(operations),
                )));
            }
            if *auditable && *operations <= 16 {
                Ok(SurfaceRecommendation::Graph)
            } else {
                Ok(SurfaceRecommendation::IncrementalBatch)
            }
        }
        PublicProblemShape::RegularRecurrence {
            blueprint,
            exact_order_prescribed,
        } => {
            if !exact_order_prescribed {
                return Err(RecommendationError::AmbiguousIntent);
            }
            build_authoring_frame(declarations, blueprint)
                .map(SurfaceRecommendation::FramedStaged)
                .map_err(RecommendationError::UnsupportedCapability)
        }
    }
}

/// Strictly parses one bare framed staged response.
pub fn parse_framed_staged(input: &str) -> Result<FramedStagedProposal, AuthoringError> {
    let value: Value = serde_json::from_str(input).map_err(|error| {
        framed_failure(
            AuthoringErrorCode::SchemaRejected,
            "$",
            json!("strict agentir.elementwise_framed_staged.v2 object"),
            json!(error.to_string()),
        )
    })?;
    validate_framed_shape(&value)?;
    serde_json::from_value(value).map_err(|error| {
        framed_failure(
            AuthoringErrorCode::SchemaRejected,
            "$",
            json!("strict agentir.elementwise_framed_staged.v2 object"),
            json!(error.to_string()),
        )
    })
}

/// Deterministically lowers a frame-bound response to an ordinary graph proposal.
pub fn compile_framed_staged(
    frame: &AuthoringFrame,
    source: &FramedStagedProposal,
) -> Result<GraphProposal, AuthoringError> {
    compile_framed_inner(frame, source).map_err(framed_hint)
}

impl AuthoringGateway {
    /// Lowers a frame-bound response and then uses the unchanged graph publication path.
    pub fn publish_framed_staged(
        &mut self,
        task: &AuthoringTask,
        frame: &AuthoringFrame,
        source: &FramedStagedProposal,
        mode: ExecutionMode,
    ) -> Result<super::AuthoringResult, AuthoringError> {
        verify_authoring_frame(&PublicAuthoringDeclarations::from(task), frame)?;
        let proposal = compile_framed_staged(frame, source)?;
        self.publish(task, &proposal, mode)
    }
}

fn compile_framed_inner(
    frame: &AuthoringFrame,
    source: &FramedStagedProposal,
) -> Result<GraphProposal, AuthoringError> {
    if source.schema != FRAMED_STAGED_SCHEMA {
        return Err(failure(
            AuthoringErrorCode::SchemaRejected,
            "$.schema",
            json!(FRAMED_STAGED_SCHEMA),
            json!(source.schema),
        ));
    }
    if source.task_id != frame.task_id {
        return Err(failure(
            AuthoringErrorCode::ValidationRejected,
            "$.task_id",
            json!(frame.task_id),
            json!(source.task_id),
        ));
    }
    if source.frame_hash != frame.frame_hash {
        return Err(failure(
            AuthoringErrorCode::ValidationRejected,
            "$.frame_hash",
            json!(frame.frame_hash),
            json!(source.frame_hash),
        ));
    }
    if source.choices.len() != frame.slots.len() {
        return Err(failure(
            AuthoringErrorCode::ValidationRejected,
            "$.choices",
            json!({"length":frame.slots.len()}),
            json!({"length":source.choices.len()}),
        ));
    }
    let mut choices = BTreeMap::new();
    for (index, choice) in source.choices.iter().enumerate() {
        if choices.insert(choice.slot.as_str(), choice).is_some() {
            return Err(failure(
                AuthoringErrorCode::ValidationRejected,
                format!("$.choices[{index}].slot"),
                json!("unique frame slot"),
                json!(choice.slot),
            ));
        }
    }
    let state = frame
        .state_candidates
        .iter()
        .find(|candidate| *candidate == &source.state)
        .ok_or_else(|| {
            failure(
                AuthoringErrorCode::ValidationRejected,
                "$.state",
                json!(frame.state_candidates),
                json!(source.state),
            )
        })?;
    let seed = frame.roles.get(&frame.seed_role).ok_or_else(|| {
        failure(
            AuthoringErrorCode::ValidationRejected,
            "$.frame.seed_role",
            json!("known scalar or tensor role"),
            json!(frame.seed_role),
        )
    })?;
    let seed = direct_graph_operand(seed).ok_or_else(|| {
        failure(
            AuthoringErrorCode::ValidationRejected,
            "$.frame.seed_role",
            json!("scalar or tensor role"),
            json!(frame.seed_role),
        )
    })?;
    let mut body = Vec::with_capacity(frame.slots.len());
    for (slot_index, slot) in frame.slots.iter().enumerate() {
        let choice = choices.get(slot.id.as_str()).ok_or_else(|| {
            failure(
                AuthoringErrorCode::ValidationRejected,
                "$.choices",
                json!({"required_slot":slot.id}),
                json!(choices.keys().collect::<Vec<_>>()),
            )
        })?;
        let menu = slot
            .menus
            .iter()
            .find(|menu| menu.op == choice.op)
            .ok_or_else(|| {
                failure(
                    AuthoringErrorCode::ValidationRejected,
                    format!("$.choices[{slot_index}].op"),
                    json!(
                        slot.menus
                            .iter()
                            .map(|menu| opcode_name(menu.op))
                            .collect::<Vec<_>>()
                    ),
                    json!(opcode_name(choice.op)),
                )
            })?;
        if choice.operands.len() != menu.operand_roles.len() {
            return Err(failure(
                AuthoringErrorCode::ValidationRejected,
                format!("$.choices[{slot_index}].operands"),
                json!({"length":menu.operand_roles.len()}),
                json!({"length":choice.operands.len()}),
            ));
        }
        let operands = choice
            .operands
            .iter()
            .enumerate()
            .map(|(operand_index, role_id)| {
                if !menu.operand_roles[operand_index].contains(role_id) {
                    return Err(failure(
                        AuthoringErrorCode::ValidationRejected,
                        format!("$.choices[{slot_index}].operands[{operand_index}]"),
                        json!(menu.operand_roles[operand_index]),
                        json!(role_id),
                    ));
                }
                frame.roles.get(role_id).map(role_to_staged).ok_or_else(|| {
                    failure(
                        AuthoringErrorCode::ValidationRejected,
                        format!("$.choices[{slot_index}].operands[{operand_index}]"),
                        json!({"known_role":frame.roles.keys().collect::<Vec<_>>()}),
                        json!(role_id),
                    )
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        body.push(StagedOperation {
            bind: format!("${}", slot.id),
            op: choice.op,
            operands,
        });
    }
    compile_staged(&StagedProposal {
        schema: super::STAGED_SCHEMA.to_owned(),
        stages: frame.stages,
        seed,
        body,
        state: format!("${state}"),
    })
}

fn validate_blueprint(
    declarations: &PublicAuthoringDeclarations,
    blueprint: &AuthoringFrameBlueprint,
) -> Result<(), AuthoringError> {
    if declarations.task_id.is_empty() {
        return Err(failure(
            AuthoringErrorCode::ValidationRejected,
            "$.task_id",
            json!("non-empty task identity"),
            json!(declarations.task_id),
        ));
    }
    if blueprint.stages == 0 || blueprint.slots.is_empty() || blueprint.slots.len() > 8 {
        return Err(failure(
            AuthoringErrorCode::ValidationRejected,
            "$.frame",
            json!({"stages_min":1,"body_min":1,"body_max":8}),
            json!({"stages":blueprint.stages,"body":blueprint.slots.len()}),
        ));
    }
    let expanded = blueprint
        .stages
        .checked_mul(blueprint.slots.len())
        .ok_or_else(|| {
            failure(
                AuthoringErrorCode::ValidationRejected,
                "$.frame.stages",
                json!({"expanded_max":MAX_OPERATIONS}),
                json!("overflow"),
            )
        })?;
    if expanded > MAX_OPERATIONS {
        return Err(failure(
            AuthoringErrorCode::ValidationRejected,
            "$.frame.stages",
            json!({"expanded_max":MAX_OPERATIONS}),
            json!({"expanded":expanded}),
        ));
    }
    let scalar_set = declarations.scalars.iter().collect::<BTreeSet<_>>();
    let tensor_set = declarations.tensors.iter().collect::<BTreeSet<_>>();
    for (id, role) in &blueprint.roles {
        if !valid_role_id(id) {
            return Err(failure(
                AuthoringErrorCode::ValidationRejected,
                format!("$.frame.roles.{id}"),
                json!("identifier"),
                json!(id),
            ));
        }
        validate_role(role, id, &scalar_set, &tensor_set)?;
    }
    if blueprint
        .roles
        .get(&blueprint.seed_role)
        .and_then(direct_graph_operand)
        .is_none()
    {
        return Err(failure(
            AuthoringErrorCode::ValidationRejected,
            "$.frame.seed_role",
            json!("known scalar or tensor role"),
            json!(blueprint.seed_role),
        ));
    }
    let mut slot_indices = BTreeMap::new();
    for (slot_index, slot) in blueprint.slots.iter().enumerate() {
        if !valid_role_id(&slot.id) || slot_indices.insert(&slot.id, slot_index).is_some() {
            return Err(failure(
                AuthoringErrorCode::ValidationRejected,
                format!("$.frame.slots[{slot_index}].id"),
                json!("unique identifier"),
                json!(slot.id),
            ));
        }
        if slot.menus.is_empty() {
            return Err(failure(
                AuthoringErrorCode::ValidationRejected,
                format!("$.frame.slots[{slot_index}].menus"),
                json!("non-empty opcode menu"),
                json!([]),
            ));
        }
        let mut opcodes = BTreeSet::new();
        for (menu_index, menu) in slot.menus.iter().enumerate() {
            if !opcodes.insert(opcode_name(menu.op)) || menu.operand_roles.len() != menu.op.arity()
            {
                return Err(failure(
                    AuthoringErrorCode::ValidationRejected,
                    format!("$.frame.slots[{slot_index}].menus[{menu_index}]"),
                    json!({"unique_opcode":true,"operand_count":menu.op.arity()}),
                    json!({"op":opcode_name(menu.op),"operand_count":menu.operand_roles.len()}),
                ));
            }
            for (operand_index, roles) in menu.operand_roles.iter().enumerate() {
                if roles.is_empty() {
                    return Err(failure(
                        AuthoringErrorCode::ValidationRejected,
                        format!(
                            "$.frame.slots[{slot_index}].menus[{menu_index}].operand_roles[{operand_index}]"
                        ),
                        json!("non-empty role menu"),
                        json!([]),
                    ));
                }
                for role_id in roles {
                    let role = blueprint.roles.get(role_id).ok_or_else(|| failure(
                        AuthoringErrorCode::ValidationRejected,
                        format!("$.frame.slots[{slot_index}].menus[{menu_index}].operand_roles[{operand_index}]"),
                        json!({"known_role":blueprint.roles.keys().collect::<Vec<_>>()}),
                        json!(role_id),
                    ))?;
                    if let FrameRole::StageLocal { slot: prior } = role
                        && slot_indices
                            .get(prior)
                            .is_none_or(|index| *index >= slot_index)
                    {
                        return Err(failure(
                            AuthoringErrorCode::ValidationRejected,
                            format!("$.frame.roles.{role_id}.slot"),
                            json!("earlier frame slot"),
                            json!(prior),
                        ));
                    }
                }
            }
        }
    }
    if blueprint.state_candidates.is_empty()
        || blueprint
            .state_candidates
            .iter()
            .any(|state| !slot_indices.contains_key(state))
    {
        return Err(failure(
            AuthoringErrorCode::ValidationRejected,
            "$.frame.state_candidates",
            json!({"non_empty_subset_of_slots":slot_indices.keys().collect::<Vec<_>>()}),
            json!(blueprint.state_candidates),
        ));
    }
    Ok(())
}

fn validate_role(
    role: &FrameRole,
    id: &str,
    scalars: &BTreeSet<&String>,
    tensors: &BTreeSet<&String>,
) -> Result<(), AuthoringError> {
    let path = format!("$.frame.roles.{id}");
    match role {
        FrameRole::Scalar { name } if !scalars.contains(name) => Err(failure(
            AuthoringErrorCode::ValidationRejected,
            path,
            json!({"declared_scalar":scalars}),
            json!(name),
        )),
        FrameRole::Tensor { name } if !tensors.contains(name) => Err(failure(
            AuthoringErrorCode::ValidationRejected,
            path,
            json!({"declared_tensor":tensors}),
            json!(name),
        )),
        FrameRole::StageLocal { slot } if !valid_role_id(slot) => Err(failure(
            AuthoringErrorCode::ValidationRejected,
            path,
            json!("slot identifier"),
            json!(slot),
        )),
        FrameRole::StateLag { stages, initial }
            if *stages == 0
                || initial.len() < *stages
                || initial
                    .iter()
                    .any(|value| !capture_declared(value, scalars, tensors)) =>
        {
            Err(failure(
                AuthoringErrorCode::ValidationRejected,
                path,
                json!({"positive_lag":true,"warmup_minimum":stages,"declared_captures_only":true}),
                json!({"lag":stages,"warmup_length":initial.len()}),
            ))
        }
        FrameRole::ScalarCycle { prefix, count, .. } => {
            validate_cycle(&path, prefix, *count, scalars)
        }
        FrameRole::TensorCycle { prefix, count, .. } => {
            validate_cycle(&path, prefix, *count, tensors)
        }
        FrameRole::Scalar { .. }
        | FrameRole::Tensor { .. }
        | FrameRole::StageLocal { .. }
        | FrameRole::StatePrev
        | FrameRole::StateLag { .. } => Ok(()),
    }
}

fn validate_cycle(
    path: &str,
    prefix: &str,
    count: usize,
    declarations: &BTreeSet<&String>,
) -> Result<(), AuthoringError> {
    if count == 0 || !(0..count).all(|index| declarations.contains(&format!("{prefix}{index}"))) {
        return Err(failure(
            AuthoringErrorCode::ValidationRejected,
            path,
            json!({"positive_count":true,"all_generated_names_declared":true}),
            json!({"prefix":prefix,"count":count}),
        ));
    }
    Ok(())
}

fn capture_declared(
    operand: &GraphOperand,
    scalars: &BTreeSet<&String>,
    tensors: &BTreeSet<&String>,
) -> bool {
    match operand {
        GraphOperand::Scalar { name } => scalars.contains(name),
        GraphOperand::Tensor { name } => tensors.contains(name),
        GraphOperand::Local { .. } => false,
    }
}

fn direct_graph_operand(role: &FrameRole) -> Option<GraphOperand> {
    match role {
        FrameRole::Scalar { name } => Some(GraphOperand::Scalar { name: name.clone() }),
        FrameRole::Tensor { name } => Some(GraphOperand::Tensor { name: name.clone() }),
        FrameRole::StageLocal { .. }
        | FrameRole::StatePrev
        | FrameRole::StateLag { .. }
        | FrameRole::ScalarCycle { .. }
        | FrameRole::TensorCycle { .. } => None,
    }
}

fn role_to_staged(role: &FrameRole) -> StagedOperand {
    match role {
        FrameRole::Scalar { name } => StagedOperand::Scalar { name: name.clone() },
        FrameRole::Tensor { name } => StagedOperand::Tensor { name: name.clone() },
        FrameRole::StageLocal { slot } => StagedOperand::StageLocal {
            name: format!("${slot}"),
        },
        FrameRole::StatePrev => StagedOperand::StatePrev,
        FrameRole::StateLag { stages, initial } => StagedOperand::StateLag {
            stages: *stages,
            initial: initial.clone(),
        },
        FrameRole::ScalarCycle {
            prefix,
            count,
            stride,
            offset,
        } => StagedOperand::ScalarCycle {
            prefix: prefix.clone(),
            count: *count,
            stride: *stride,
            offset: *offset,
        },
        FrameRole::TensorCycle {
            prefix,
            count,
            stride,
            offset,
        } => StagedOperand::TensorCycle {
            prefix: prefix.clone(),
            count: *count,
            stride: *stride,
            offset: *offset,
        },
    }
}

fn validate_framed_shape(value: &Value) -> Result<(), AuthoringError> {
    let root = require_object(value, "$").map_err(framed_hint)?;
    require_keys(
        root,
        "$",
        &["schema", "task_id", "frame_hash", "choices", "state"],
    )
    .map_err(framed_hint)?;
    if root.get("schema").and_then(Value::as_str) != Some(FRAMED_STAGED_SCHEMA) {
        return Err(framed_failure(
            AuthoringErrorCode::SchemaRejected,
            "$.schema",
            json!(FRAMED_STAGED_SCHEMA),
            root.get("schema").cloned().unwrap_or(Value::Null),
        ));
    }
    for field in ["task_id", "frame_hash", "state"] {
        if !root.get(field).is_some_and(Value::is_string) {
            return Err(framed_failure(
                AuthoringErrorCode::SchemaRejected,
                format!("$.{field}"),
                json!("string"),
                root.get(field).cloned().unwrap_or(Value::Null),
            ));
        }
    }
    let choices = root
        .get("choices")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            framed_failure(
                AuthoringErrorCode::SchemaRejected,
                "$.choices",
                json!("array"),
                root.get("choices").cloned().unwrap_or(Value::Null),
            )
        })?;
    if choices.is_empty() || choices.len() > 8 {
        return Err(framed_failure(
            AuthoringErrorCode::SchemaRejected,
            "$.choices",
            json!({"min_items":1,"max_items":8}),
            json!({"length":choices.len()}),
        ));
    }
    for (index, choice) in choices.iter().enumerate() {
        let path = format!("$.choices[{index}]");
        let choice = require_object(choice, &path).map_err(framed_hint)?;
        require_keys(choice, &path, &["slot", "op", "operands"]).map_err(framed_hint)?;
        if !choice.get("slot").is_some_and(Value::is_string) {
            return Err(framed_failure(
                AuthoringErrorCode::SchemaRejected,
                format!("{path}.slot"),
                json!("string"),
                choice.get("slot").cloned().unwrap_or(Value::Null),
            ));
        }
        if !matches!(
            choice.get("op").and_then(Value::as_str),
            Some("add" | "mul" | "fma")
        ) {
            return Err(framed_failure(
                AuthoringErrorCode::SchemaRejected,
                format!("{path}.op"),
                json!(["add", "mul", "fma"]),
                choice.get("op").cloned().unwrap_or(Value::Null),
            ));
        }
        let operands = choice
            .get("operands")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                framed_failure(
                    AuthoringErrorCode::SchemaRejected,
                    format!("{path}.operands"),
                    json!("array of role IDs"),
                    choice.get("operands").cloned().unwrap_or(Value::Null),
                )
            })?;
        if operands.iter().any(|operand| !operand.is_string()) {
            return Err(framed_failure(
                AuthoringErrorCode::SchemaRejected,
                format!("{path}.operands"),
                json!("array of role ID strings"),
                json!(operands),
            ));
        }
    }
    Ok(())
}

fn valid_role_id(value: &str) -> bool {
    valid_binding(&format!("${value}"))
}

fn opcode_name(opcode: GraphOpcode) -> &'static str {
    match opcode {
        GraphOpcode::Add => "add",
        GraphOpcode::Mul => "mul",
        GraphOpcode::Fma => "fma",
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    encode_digest(Sha256::digest(bytes).as_slice())
}

fn encode_digest(digest: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(digest.len() * 2);
    for byte in digest {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

fn frame_hash(
    task_id: &str,
    stages: usize,
    seed_role: &str,
    roles: &BTreeMap<String, FrameRole>,
    slots: &[FrameSlot],
    state_candidates: &[String],
) -> String {
    let identity = json!({
        "schema":AUTHORING_FRAME_SCHEMA,
        "task_id":task_id,
        "stages":stages,
        "seed_role":seed_role,
        "roles":roles,
        "slots":slots,
        "state_candidates":state_candidates,
        "expanded_operation_limit":MAX_OPERATIONS,
    });
    sha256_hex(&serde_json::to_vec(&identity).expect("frame identity serializes"))
}

fn framed_failure(
    code: AuthoringErrorCode,
    path: impl Into<String>,
    expected: Value,
    actual: Value,
) -> AuthoringError {
    with_repair_hint(
        failure(code, path, expected, actual),
        FRAMED_STAGED_MODEL_INSTRUCTION,
    )
}

fn framed_hint(error: AuthoringError) -> AuthoringError {
    with_repair_hint(error, FRAMED_STAGED_MODEL_INSTRUCTION)
}
