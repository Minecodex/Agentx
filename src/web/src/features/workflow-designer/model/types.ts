import type { Edge, Node, Viewport } from "@xyflow/react";

export type PortKind = "main" | "error";
export type ResourceType =
  | "credential"
  | "model"
  | "mcp_server"
  | "mcp_tool"
  | "skill"
  | "rag"
  | "memory"
  | "sandbox_profile";

export type NodePort = {
  name: string;
  kind: PortKind;
  required: boolean;
  variadic: boolean;
};
export type BindingSlot = {
  name: string;
  resourceType: ResourceType;
  placement: "inspector" | "canvas";
  required: boolean;
  multiple: boolean;
};
export type JsonSchemaProperty = {
  type?: string | string[];
  title?: string;
  description?: string;
  default?: unknown;
  enum?: unknown[];
  minimum?: number;
  maximum?: number;
  multipleOf?: number;
  minLength?: number;
  maxLength?: number;
  pattern?: string;
  minItems?: number;
  maxItems?: number;
  uniqueItems?: boolean;
  items?: JsonSchemaProperty;
  properties?: Record<string, JsonSchemaProperty>;
  additionalProperties?: boolean | JsonSchemaProperty;
  required?: string[];
  format?: string;
  "x-agentx-binding"?: {
    acceptedKinds: Array<"literal" | "reference" | "template" | "array" | "object">;
    allowedNamespaces: ValueNamespace[];
    acceptedCardinality: Array<"single" | "many">;
    missingPolicies: Array<"error" | "null" | "omit">;
    recursive: boolean;
  };
  multiline?: boolean;
};
export type ParameterSchema = {
  type?: string;
  required?: string[];
  properties?: Record<string, JsonSchemaProperty>;
  additionalProperties?: boolean;
  "x-agentx-workflowId"?: string;
  "x-agentx-workflowVersionId"?: string;
  "x-agentx-versionNumber"?: number;
  "x-agentx-contextContract"?: Record<string, ContextDefinition>;
};
export type CanvasNodeRole =
  | "default"
  | "trigger"
  | "branch"
  | "flow"
  | "merge"
  | "loop"
  | "suspend"
  | "approval"
  | "sub_workflow"
  | "agent"
  | "code";
export type CanvasNodeFamily = "compact" | "agent";
export type ConnectionInteractionState =
  | { status: "idle" | "cancelled" | "committed" }
  | {
      status: "source-hover" | "connecting";
      nodeId: string;
      handleId?: string | null;
    }
  | { status: "compatible" | "incompatible" | "occupied"; reason?: string };
export type UiField = {
  control: string;
  label?: string;
  unit?: string;
  languageField?: string;
  provider?: string;
  options?: Array<{ value: string; label: string }>;
  visibleWhen?: { field: string; equals: unknown };
};
export type NodeUiSchema = {
  order?: string[];
  fields?: Record<string, UiField>;
  resourceSelectors?: unknown[];
  canvas?: { role?: CanvasNodeRole };
};

export type NodeManifest = {
  protocolVersion: string;
  nodeType: string;
  version: number;
  displayName: string;
  description: string;
  category: string;
  keywords: string[];
  iconKey: string;
  executionStyle: "action" | "trigger" | "suspend" | "sub_workflow";
  capability: string;
  readiness: "any" | "all" | "required";
  inputPorts: NodePort[];
  outputPorts: NodePort[];
  bindingSlots: BindingSlot[];
  parameterSchema: ParameterSchema;
  outputSchema?: unknown;
  outputPortSchemas?: Record<string, unknown>;
  outputCardinality?: Record<
    string,
    "zero_or_one" | "exactly_one" | "zero_or_many" | "many"
  >;
  selectorCapabilities?: {
    namespaces?: string[];
    supportsCurrent?: boolean;
    supportsFirstLast?: boolean;
    supportsAll?: boolean;
    supportsRunSelection?: boolean;
  };
  contextReadCapability?: boolean;
  contextWriteCapability?: boolean;
  artifactOutputSchema?: unknown;
  uiSchema: NodeUiSchema;
  plugin?: {
    packageId: string;
    packageVersion: string;
    bundleDigest: string;
    runtimeEntry: string;
    runtimeSource: string;
    runtimeArtifact?: { objectId: string; contentHash: string; sizeBytes: number; mediaType: string } | null;
    uiEntry?: string | null;
    uiSource?: string | null;
    uiStyles?: string | null;
    uiAssets?: Record<string, string>;
    traceRenderers?: Array<{ contentType: string; contentVersion: number; exportName: string; schema: unknown }>;
  };
  providers: string[];
  credentials: Array<{ credentialType: string; required: boolean }>;
  defaultTimeoutMs?: number | null;
  retryPolicy: {
    retryable: boolean;
    maxAttempts: number;
    initialBackoffMs: number;
    maxBackoffMs: number;
  };
  sandboxRequired: boolean;
  supportsMock: boolean;
  sideEffectLevel: "none" | "idempotent" | "reversible" | "irreversible";
  localizations?: Record<string, NodeManifestLocalization>;
};

export type NodeManifestLocalization = {
  displayName?: string;
  description?: string;
  keywords?: string[];
  inputPortLabels?: Record<string, string>;
  outputPortLabels?: Record<string, string>;
  bindingSlotLabels?: Record<string, string>;
  parameterLabels?: Record<string, string>;
  parameterDescriptions?: Record<string, string>;
  parameterPlaceholders?: Record<string, string>;
  parameterEnumOptions?: Record<string, Record<string, string>>;
};

export type ResourceReference = {
  bindingRole?: string;
  resourceType: ResourceType;
  resourceId: string;
  resourceVersionId?: string | null;
  operation: "view" | "use" | "read" | "write" | "manage";
};

export type DefinitionNode = {
  id: string;
  key: string;
  type: string;
  typeVersion: number;
  name: string;
  disabled: boolean;
  protected: boolean;
  parentId?: string;
  parameters: Record<string, unknown>;
  contextWrites: ContextWrite[];
  resourceReferences: ResourceReference[];
  settings: Record<string, unknown>;
};
export type ValueNamespace = "inputs" | "outputs" | "contexts" | "execution" | "item" | "loop";
export type ValueSelection = { kind: "current" | "first" | "last" | "all" } | { kind: "index"; index: number };
export type ValueSelector = {
  namespace: ValueNamespace;
  sourceNodeId?: string;
  port?: string;
  run?: ValueSelection;
  item?: ValueSelection;
  path?: Array<string | number>;
};
export type MissingValuePolicy =
  | { kind: "error" }
  | { kind: "null" }
  | { kind: "omit" };
export type InputTemplateSegment =
  | { kind: "text"; text: string }
  | { kind: "reference"; selector: ValueSelector; missingPolicy?: MissingValuePolicy };
export type InputBinding =
  | { kind: "literal"; value: unknown }
  | { kind: "reference"; selector: ValueSelector; missingPolicy?: MissingValuePolicy }
  | { kind: "template"; segments: InputTemplateSegment[] }
  | { kind: "array"; items: InputBinding[] }
  | { kind: "object"; fields: Record<string, InputBinding> };
export type ReferenceBinding = Extract<InputBinding, { kind: "reference" }>;
export type TemplateBinding = Extract<InputBinding, { kind: "template" }>;
export type ConditionOperator = "eq" | "ne" | "gt" | "gte" | "lt" | "lte" | "in" | "contains" | "not_contains" | "ends_with" | "starts_with" | "matches" | "is_empty" | "is_not_empty";
export type ConditionSpec = { left: InputBinding; operator: ConditionOperator; right?: InputBinding };
export type DefinitionConnection = {
  id: string;
  sourceNodeId: string;
  sourceHandle: string;
  targetNodeId: string;
  targetHandle: string;
  order: number;
};
export type WorkflowSettings = {
  executionOrder: "deterministic" | "parallel";
  activationBudget: number;
  timeoutMs?: number | null;
};
export type ContextDefinition = {
  title?: string | null;
  description?: string | null;
  schema: unknown;
  default: unknown;
  mutable: boolean;
  sensitive: boolean;
  clientWritable: boolean;
  scope: "execution_tree" | "session";
  maxSize?: number | null;
  mergePolicy:
    "replace" | "append" | "merge_object" | "increment" | "reject_conflict";
};
export type ContextWrite = {
  operation:
    | "set"
    | "set_if_absent"
    | "delete"
    | "append"
    | "merge_object"
    | "increment"
    | "min"
    | "max"
    | "compare_and_set";
  path: string;
  value: InputBinding;
};
export type WorkflowOutput = {
  schema: unknown;
  required: boolean;
  sensitive: boolean;
};
export type ExitParameters = {
  outputs: Record<string, InputBinding>;
  errorOutputs: Record<string, InputBinding>;
};
export type WorkflowCompletion = "first_return" | "all_complete";
export type WorkflowErrorEnd = {
  outputs: Record<string, WorkflowOutput>;
};
export type WorkflowStart = {
  inputs: unknown;
  contexts: Record<string, ContextDefinition>;
};
export type WorkflowEnd = { completion: WorkflowCompletion; outputs: Record<string, WorkflowOutput>; error: WorkflowErrorEnd };
export type WorkflowDefinition = {
  schemaVersion: "8.0";
  start: WorkflowStart;
  nodes: DefinitionNode[];
  connections: DefinitionConnection[];
  end: WorkflowEnd;
  settings: WorkflowSettings;
};

export type EditorDocument = {
  nodeLayouts: Array<{
    nodeId: string;
    x: number;
    y: number;
    width?: number;
    height?: number;
    collapsed?: boolean;
  }>;
  boundaryLayouts: Array<{ boundary: "start"; x: number; y: number }>;
  edges: Array<{ edgeId: string; labelPosition?: number }>;
  annotations: Array<{
    id: string;
    text: string;
    x: number;
    y: number;
    width?: number;
    height?: number;
    color?: string;
  }>;
  groups: Array<{
    id: string;
    label: string;
    nodeIds: string[];
    collapsed?: boolean;
    color?: string;
  }>;
  viewport: Viewport;
};

export type ActionNodeData = {
  editorKind: "action";
  nodeType: string;
  typeVersion: number;
  label: string;
  key: string;
  parentId?: string;
  parameters: Record<string, unknown>;
  contextWrites: ContextWrite[];
  resourceReferences: ResourceReference[];
  settings: Record<string, unknown>;
  disabled: boolean;
};
export type GroupNodeData = {
  editorKind: "group";
  groupId: string;
  label: string;
  collapsed: boolean;
  color?: string;
  memberCount: number;
  onToggle: () => void;
  onRemove: () => void;
};
export type AnnotationNodeData = {
  editorKind: "annotation";
  annotationId: string;
  text: string;
  color?: string;
  onChange: (patch: Partial<EditorDocument["annotations"][number]>) => void;
  onRemove: () => void;
  onResizeStart: () => void;
  onResize: (frame: {
    x: number;
    y: number;
    width: number;
    height: number;
  }) => void;
  onResizeEnd: () => void;
};
export type BoundaryNodeData = {
  editorKind: "boundary";
  boundary: "start";
  label: string;
};
/** View-only projection of a loop_over_items node that owns body children. */
export type LoopContainerNodeData = {
  editorKind: "loop-container";
  loopId: string;
  label: string;
  parallelism: number;
  childCount: number;
};
/** Edit-only iteration entry chip: never serialized into the Definition. */
export type IterationChipNodeData = {
  editorKind: "iteration-chip";
  loopId: string;
};
/** Edit-only iteration exit chip: body sinks converge here in the editor only. */
export type IterationEndNodeData = {
  editorKind: "iteration-end";
  loopId: string;
};
export type ExitNodeData = {
  editorKind: "exit";
  key: string;
  label: string;
  protected: boolean;
  parameters: ExitParameters;
};
export type StudioNodeData = ActionNodeData | ExitNodeData;
export type StudioNode = Node<StudioNodeData, "manifest" | "exit"> & { editorCollapsed?: boolean };
export type CanvasNodeData =
  | StudioNodeData
  | GroupNodeData
  | AnnotationNodeData
  | BoundaryNodeData
  | LoopContainerNodeData
  | IterationChipNodeData
  | IterationEndNodeData;
export type CanvasNode = Node<
  CanvasNodeData,
  | "manifest"
  | "exit"
  | "group"
  | "annotation"
    | "boundary"
    | "loop-container"
    | "iteration-chip"
    | "iteration-end"
>;
export type StudioEdgeData = {
  edgeKind: "execution";
  order?: number;
  sourcePortKind?: PortKind;
  /** Derived from the target contract; never serialized into a connection. */
  targetPortKind?: PortKind;
  runtimeStatus?: string;
  labelPosition?: number;
  /** Purely projected loop boundary links; never placed in the editor document. */
  derived?: boolean;
};
export type StudioEdge = Edge<StudioEdgeData>;

export type StudioDocument = {
  start: WorkflowStart;
  nodes: StudioNode[];
  edges: StudioEdge[];
  end: WorkflowEnd;
  viewport: Viewport;
  boundaryLayouts: EditorDocument["boundaryLayouts"];
  annotations: EditorDocument["annotations"];
  groups: EditorDocument["groups"];
  settings: WorkflowSettings;
};
export type ResourceOption = {
  value: string;
  label: string;
  resourceType?: ResourceType;
  operation?: "view" | "use" | "read" | "write" | "manage";
  versionId?: string | null;
  detail?: string;
  status?: string;
  accessState?: "authorized" | "grantable" | "requestable" | "pending" | "rejected" | "unavailable";
  pendingRequestId?: string | null;
  manifest?: NodeManifest;
  metadata?: {
    connectionId?: string;
    connectionName?: string;
    description?: string;
    scope?: { kind: "ragflow"; datasetIds: string[] } | { kind: "lightrag"; workspace: string };
    maxInputTokens?: number;
    maxOutputTokens?: number;
    currency?: string | null;
  };
  requirements?: Array<{
    resourceType: string;
    resourceId: string;
    resourceVersionId?: string | null;
    operation: string;
    name?: string | null;
    requiredByResourceId?: string | null;
    ownerDepartmentId?: string | null;
    authorized: boolean;
    active: boolean;
  }>;
};
export type ResourceRequestContext = {
  sourceNodeId?: string;
  message?: string;
};
export type ReferenceNamespace =
  | "inputs"
  | "outputs"
  | "contexts"
  | "item"
  | "execution"
  | "loop";
export type ReferenceEntry = {
  id: string;
  label: string;
  path: string;
  selector?: ValueSelector;
  sourceNodeType?: string;
  type?: string;
  schema?: JsonSchemaProperty;
  cardinality?: "zero_or_one" | "exactly_one" | "many" | "zero_or_many";
  nullable?: boolean;
  sensitive?: boolean;
  scope?: "execution_tree" | "session";
  example?: unknown;
  disabledReason?: string;
  conversionNote?: string;
  recommended?: boolean;
  description?: string;
  children: ReferenceEntry[];
};
export type ReferenceCatalog = Record<
  "inputs" | "outputs" | "contexts",
  ReferenceEntry[]
> &
  Partial<Record<"item" | "execution" | "loop", ReferenceEntry[]>>;

export const emptyEditorDocument = (): EditorDocument => ({
  nodeLayouts: [],
  boundaryLayouts: [],
  edges: [],
  annotations: [],
  groups: [],
  viewport: { x: 0, y: 0, zoom: 1 },
});
