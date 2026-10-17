use std::collections::{BTreeMap, HashSet};

use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, de};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::{ResourceOperation, ResourceReference, ResourceType, WorkflowId, WorkflowVersionId};

pub const WORKFLOW_START_NODE_ID: &str = "__start__";
pub const WORKFLOW_END_NODE_ID: &str = "__end__";
pub const WORKFLOW_EXIT_NODE_TYPE: &str = "exit";

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkflowDefinition {
    #[schemars(with = "WorkflowSchemaVersion")]
    #[serde(deserialize_with = "deserialize_workflow_schema_version")]
    pub schema_version: String,
    pub start: WorkflowStart,
    pub nodes: Vec<WorkflowNode>,
    pub connections: Vec<WorkflowConnection>,
    pub end: WorkflowEnd,
    #[serde(default)]
    pub settings: WorkflowSettings,
}

#[derive(Clone, Debug, Default, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EditorDocument {
    #[serde(default)]
    pub node_layouts: Vec<NodeLayout>,
    #[serde(default)]
    pub boundary_layouts: Vec<BoundaryLayout>,
    #[serde(default)]
    pub edges: Vec<EditorEdge>,
    #[serde(default)]
    pub annotations: Vec<EditorAnnotation>,
    #[serde(default)]
    pub groups: Vec<EditorGroup>,
    #[serde(default)]
    pub viewport: EditorViewport,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BoundaryLayout {
    pub boundary: WorkflowBoundary,
    pub x: f64,
    pub y: f64,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowBoundary {
    Start,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NodeLayout {
    pub node_id: String,
    pub x: f64,
    pub y: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<f64>,
    #[serde(default)]
    pub collapsed: bool,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EditorEdge {
    pub edge_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label_position: Option<f64>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EditorAnnotation {
    pub id: String,
    pub text: String,
    pub x: f64,
    pub y: f64,
    #[serde(default = "default_annotation_width")]
    pub width: f64,
    #[serde(default = "default_annotation_height")]
    pub height: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EditorGroup {
    pub id: String,
    pub label: String,
    pub node_ids: Vec<String>,
    #[serde(default)]
    pub collapsed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

const fn default_annotation_width() -> f64 {
    240.0
}

const fn default_annotation_height() -> f64 {
    160.0
}

#[derive(Clone, Copy, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EditorViewport {
    pub x: f64,
    pub y: f64,
    pub zoom: f64,
}

impl Default for EditorViewport {
    fn default() -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            zoom: 1.0,
        }
    }
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub enum ExecutionSource {
    Version {
        version_id: WorkflowVersionId,
    },
    DraftRevision {
        workflow_id: WorkflowId,
        revision: u64,
    },
}

#[derive(Clone, Debug, Default, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DebugPlan {
    pub mode: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_node_id: Option<String>,
    #[serde(default)]
    pub included_node_ids: Vec<String>,
    #[serde(default)]
    pub skipped_node_ids: Vec<String>,
    #[serde(default)]
    pub input_source: Option<Value>,
    #[serde(default)]
    pub side_effect_decisions: Value,
    #[serde(default)]
    pub overlay_hash: Option<String>,
}

/// The single source of truth for the persisted Workflow Definition schema
/// version. Bump together with `WorkflowSchemaVersion` and every fixture.
pub const WORKFLOW_SCHEMA_VERSION: &str = "8.0";

#[derive(JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum WorkflowSchemaVersion {
    #[serde(rename = "8.0")]
    V8,
}

fn deserialize_workflow_schema_version<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    let version = String::deserialize(deserializer)?;
    if version == WORKFLOW_SCHEMA_VERSION {
        Ok(version)
    } else {
        Err(de::Error::custom(format!(
            "unsupported Workflow Definition schema version '{version}'; expected {WORKFLOW_SCHEMA_VERSION}"
        )))
    }
}

/// The only public value language accepted by Workflow Definition 8.0.
///
/// Input bindings deliberately contain no calculation node. Arrays and objects
/// are recursive so structured JSON may embed stable references and text
/// templates without falling back to a second expression protocol.
#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind", deny_unknown_fields)]
pub enum InputBinding {
    Literal {
        value: Value,
    },
    Reference {
        selector: ValueSelector,
        #[serde(default, rename = "missingPolicy")]
        missing_policy: MissingValuePolicy,
    },
    Template {
        segments: Vec<InputTemplateSegment>,
    },
    Array {
        items: Vec<InputBinding>,
    },
    Object {
        fields: BTreeMap<String, InputBinding>,
    },
}

/// Reference-only parameters such as List input and Loop input/output.
#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind", deny_unknown_fields)]
pub enum ReferenceBinding {
    Reference {
        selector: ValueSelector,
        #[serde(default, rename = "missingPolicy")]
        missing_policy: MissingValuePolicy,
    },
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ValueSelector {
    pub namespace: ValueNamespace,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_node_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<String>,
    #[serde(default)]
    pub run: ValueSelection,
    #[serde(default)]
    pub item: ValueSelection,
    #[serde(default)]
    pub path: Vec<ValuePathSegment>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ValueNamespace {
    Inputs,
    Outputs,
    Contexts,
    Execution,
    Item,
    Loop,
}

#[derive(Clone, Debug, Default, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind", deny_unknown_fields)]
pub enum ValueSelection {
    #[default]
    Current,
    First,
    Last,
    All,
    Index {
        index: u32,
    },
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(untagged)]
pub enum ValuePathSegment {
    Key(String),
    Index(u32),
}

#[derive(Clone, Debug, Default, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind", deny_unknown_fields)]
pub enum MissingValuePolicy {
    #[default]
    Error,
    Null,
    Omit,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind", deny_unknown_fields)]
pub enum InputTemplateSegment {
    Text {
        text: String,
    },
    Reference {
        selector: ValueSelector,
        #[serde(default, rename = "missingPolicy")]
        missing_policy: MissingValuePolicy,
    },
}

/// The only computation shape accepted by IF and List filter rows.
#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConditionSpec {
    pub left: InputBinding,
    pub operator: ConditionOperator,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub right: Option<InputBinding>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConditionOperator {
    Eq,
    Ne,
    Gt,
    Gte,
    Lt,
    Lte,
    In,
    Contains,
    NotContains,
    EndsWith,
    StartsWith,
    Matches,
    IsEmpty,
    IsNotEmpty,
}

impl WorkflowDefinition {
    /// Fresh workflows start with one protected exit node wired from Start so
    /// the canvas always has a terminal to connect to.
    #[must_use]
    pub fn empty() -> Self {
        let exit = WorkflowNode {
            id: "exit".to_owned(),
            key: "exit".to_owned(),
            node_type: WORKFLOW_EXIT_NODE_TYPE.to_owned(),
            type_version: 1,
            name: "End".to_owned(),
            disabled: false,
            protected: true,
            parent_id: None,
            parameters: ExitParameters::default().to_value(),
            context_writes: Vec::new(),
            resource_references: Vec::new(),
            settings: NodeSettings::default(),
        };
        Self {
            schema_version: WORKFLOW_SCHEMA_VERSION.to_owned(),
            start: WorkflowStart::default(),
            nodes: vec![exit],
            connections: vec![WorkflowConnection {
                id: "start-exit".to_owned(),
                source_node_id: WORKFLOW_START_NODE_ID.to_owned(),
                source_handle: "main".to_owned(),
                target_node_id: "exit".to_owned(),
                target_handle: "main".to_owned(),
                order: 0,
            }],
            end: WorkflowEnd::default(),
            settings: WorkflowSettings::default(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkflowStart {
    #[serde(default)]
    pub inputs: Value,
    #[serde(default)]
    pub contexts: BTreeMap<String, ContextDefinition>,
}

impl Default for WorkflowStart {
    fn default() -> Self {
        Self {
            inputs: serde_json::json!({"type":"object","properties":{},"additionalProperties":false}),
            contexts: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ContextDefinition {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub schema: Value,
    #[serde(default)]
    pub default: Value,
    #[serde(default)]
    pub mutable: bool,
    #[serde(default)]
    pub sensitive: bool,
    #[serde(default)]
    pub client_writable: bool,
    #[serde(default)]
    pub scope: ContextScope,
    #[serde(default)]
    pub max_size: Option<u64>,
    #[serde(default)]
    pub merge_policy: ContextMergePolicy,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextScope {
    #[default]
    ExecutionTree,
    Session,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextMergePolicy {
    #[default]
    Replace,
    Append,
    MergeObject,
    Increment,
    RejectConflict,
}

#[derive(Clone, Debug, Default, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkflowEnd {
    #[serde(default)]
    pub completion: WorkflowCompletion,
    #[serde(default)]
    pub outputs: BTreeMap<String, WorkflowOutput>,
    #[serde(default)]
    pub error: WorkflowErrorEnd,
}

/// How the engine finalizes an execution whose branches reach exit nodes.
///
/// - `FirstReturn` (default): the first delivery to reach an exit (main or
///   error port, symmetric) finalizes the execution immediately and cancels
///   every other in-flight activation.
/// - `AllComplete`: the execution waits for quiescence; every reached exit is
///   evaluated against the shared `end.outputs` contract and each output field
///   is returned as an array with one element per reached exit (definition
///   order; unreached exits and unbound fields are omitted).
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowCompletion {
    #[default]
    FirstReturn,
    AllComplete,
}

#[derive(Clone, Debug, Default, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkflowErrorEnd {
    #[serde(default)]
    pub outputs: BTreeMap<String, WorkflowOutput>,
}

/// The global output contract shared by every exit node. Field values are
/// mapped per exit in [`ExitParameters`], never here.
#[derive(Clone, Debug, Default, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkflowOutput {
    #[serde(default)]
    pub schema: Value,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub sensitive: bool,
}

/// Per-exit mapping of contract field names to dynamic values. Stored inside
/// the exit node's `parameters` payload.
#[derive(Clone, Debug, Default, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExitParameters {
    #[serde(default)]
    pub outputs: BTreeMap<String, InputBinding>,
    #[serde(default)]
    pub error_outputs: BTreeMap<String, InputBinding>,
}

impl ExitParameters {
    #[must_use]
    pub fn parse(parameters: &Value) -> Option<Self> {
        if parameters.is_null() {
            return Some(Self::default());
        }
        serde_json::from_value(parameters.clone()).ok()
    }

    #[must_use]
    pub fn to_value(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ContextWrite {
    pub operation: ContextWriteOperation,
    pub path: String,
    pub value: InputBinding,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextWriteOperation {
    Set,
    SetIfAbsent,
    Delete,
    Append,
    MergeObject,
    Increment,
    Min,
    Max,
    CompareAndSet,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionOrder {
    #[default]
    Deterministic,
    Parallel,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkflowSettings {
    #[serde(default)]
    pub execution_order: ExecutionOrder,
    #[serde(default = "default_activation_budget")]
    #[schemars(range(min = 1))]
    pub activation_budget: u32,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

impl Default for WorkflowSettings {
    fn default() -> Self {
        Self {
            execution_order: ExecutionOrder::Deterministic,
            activation_budget: default_activation_budget(),
            timeout_ms: None,
        }
    }
}

const fn default_activation_budget() -> u32 {
    10_000
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NodeSettings {
    #[serde(default)]
    pub execute_once: bool,
    #[serde(default)]
    pub always_output_data: bool,
    #[serde(default)]
    pub retry_on_fail: bool,
    #[serde(default = "default_max_tries")]
    pub max_tries: u16,
    #[serde(default)]
    pub wait_between_tries_ms: u64,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

impl Default for NodeSettings {
    fn default() -> Self {
        Self {
            execute_once: false,
            always_output_data: false,
            retry_on_fail: false,
            max_tries: default_max_tries(),
            wait_between_tries_ms: 0,
            timeout_ms: None,
        }
    }
}

const fn default_max_tries() -> u16 {
    1
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkflowNode {
    pub id: String,
    pub key: String,
    #[serde(rename = "type")]
    pub node_type: String,
    #[schemars(range(min = 1))]
    pub type_version: u32,
    pub name: String,
    #[serde(default)]
    pub disabled: bool,
    /// Protected exit nodes cannot be removed from the canvas; the initial
    /// exit created with the workflow is protected.
    #[serde(default)]
    pub protected: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    #[serde(default)]
    pub parameters: Value,
    #[serde(default)]
    pub context_writes: Vec<ContextWrite>,
    #[serde(default)]
    pub resource_references: Vec<ResourceReference>,
    #[serde(default)]
    pub settings: NodeSettings,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkflowConnection {
    pub id: String,
    pub source_node_id: String,
    pub source_handle: String,
    pub target_node_id: String,
    pub target_handle: String,
    #[schemars(range(min = 0))]
    pub order: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DefinitionIssue {
    pub code: String,
    pub path: String,
    pub message: String,
}

#[must_use]
pub fn validate_definition(definition: &WorkflowDefinition) -> Vec<DefinitionIssue> {
    let mut issues = Vec::new();
    if definition.schema_version != WORKFLOW_SCHEMA_VERSION {
        issue(
            &mut issues,
            "UNSUPPORTED_SCHEMA",
            "schemaVersion",
            "Only schema version 8.0 is supported",
        );
    }
    if definition.settings.activation_budget == 0 {
        issue(
            &mut issues,
            "INVALID_ACTIVATION_BUDGET",
            "settings.activationBudget",
            "Activation budget must be greater than zero",
        );
    }
    if !definition.start.inputs.is_object() {
        issue(
            &mut issues,
            "INVALID_INPUT_SCHEMA",
            "start.inputs",
            "Start inputs must be a JSON Schema object",
        );
    }
    for (name, context) in &definition.start.contexts {
        if !valid_reference_key(name) {
            issue(
                &mut issues,
                "INVALID_CONTEXT_KEY",
                &format!("start.contexts.{name}"),
                "Context keys must use lowercase ASCII letters, digits and underscores",
            );
        }
        if !context.schema.is_object() {
            issue(
                &mut issues,
                "INVALID_CONTEXT_SCHEMA",
                &format!("start.contexts.{name}.schema"),
                "Context schema must be an object",
            );
        }
        if context.max_size == Some(0) {
            issue(
                &mut issues,
                "INVALID_CONTEXT_MAX_SIZE",
                &format!("start.contexts.{name}.maxSize"),
                "Context maxSize must be greater than zero",
            );
        }
    }
    validate_workflow_outputs("end.outputs", &definition.end.outputs, &mut issues);
    validate_workflow_outputs(
        "end.error.outputs",
        &definition.end.error.outputs,
        &mut issues,
    );
    let mut ids = HashSet::new();
    let mut keys = HashSet::new();
    let mut exit_ids = HashSet::new();
    for (index, node) in definition.nodes.iter().enumerate() {
        if node.node_type == WORKFLOW_EXIT_NODE_TYPE {
            exit_ids.insert(node.id.clone());
        }
        if matches!(
            node.id.as_str(),
            WORKFLOW_START_NODE_ID | WORKFLOW_END_NODE_ID
        ) {
            issue(
                &mut issues,
                "RESERVED_NODE_ID",
                &format!("nodes[{index}].id"),
                "Node id is reserved for a Workflow boundary",
            );
        } else if node.id.is_empty() || node.id.len() > 128 {
            issue(
                &mut issues,
                "INVALID_NODE_ID",
                &format!("nodes[{index}].id"),
                "Node id must contain 1 to 128 characters",
            );
        } else if !ids.insert(node.id.clone()) {
            issue(
                &mut issues,
                "DUPLICATE_NODE_ID",
                &format!("nodes[{index}].id"),
                "Node id must be unique",
            );
        }
        if !valid_reference_key(&node.key) {
            issue(
                &mut issues,
                "INVALID_NODE_KEY",
                &format!("nodes[{index}].key"),
                "Node key must use lowercase ASCII letters, digits and underscores",
            );
        } else if !keys.insert(node.key.as_str()) {
            issue(
                &mut issues,
                "DUPLICATE_NODE_KEY",
                &format!("nodes[{index}].key"),
                "Node key must be unique within a workflow",
            );
        }
        if matches!(node.node_type.as_str(), "manual_trigger" | "remote_trigger") {
            issue(
                &mut issues,
                "TRIGGER_NODE_REMOVED",
                &format!("nodes[{index}].type"),
                "Trigger nodes are not part of Workflow Definition 8.0; configure a Trigger Binding instead",
            );
        }
        if node.node_type.is_empty()
            || node.node_type.len() > 128
            || !node.node_type.bytes().all(|byte| {
                byte.is_ascii_lowercase()
                    || byte.is_ascii_digit()
                    || matches!(byte, b'_' | b'.' | b'-')
            })
        {
            issue(
                &mut issues,
                "INVALID_NODE_TYPE",
                &format!("nodes[{index}].type"),
                "Node type must use lowercase ASCII letters, digits, '.', '_' or '-'",
            );
        }
        if node.name.trim().is_empty() || node.name.chars().count() > 160 {
            issue(
                &mut issues,
                "INVALID_NODE_NAME",
                &format!("nodes[{index}].name"),
                "Node name must contain 1 to 160 characters",
            );
        }
        if node.type_version == 0 {
            issue(
                &mut issues,
                "UNSUPPORTED_NODE_VERSION",
                &format!("nodes[{index}].typeVersion"),
                "Node type version must be greater than zero",
            );
        }
        if node.settings.max_tries == 0 {
            issue(
                &mut issues,
                "INVALID_MAX_TRIES",
                &format!("nodes[{index}].settings.maxTries"),
                "maxTries must be greater than zero",
            );
        }
        for (reference_index, reference) in node.resource_references.iter().enumerate() {
            if is_resource_node(&node.node_type)
                && !reference_matches_node(&node.node_type, reference)
            {
                issue(
                    &mut issues,
                    "INVALID_RESOURCE_REFERENCE",
                    &format!("nodes[{index}].resourceReferences[{reference_index}]"),
                    "Resource type or operation does not match the node type",
                );
            }
        }
        if node.node_type == "agent" {
            validate_agent_node(index, node, &mut issues);
        }
        if node.node_type == WORKFLOW_EXIT_NODE_TYPE {
            validate_exit_node(
                index,
                node,
                &definition.end.outputs,
                &definition.end.error.outputs,
                &definition.connections,
                &mut issues,
            );
        }
    }
    let mut connection_ids = HashSet::new();
    let mut connection_orders = HashSet::new();
    for (index, connection) in definition.connections.iter().enumerate() {
        if connection.id.is_empty() || !connection_ids.insert(connection.id.as_str()) {
            issue(
                &mut issues,
                "INVALID_CONNECTION_ID",
                &format!("connections[{index}].id"),
                "Connection id must be present and unique",
            );
        }
        if connection.source_handle.is_empty() || connection.target_handle.is_empty() {
            issue(
                &mut issues,
                "INVALID_CONNECTION_HANDLE",
                &format!("connections[{index}]"),
                "Connection handles are required",
            );
        }
        if !connection_orders.insert((
            connection.source_node_id.as_str(),
            connection.source_handle.as_str(),
            connection.order,
        )) {
            issue(
                &mut issues,
                "DUPLICATE_CONNECTION_ORDER",
                &format!("connections[{index}].order"),
                "Connection order must be unique for each source node and source handle",
            );
        }
        let source_is_start = connection.source_node_id == WORKFLOW_START_NODE_ID;
        let source_is_end = connection.source_node_id == WORKFLOW_END_NODE_ID;
        let target_is_start = connection.target_node_id == WORKFLOW_START_NODE_ID;
        let target_is_end = connection.target_node_id == WORKFLOW_END_NODE_ID;
        let source_is_exit = exit_ids.contains(&connection.source_node_id);
        let target_is_exit = exit_ids.contains(&connection.target_node_id);
        if source_is_end || target_is_end {
            issue(
                &mut issues,
                "END_BOUNDARY_REMOVED",
                &format!("connections[{index}]"),
                "Workflow Definition 8.0 uses Exit nodes; connect to an Exit node instead",
            );
            continue;
        }
        if target_is_start || source_is_exit {
            issue(
                &mut issues,
                "INVALID_BOUNDARY_DIRECTION",
                &format!("connections[{index}]"),
                "Start can only be a connection source and exit nodes can only be connection targets",
            );
            continue;
        }
        if source_is_start && connection.source_handle != "main" {
            issue(
                &mut issues,
                "INVALID_START_PORT",
                &format!("connections[{index}].sourceHandle"),
                "Start only exposes the main output port",
            );
        }
        if target_is_exit && !matches!(connection.target_handle.as_str(), "main" | "error") {
            issue(
                &mut issues,
                "INVALID_END_PORT",
                &format!("connections[{index}].targetHandle"),
                "Exit nodes only accept main or error input ports",
            );
        }
        if source_is_start && target_is_exit && connection.target_handle != "main" {
            issue(
                &mut issues,
                "INVALID_DIRECT_ERROR_CONNECTION",
                &format!("connections[{index}]"),
                "Start can only connect to an exit node through the main port",
            );
        }
        if (!source_is_start && !ids.contains(&connection.source_node_id))
            || !ids.contains(&connection.target_node_id)
        {
            issue(
                &mut issues,
                "DANGLING_CONNECTION",
                &format!("connections[{index}]"),
                "Connection references a missing node",
            );
            continue;
        }
    }
    issues
}

fn validate_agent_node(index: usize, node: &WorkflowNode, issues: &mut Vec<DefinitionIssue>) {
    let node_path = format!("nodes[{index}]");
    if node.type_version != 2 {
        issue(
            issues,
            "UNSUPPORTED_NODE_VERSION",
            &format!("{node_path}.typeVersion"),
            "Workflow Definition 8.0 requires Agent node version 2",
        );
    }

    match node.parameters.get("sessionPolicy") {
        Some(policy)
            if policy.as_object().is_some_and(|object| {
                matches!(
                    object.get("mode").and_then(Value::as_str),
                    Some("application_session" | "invocation")
                )
            }) => {}
        _ => issue(
            issues,
            "AGENT_SESSION_POLICY_INVALID",
            &format!("{node_path}.parameters.sessionPolicy"),
            "Agent sessionPolicy must explicitly contain mode application_session or invocation",
        ),
    }

    let mut model_count = 0_usize;
    let mut workspace_sandbox_count = 0_usize;
    let mut long_term_memory_count = 0_usize;

    for (reference_index, reference) in node.resource_references.iter().enumerate() {
        let path = format!("{node_path}.resourceReferences[{reference_index}]");
        // Every embedded Agent resource names its frozen slot via bindingRole.
        if let Some(binding_role) = reference.binding_role.as_deref() {
            if binding_role.is_empty() {
                issue(
                    issues,
                    "AGENT_RESOURCE_SLOT_INVALID",
                    &format!("{path}.bindingRole"),
                    "Agent attachment bindingRole must not be empty",
                );
            }
        }

        let valid = match (reference.binding_role.as_deref(), reference.resource_type) {
            (Some("model"), ResourceType::Model) => {
                model_count += 1;
                reference.operation == ResourceOperation::Use
                    && reference.resource_version_id.is_some()
            }
            (Some("workspace_sandbox"), ResourceType::SandboxProfile) => {
                workspace_sandbox_count += 1;
                reference.operation == ResourceOperation::Use
                    && reference.resource_version_id.is_some()
            }
            (Some("mcp_tools"), ResourceType::McpTool) => {
                reference.operation == ResourceOperation::Use
                    && reference.resource_version_id.is_some()
            }
            (Some("skills"), ResourceType::Skill) => {
                reference.operation == ResourceOperation::Use
                    && reference.resource_version_id.is_some()
            }
            (Some("knowledge"), ResourceType::Rag) => {
                reference.operation == ResourceOperation::Read
            }
            (Some("long_term_memory"), ResourceType::Memory) => {
                long_term_memory_count += 1;
                matches!(reference.operation, ResourceOperation::Use)
            }
            _ => false,
        };
        if !valid {
            issue(
                issues,
                "AGENT_RESOURCE_SLOT_INVALID",
                &path,
                "Agent resource reference does not match a Manifest 3.0 slot, type, operation, or exact-version requirement",
            );
        }
    }

    if model_count > 1 {
        issue(
            issues,
            "AGENT_RESOURCE_SLOT_INVALID",
            &format!("{node_path}.resourceReferences"),
            "Agent accepts at most one inspector Model reference",
        );
    }
    if workspace_sandbox_count > 1 || long_term_memory_count > 1 {
        issue(
            issues,
            "AGENT_RESOURCE_SLOT_INVALID",
            &format!("{node_path}.resourceReferences"),
            "Agent accepts at most one Workspace Sandbox and one long-term memory attachment",
        );
    }
}

fn validate_exit_node(
    index: usize,
    node: &WorkflowNode,
    success_contract: &BTreeMap<String, WorkflowOutput>,
    error_contract: &BTreeMap<String, WorkflowOutput>,
    connections: &[WorkflowConnection],
    issues: &mut Vec<DefinitionIssue>,
) {
    let node_path = format!("nodes[{index}]");
    let parameters = match serde_json::from_value::<ExitParameters>(node.parameters.clone()) {
        Ok(parameters) => parameters,
        Err(error) => {
            issue(
                issues,
                "EXIT_PARAMETERS_INVALID",
                &format!("{node_path}.parameters"),
                &format!("Exit node {} parameters are invalid: {error}", node.id),
            );
            return;
        }
    };
    validate_exit_mappings(
        issues,
        &node_path,
        "outputs",
        &parameters.outputs,
        success_contract,
        connections
            .iter()
            .any(|edge| edge.target_node_id == node.id && edge.target_handle == "main"),
    );
    validate_exit_mappings(
        issues,
        &node_path,
        "errorOutputs",
        &parameters.error_outputs,
        error_contract,
        connections
            .iter()
            .any(|edge| edge.target_node_id == node.id && edge.target_handle == "error"),
    );
}

fn validate_exit_mappings(
    issues: &mut Vec<DefinitionIssue>,
    node_path: &str,
    group: &str,
    mappings: &BTreeMap<String, InputBinding>,
    contract: &BTreeMap<String, WorkflowOutput>,
    connected: bool,
) {
    for name in mappings.keys() {
        if !contract.contains_key(name) {
            issue(
                issues,
                "EXIT_MAPPING_KEY_UNKNOWN",
                &format!("{node_path}.parameters.{group}.{name}"),
                "Exit mappings must reference fields declared in the global output contract",
            );
        }
    }
    for (name, output) in contract {
        if connected && output.required && !mappings.contains_key(name) {
            issue(
                issues,
                "EXIT_REQUIRED_MAPPING_MISSING",
                &format!("{node_path}.parameters.{group}.{name}"),
                "Connected exit inputs must map each required contract field",
            );
        }
    }
}

fn validate_workflow_outputs(
    path: &str,
    outputs: &BTreeMap<String, WorkflowOutput>,
    issues: &mut Vec<DefinitionIssue>,
) {
    for (name, output) in outputs {
        if !valid_reference_key(name) {
            issue(
                issues,
                "INVALID_END_OUTPUT_KEY",
                &format!("{path}.{name}"),
                "End output keys must use lowercase ASCII letters, digits and underscores",
            );
        }
        if !output.schema.is_object() {
            issue(
                issues,
                "INVALID_END_OUTPUT_SCHEMA",
                &format!("{path}.{name}.schema"),
                "End output schema must be an object",
            );
        }
    }
}

fn valid_reference_key(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.bytes().enumerate().all(|(index, byte)| {
            byte.is_ascii_lowercase() || byte == b'_' || (index > 0 && byte.is_ascii_digit())
        })
}

#[must_use]
pub fn validate_editor_document(
    definition: &WorkflowDefinition,
    document: &EditorDocument,
) -> Vec<DefinitionIssue> {
    let mut issues = Vec::new();
    let node_ids = definition
        .nodes
        .iter()
        .map(|node| node.id.as_str())
        .collect::<HashSet<_>>();
    let mut layout_nodes = HashSet::new();
    for (index, layout) in document.node_layouts.iter().enumerate() {
        if !node_ids.contains(layout.node_id.as_str()) {
            issue(
                &mut issues,
                "DANGLING_EDITOR_NODE",
                &format!("nodeLayouts[{index}].nodeId"),
                "Node layout references a missing workflow node",
            );
        }
        if !layout_nodes.insert(layout.node_id.as_str()) {
            issue(
                &mut issues,
                "DUPLICATE_NODE_LAYOUT",
                &format!("nodeLayouts[{index}].nodeId"),
                "Each workflow node may have only one layout",
            );
        }
        if !layout.x.is_finite()
            || !layout.y.is_finite()
            || layout
                .width
                .is_some_and(|value| !value.is_finite() || value <= 0.0)
            || layout
                .height
                .is_some_and(|value| !value.is_finite() || value <= 0.0)
        {
            issue(
                &mut issues,
                "INVALID_NODE_LAYOUT",
                &format!("nodeLayouts[{index}]"),
                "Node layout coordinates and dimensions must be finite and positive",
            );
        }
    }
    let mut boundaries = HashSet::new();
    for (index, layout) in document.boundary_layouts.iter().enumerate() {
        if !boundaries.insert(layout.boundary) {
            issue(
                &mut issues,
                "DUPLICATE_BOUNDARY_LAYOUT",
                &format!("boundaryLayouts[{index}].boundary"),
                "Each Workflow boundary may have only one layout",
            );
        }
        if !layout.x.is_finite() || !layout.y.is_finite() {
            issue(
                &mut issues,
                "INVALID_BOUNDARY_LAYOUT",
                &format!("boundaryLayouts[{index}]"),
                "Boundary layout coordinates must be finite",
            );
        }
    }
    let mut annotation_ids = HashSet::new();
    for (index, annotation) in document.annotations.iter().enumerate() {
        if annotation.id.is_empty() || !annotation_ids.insert(annotation.id.as_str()) {
            issue(
                &mut issues,
                "INVALID_ANNOTATION_ID",
                &format!("annotations[{index}].id"),
                "Annotation ids must be present and unique",
            );
        }
        if !annotation.x.is_finite()
            || !annotation.y.is_finite()
            || !annotation.width.is_finite()
            || annotation.width < 150.0
            || !annotation.height.is_finite()
            || annotation.height < 80.0
        {
            issue(
                &mut issues,
                "INVALID_ANNOTATION_LAYOUT",
                &format!("annotations[{index}]"),
                "Annotation coordinates must be finite and dimensions must be at least 150 by 80",
            );
        }
    }
    let mut group_ids = HashSet::new();
    for (index, group) in document.groups.iter().enumerate() {
        if group.id.is_empty() || !group_ids.insert(group.id.as_str()) {
            issue(
                &mut issues,
                "INVALID_GROUP_ID",
                &format!("groups[{index}].id"),
                "Group ids must be present and unique",
            );
        }
        if group.node_ids.is_empty()
            || group
                .node_ids
                .iter()
                .any(|node_id| !node_ids.contains(node_id.as_str()))
        {
            issue(
                &mut issues,
                "INVALID_GROUP_MEMBERS",
                &format!("groups[{index}].nodeIds"),
                "Groups must reference at least one existing workflow node",
            );
        }
    }
    if !document.viewport.x.is_finite()
        || !document.viewport.y.is_finite()
        || !document.viewport.zoom.is_finite()
        || document.viewport.zoom <= 0.0
    {
        issue(
            &mut issues,
            "INVALID_VIEWPORT",
            "viewport",
            "Viewport coordinates must be finite and zoom must be positive",
        );
    }
    issues
}

fn is_resource_node(node_type: &str) -> bool {
    matches!(
        node_type,
        "model"
            | "mcp_tool"
            | "skill"
            | "rag"
            | "memory"
            | "agent"
            | "code"
            | "knowledge_retrieval"
    )
}

fn reference_matches_node(node_type: &str, reference: &ResourceReference) -> bool {
    match node_type {
        "manual_trigger" => false,
        "model" => {
            reference.resource_type == ResourceType::Model
                && reference.operation == ResourceOperation::Use
        }
        "mcp_tool" => {
            reference.resource_type == ResourceType::McpTool
                && reference.operation == ResourceOperation::Use
        }
        "skill" => {
            reference.resource_type == ResourceType::Skill
                && reference.operation == ResourceOperation::Use
        }
        "knowledge_retrieval" => {
            reference.resource_type == ResourceType::Rag
                && reference.operation == ResourceOperation::Read
                && reference.binding_role.is_none()
        }
        "rag" => {
            reference.resource_type == ResourceType::Rag
                && matches!(
                    reference.operation,
                    ResourceOperation::Read | ResourceOperation::Write
                )
        }
        "agent" => matches!(
            reference.resource_type,
            ResourceType::Model
                | ResourceType::McpTool
                | ResourceType::Skill
                | ResourceType::Rag
                | ResourceType::Memory
                | ResourceType::SandboxProfile
        ),
        "code" => matches!(
            (reference.resource_type, reference.operation),
            (ResourceType::SandboxProfile, ResourceOperation::Use)
                | (ResourceType::Credential, ResourceOperation::Use)
        ),
        "memory" => {
            reference.resource_type == ResourceType::Memory
                && reference.operation == ResourceOperation::Use
        }
        _ => false,
    }
}

fn issue(issues: &mut Vec<DefinitionIssue>, code: &str, path: &str, message: &str) {
    issues.push(DefinitionIssue {
        code: code.to_owned(),
        path: path.to_owned(),
        message: message.to_owned(),
    });
}

pub fn canonical_content_hash(value: &Value) -> Result<String, serde_json::Error> {
    let bytes = serde_jcs::to_vec(value)?;
    Ok(format!("sha256:v1:{:x}", Sha256::digest(bytes)))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        EditorDocument, WORKFLOW_EXIT_NODE_TYPE, WorkflowDefinition, canonical_content_hash,
        validate_definition,
    };

    #[test]
    fn canonical_hash_ignores_object_order_but_keeps_array_order() {
        assert_eq!(
            canonical_content_hash(&json!({"b": 2, "a": 1})).unwrap(),
            canonical_content_hash(&json!({"a": 1, "b": 2})).unwrap()
        );
        assert_ne!(
            canonical_content_hash(&json!([1, 2])).unwrap(),
            canonical_content_hash(&json!([2, 1])).unwrap()
        );
    }

    #[test]
    fn canonical_hash_normalizes_json_equivalent_integral_floats() {
        assert_eq!(
            canonical_content_hash(&json!({"x": 120.0, "y": -0.0, "zoom": 1.0})).unwrap(),
            canonical_content_hash(&json!({"x": 120, "y": 0, "zoom": 1})).unwrap()
        );
        assert_ne!(
            canonical_content_hash(&json!({"x": 1.5})).unwrap(),
            canonical_content_hash(&json!({"x": 1})).unwrap()
        );
    }

    #[test]
    fn canonical_hash_survives_repeated_fractional_layout_json_round_trips() {
        let mut value =
            json!({"x": 209.66666666666666_f64, "y": 0.1_f64, "zoom": 0.3333333333333333_f64});
        let expected = canonical_content_hash(&value).unwrap();
        for _ in 0..8 {
            value = serde_json::from_str(&serde_json::to_string(&value).unwrap()).unwrap();
            assert_eq!(canonical_content_hash(&value).unwrap(), expected);
        }
    }

    #[test]
    fn empty_definition_is_valid() {
        let definition = WorkflowDefinition::empty();
        assert!(validate_definition(&definition).is_empty());
        assert_eq!(definition.nodes.len(), 1);
        assert_eq!(definition.nodes[0].node_type, WORKFLOW_EXIT_NODE_TYPE);
        assert!(definition.nodes[0].protected);
        assert_eq!(definition.connections.len(), 1);
        assert_eq!(definition.connections[0].target_node_id, "exit");
    }

    fn agent_definition(resource_references: serde_json::Value) -> WorkflowDefinition {
        serde_json::from_value(json!({
            "schemaVersion":"8.0",
            "start":{"inputs":{"type":"object","properties":{},"additionalProperties":false},"contexts":{}},
            "nodes":[{
                "id":"agent","key":"agent","type":"agent","typeVersion":2,"name":"Agent",
                "parameters":{"sessionPolicy":{"mode":"invocation"}},
                "resourceReferences":resource_references
            }],
            "connections":[],
            "end":{"outputs":{}}
        }))
        .expect("Agent Definition 8.0 fixture")
    }

    #[test]
    fn agent_draft_allows_missing_model_and_validates_existing_inspector_references() {
        let model = json!({
            "bindingRole":"model",
            "resourceType":"model",
            "resourceId":"018f47a0-7e9c-7000-8000-000000000001",
            "resourceVersionId":"018f47a0-7e9c-7000-8000-000000000002",
            "operation":"use"
        });
        let valid = agent_definition(json!([model.clone()]));
        assert!(validate_definition(&valid).is_empty());

        let missing_model = agent_definition(json!([]));
        assert!(validate_definition(&missing_model).is_empty());
        assert!(
            validate_definition(&agent_definition(json!([model.clone(), model])))
                .iter()
                .any(|issue| issue.code == "AGENT_RESOURCE_SLOT_INVALID")
        );

        let wrong_model_role = agent_definition(json!([{
            "bindingRole":"workspace_sandbox",
            "resourceType":"model",
            "resourceId":"018f47a0-7e9c-7000-8000-000000000001",
            "resourceVersionId":"018f47a0-7e9c-7000-8000-000000000002",
            "operation":"use"
        }]));
        assert!(
            validate_definition(&wrong_model_role)
                .iter()
                .any(|issue| { issue.code == "AGENT_RESOURCE_SLOT_INVALID" })
        );

        let unversioned_model = agent_definition(json!([{
            "bindingRole":"model",
            "resourceType":"model",
            "resourceId":"018f47a0-7e9c-7000-8000-000000000001",
            "operation":"use"
        }]));
        assert!(
            validate_definition(&unversioned_model)
                .iter()
                .any(|issue| issue.code == "AGENT_RESOURCE_SLOT_INVALID")
        );

        let duplicate_sandbox = agent_definition(json!([
            model,
            {
                "bindingRole":"workspace_sandbox",
                "resourceType":"sandbox_profile",
                "resourceId":"018f47a0-7e9c-7000-8000-000000000003",
                "resourceVersionId":"018f47a0-7e9c-7000-8000-000000000004",
                "operation":"use"
            },
            {
                "bindingRole":"workspace_sandbox",
                "resourceType":"sandbox_profile",
                "resourceId":"018f47a0-7e9c-7000-8000-000000000005",
                "resourceVersionId":"018f47a0-7e9c-7000-8000-000000000006",
                "operation":"use"
            }
        ]));
        assert!(
            validate_definition(&duplicate_sandbox)
                .iter()
                .any(|issue| issue.code == "AGENT_RESOURCE_SLOT_INVALID")
        );
    }

    #[test]
    fn workflow_definition_6_is_rejected_without_fallback() {
        let mut definition = WorkflowDefinition::empty();
        definition.schema_version = "6.0".into();
        assert!(
            validate_definition(&definition)
                .iter()
                .any(|issue| issue.code == "UNSUPPORTED_SCHEMA")
        );

        let mut serialized = serde_json::to_value(WorkflowDefinition::empty()).unwrap();
        serialized["schemaVersion"] = json!("6.0");
        assert!(serde_json::from_value::<WorkflowDefinition>(serialized).is_err());
    }

    fn exit_definition(
        exit_parameters: serde_json::Value,
        end: serde_json::Value,
    ) -> WorkflowDefinition {
        serde_json::from_value(json!({
            "schemaVersion":"8.0",
            "start":{"inputs":{"type":"object","properties":{},"additionalProperties":false},"contexts":{}},
            "nodes":[
                {"id":"source","key":"source","type":"set","typeVersion":1,"name":"Source"},
                {"id":"exit","key":"exit","type":"exit","typeVersion":1,"name":"End","parameters":exit_parameters}
            ],
            "connections":[
                {"id":"source-exit","sourceNodeId":"source","sourceHandle":"main","targetNodeId":"exit","targetHandle":"main","order":0}
            ],
            "end":end
        }))
        .expect("Exit Definition 8.0 fixture")
    }

    #[test]
    fn exit_mappings_follow_the_global_contract() {
        let valid = exit_definition(
            json!({"outputs":{"answer":{"kind":"literal","value":"ok"}}}),
            json!({"outputs":{"answer":{"schema":{"type":"string"},"required":true}}}),
        );
        assert!(validate_definition(&valid).is_empty());

        let unknown_key = exit_definition(
            json!({"outputs":{"ghost":{"kind":"literal","value":"ok"}}}),
            json!({"outputs":{}}),
        );
        assert!(
            validate_definition(&unknown_key)
                .iter()
                .any(|issue| issue.code == "EXIT_MAPPING_KEY_UNKNOWN")
        );

        let missing_required = exit_definition(
            json!({"outputs":{}}),
            json!({"outputs":{"answer":{"schema":{"type":"string"},"required":true}}}),
        );
        assert!(
            validate_definition(&missing_required)
                .iter()
                .any(|issue| issue.code == "EXIT_REQUIRED_MAPPING_MISSING")
        );

        let bad_parameters = exit_definition(json!("nope"), json!({"outputs":{}}));
        assert!(
            validate_definition(&bad_parameters)
                .iter()
                .any(|issue| issue.code == "EXIT_PARAMETERS_INVALID")
        );
    }

    #[test]
    fn exit_nodes_are_terminals_and_the_end_boundary_is_gone() {
        let mut exit_as_source = exit_definition(json!({}), json!({"outputs":{}}));
        exit_as_source.connections[0] = serde_json::from_value(json!({
            "id":"exit-leak","sourceNodeId":"exit","sourceHandle":"main",
            "targetNodeId":"source","targetHandle":"main","order":0
        }))
        .unwrap();
        assert!(
            validate_definition(&exit_as_source)
                .iter()
                .any(|issue| issue.code == "INVALID_BOUNDARY_DIRECTION")
        );

        let mut end_boundary = exit_definition(json!({}), json!({"outputs":{}}));
        end_boundary.connections[0] = serde_json::from_value(json!({
            "id":"legacy","sourceNodeId":"source","sourceHandle":"main",
            "targetNodeId":"__end__","targetHandle":"main","order":0
        }))
        .unwrap();
        assert!(
            validate_definition(&end_boundary)
                .iter()
                .any(|issue| issue.code == "END_BOUNDARY_REMOVED")
        );

        let mut bad_port = exit_definition(json!({}), json!({"outputs":{}}));
        bad_port.connections[0].target_handle = "side".into();
        assert!(
            validate_definition(&bad_port)
                .iter()
                .any(|issue| issue.code == "INVALID_END_PORT")
        );
    }

    #[test]
    fn editor_annotations_and_groups_apply_backward_compatible_defaults() {
        let document: EditorDocument = serde_json::from_value(json!({
            "annotations": [{"id": "note", "text": "Remember", "x": 10, "y": 20}],
            "groups": [{"id": "group", "label": "Main", "nodeIds": ["node"]}]
        }))
        .unwrap();
        assert_eq!(document.annotations[0].width, 240.0);
        assert_eq!(document.annotations[0].height, 160.0);
        assert!(!document.groups[0].collapsed);
    }

    #[test]
    fn rejects_dangling_cycles_and_mismatched_resources() {
        let definition: WorkflowDefinition = serde_json::from_value(json!({
            "schemaVersion":"8.0",
            "start":{"inputs":{"type":"object","properties":{},"additionalProperties":false},"contexts":{}},
            "nodes":[
                {"id":"root","key":"root","type":"set","typeVersion":1,"name":"Root","contextWrites":[],"resourceReferences":[]},
                {"id":"model","key":"model","type":"model","typeVersion":1,"name":"Model","contextWrites":[],"resourceReferences":[{"resourceType":"mcp_tool","resourceId":"018f47a0-7e9c-7000-8000-000000000001","operation":"use"}]}
            ],
            "connections":[
                {"id":"edge","sourceNodeId":"root","sourceHandle":"main","targetNodeId":"model","targetHandle":"main","order":0},
                {"id":"edge","sourceNodeId":"model","sourceHandle":"main","targetNodeId":"root","targetHandle":"main","order":0},
                {"id":"missing","sourceNodeId":"model","sourceHandle":"main","targetNodeId":"absent","targetHandle":"main","order":1}
            ],
            "end":{"outputs":{}},
            "settings":{}
        })).unwrap();
        let codes: Vec<_> = validate_definition(&definition)
            .into_iter()
            .map(|issue| issue.code)
            .collect();
        assert!(codes.contains(&"INVALID_RESOURCE_REFERENCE".to_owned()));
        assert!(codes.contains(&"INVALID_CONNECTION_ID".to_owned()));
        assert!(codes.contains(&"DANGLING_CONNECTION".to_owned()));
        assert!(!codes.contains(&"CYCLE_NOT_ALLOWED".to_owned()));
    }

    #[test]
    fn accepts_loop_container_membership_without_control_edges() {
        let definition: WorkflowDefinition = serde_json::from_value(json!({
            "schemaVersion":"8.0",
            "start":{"inputs":{"type":"object","properties":{},"additionalProperties":false},"contexts":{}},
            "nodes":[
                {"id":"root","key":"root","type":"set","typeVersion":1,"name":"Root","contextWrites":[]},
                {"id":"loop","key":"loop","type":"loop_over_items","typeVersion":1,"name":"Loop","parameters":{"input":{"kind":"literal","value":[]},"outputSelector":{"kind":"literal","value":{}}},"contextWrites":[]},
                {"id":"body","key":"body","type":"set","typeVersion":1,"name":"Body","parentId":"loop","contextWrites":[]},
                {"id":"exit","key":"exit","type":"exit","typeVersion":1,"name":"Exit","parameters":{"outputs":{},"errorOutputs":{}}}
            ],
            "connections":[
                {"id":"start","sourceNodeId":"__start__","sourceHandle":"main","targetNodeId":"root","targetHandle":"main","order":0},
                {"id":"root-loop","sourceNodeId":"root","sourceHandle":"main","targetNodeId":"loop","targetHandle":"main","order":0},
                {"id":"loop-exit","sourceNodeId":"loop","sourceHandle":"main","targetNodeId":"exit","targetHandle":"main","order":0}
            ],
            "end":{"outputs":{}}
        })).unwrap();
        assert!(validate_definition(&definition).is_empty());
    }

    #[test]
    fn connection_order_is_scoped_to_the_source_port() {
        let definition: WorkflowDefinition = serde_json::from_value(json!({
            "schemaVersion":"8.0",
            "start":{"inputs":{"type":"object","properties":{},"additionalProperties":false},"contexts":{}},
            "nodes":[
                {"id":"source","key":"source","type":"if","typeVersion":1,"name":"Source","parameters":{"cases":[{"id":"0","conditions":[{"condition":true}]},{"id":"1","conditions":[{"condition":false}]}]},"contextWrites":[]},
                {"id":"left","key":"left","type":"set","typeVersion":1,"name":"Left","contextWrites":[]},
                {"id":"right","key":"right","type":"set","typeVersion":1,"name":"Right","contextWrites":[]}
            ],
            "connections":[
                {"id":"left-edge","sourceNodeId":"source","sourceHandle":"case:0","targetNodeId":"left","targetHandle":"main","order":0},
                {"id":"right-edge","sourceNodeId":"source","sourceHandle":"case:1","targetNodeId":"right","targetHandle":"main","order":0}
            ],
            "end":{"outputs":{}}
        })).unwrap();
        assert!(validate_definition(&definition).is_empty());

        let mut duplicate = definition;
        duplicate.connections[1].source_handle = "case:0".into();
        assert!(
            validate_definition(&duplicate)
                .iter()
                .any(|issue| issue.code == "DUPLICATE_CONNECTION_ORDER")
        );
    }
}
