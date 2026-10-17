import type { DefinitionConnection, ResourceReference, StudioDocument, InputBinding } from "../model/types";
import { serializeStudio } from "../model/serializer";

import type { StudioIssue } from "./configuration";

export const REFERENCE_KEY_PATTERN = /^[a-z_][a-z0-9_]{0,127}$/;
const NODE_TYPE_PATTERN = /^[a-z0-9_.-]{1,128}$/;
const START_NODE_ID = "__start__";
const END_NODE_ID = "__end__";
const EXIT_NODE_TYPE = "exit";

export function isReferenceKey(value: string) {
  return REFERENCE_KEY_PATTERN.test(value);
}

export function isInputBindingEmpty(value: InputBinding) {
  if (value.kind === "literal") return value.value == null || (typeof value.value === "string" && !value.value.trim());
  return false;
}

export function definitionIssues(document: StudioDocument): StudioIssue[] {
  const issues: StudioIssue[] = [];
  const { definition } = serializeStudio(document);
  if (!isUnsignedInteger(definition.settings.activationBudget, 32) || definition.settings.activationBudget === 0) {
    issues.push(workflowIssue("INVALID_ACTIVATION_BUDGET", "settings.activationBudget", "Activation budget must be greater than zero."));
  }
  if (!isObject(definition.start.inputs)) {
    issues.push(workflowIssue("INVALID_INPUT_SCHEMA", "start.inputs", "Start inputs must be a JSON Schema object."));
  }
  for (const [name, context] of Object.entries(definition.start.contexts)) {
    if (!isReferenceKey(name)) issues.push(workflowIssue("INVALID_CONTEXT_KEY", `start.contexts.${name}`, "Global variable names may contain only lowercase letters, digits, and underscores."));
    if (!isObject(context.schema)) issues.push(workflowIssue("INVALID_CONTEXT_SCHEMA", `start.contexts.${name}.schema`, "Global variable schema must be an object."));
    if (context.maxSize != null && (!Number.isSafeInteger(context.maxSize) || context.maxSize <= 0)) issues.push(workflowIssue("INVALID_CONTEXT_MAX_SIZE", `start.contexts.${name}.maxSize`, "Global variable maximum size must be greater than zero."));
  }
  validateOutputs(definition.end.outputs, "end.outputs", issues);
  validateOutputs(definition.end.error.outputs, "end.error.outputs", issues);
  const exitNodes = definition.nodes.filter((node) => node.type === EXIT_NODE_TYPE);
  if (!exitNodes.length) issues.push(workflowIssue("EXIT_REQUIRED", "nodes", "At least one end node is required."));
  for (const node of exitNodes) {
    const parameters = node.parameters as { outputs?: Record<string, InputBinding>; errorOutputs?: Record<string, InputBinding> };
    validateExitMappings(node.id, parameters.outputs ?? {}, definition.end.outputs, "outputs", issues, definition.connections.some(edge => edge.targetNodeId === node.id && edge.targetHandle === "main"));
    validateExitMappings(node.id, parameters.errorOutputs ?? {}, definition.end.error.outputs, "errorOutputs", issues, definition.connections.some(edge => edge.targetNodeId === node.id && edge.targetHandle === "error"));
  }

  const ids = new Set<string>();
  const keys = new Set<string>();
  for (const node of definition.nodes) {
    const nodeIssue = (code: string, fieldPath: string, message: string): StudioIssue => ({ code, nodeId: node.id, fieldPath, message });
    if (node.id === START_NODE_ID || node.id === END_NODE_ID) issues.push(nodeIssue("RESERVED_NODE_ID", "id", "Node id is reserved for a Workflow boundary."));
    else if (!node.id || utf8Length(node.id) > 128) issues.push(nodeIssue("INVALID_NODE_ID", "id", "Node id must contain 1 to 128 bytes."));
    else if (ids.has(node.id)) issues.push(nodeIssue("DUPLICATE_NODE_ID", "id", "Node id must be unique."));
    else ids.add(node.id);

    if (!isReferenceKey(node.key)) issues.push(nodeIssue("INVALID_NODE_KEY", "key", "Node key may contain only lowercase letters, digits, and underscores."));
    else if (keys.has(node.key)) issues.push(nodeIssue("DUPLICATE_NODE_KEY", "key", "Node key must be unique within the workflow."));
    keys.add(node.key);

    if (node.type === "manual_trigger" || node.type === "remote_trigger") issues.push(nodeIssue("TRIGGER_NODE_REMOVED", "type", "Trigger nodes are not supported in Workflow Definition 8.0."));
    if (!NODE_TYPE_PATTERN.test(node.type)) issues.push(nodeIssue("INVALID_NODE_TYPE", "type", "Node type may contain only lowercase letters, digits, dots, underscores, and hyphens."));
    if (!node.name.trim() || [...node.name].length > 160) issues.push(nodeIssue("INVALID_NODE_NAME", "name", "Node name must contain 1 to 160 characters."));
    if (!isUnsignedInteger(node.typeVersion, 32) || node.typeVersion === 0) issues.push(nodeIssue("UNSUPPORTED_NODE_VERSION", "typeVersion", "Node type version must be greater than zero."));
    const maxTries = node.settings.maxTries ?? 1;
    if (typeof maxTries !== "number" || !isUnsignedInteger(maxTries, 16) || maxTries === 0) issues.push(nodeIssue("INVALID_MAX_TRIES", "settings.maxTries", "Maximum attempts must be greater than zero."));

    if (isResourceNode(node.type)) {
      for (const reference of node.resourceReferences) {
        if (!referenceMatchesNode(node.type, reference)) issues.push(nodeIssue("INVALID_RESOURCE_REFERENCE", "resourceReferences", "Resource type or operation does not match the node type."));
      }
    }
    if (node.type === "agent") {
      const sessionMode = (node.parameters.sessionPolicy as { mode?: unknown } | undefined)?.mode;
      if (sessionMode !== "application_session" && sessionMode !== "invocation") issues.push(nodeIssue("AGENT_SESSION_POLICY_INVALID", "parameters.sessionPolicy", "Select application_session or invocation explicitly."));
      const modelReferences = node.resourceReferences.filter((reference) => reference.bindingRole === "model" && reference.resourceType === "model");
      const sandboxReferences = node.resourceReferences.filter((reference) => reference.bindingRole === "workspace_sandbox" && reference.resourceType === "sandbox_profile");
      const memoryReferences = node.resourceReferences.filter((reference) => reference.bindingRole === "long_term_memory");
      if (modelReferences.length > 1) issues.push(nodeIssue("AGENT_RESOURCE_SLOT_INVALID", "resourceReferences.model", "Agent accepts at most one internal Model reference."));
      if (sandboxReferences.length > 1 || memoryReferences.length > 1) issues.push(nodeIssue("AGENT_RESOURCE_SLOT_INVALID", "resourceReferences", "Agent accepts at most one Workspace Sandbox and one Long-term Memory attachment."));
      for (const reference of node.resourceReferences) {
        const versionPinned = reference.resourceType !== "rag" && reference.resourceType !== "memory";
        if (!reference.bindingRole || (versionPinned && !reference.resourceVersionId)) issues.push(nodeIssue("AGENT_RESOURCE_SLOT_INVALID", "resourceReferences", "Every Agent resource must use a frozen bindingRole; versioned resources must pin an exact version."));
      }
    }
  }

  validateConnections(definition.connections, ids, new Set(exitNodes.map((node) => node.id)), issues);
  return issues;
}

function validateOutputs(outputs: StudioDocument["end"]["outputs"], path: string, issues: StudioIssue[]) {
  for (const [name, output] of Object.entries(outputs)) {
    if (!isReferenceKey(name)) issues.push(workflowIssue("INVALID_END_OUTPUT_KEY", `${path}.${name}`, "End output names may contain only lowercase letters, digits, and underscores."));
    if (!isObject(output.schema)) issues.push(workflowIssue("INVALID_END_OUTPUT_SCHEMA", `${path}.${name}.schema`, "End output schema must be an object."));
  }
}

function validateExitMappings(nodeId: string, mappings: Record<string, InputBinding>, contract: StudioDocument["end"]["outputs"], group: string, issues: StudioIssue[], connected: boolean) {
  for (const [name, value] of Object.entries(mappings)) {
    if (!(name in contract)) issues.push({ code: "EXIT_MAPPING_KEY_UNKNOWN", nodeId, fieldPath: `parameters.${group}.${name}`, message: "End node mappings must reference fields declared in the global output contract." });
    if (isInputBindingEmpty(value)) issues.push({ code: "END_OUTPUT_BINDING_REQUIRED", nodeId, fieldPath: `parameters.${group}.${name}`, message: "End output bindings are required." });
  }
  for (const [name, output] of Object.entries(contract)) {
    if (connected && output.required && !(name in mappings)) issues.push({ code: "EXIT_REQUIRED_MAPPING_MISSING", nodeId, fieldPath: `parameters.${group}.${name}`, message: "Connected end inputs must map each required output field." });
  }
}

function validateConnections(connections: DefinitionConnection[], nodeIds: Set<string>, exitIds: Set<string>, issues: StudioIssue[]) {
  const ids = new Set<string>();
  const orders = new Set<string>();
  const terminalFanouts = new Set<string>();
  for (const connection of connections) {
    if (!connection.id || ids.has(connection.id)) issues.push(workflowIssue("INVALID_CONNECTION_ID", "connections", "Connection id must be present and unique."));
    ids.add(connection.id);
    if (!connection.sourceHandle || !connection.targetHandle) issues.push(workflowIssue("INVALID_CONNECTION_HANDLE", "connections", "Connection handles are required."));
    const orderKey = `${connection.sourceNodeId}\u0000${connection.sourceHandle}\u0000${connection.order}`;
    if (orders.has(orderKey)) issues.push(workflowIssue("DUPLICATE_CONNECTION_ORDER", "connections", "Connection order must be unique for each source node and source handle."));
    orders.add(orderKey);

    const sourceIsStart = connection.sourceNodeId === START_NODE_ID;
    const boundaryRef = connection.sourceNodeId === END_NODE_ID || connection.targetNodeId === END_NODE_ID || connection.targetNodeId === START_NODE_ID;
    if (boundaryRef) {
      issues.push(workflowIssue("END_BOUNDARY_REMOVED", "connections", "Workflow Definition 8.0 uses Exit nodes; connect to an Exit node instead."));
      continue;
    }
    if (sourceIsStart && connection.sourceHandle !== "main") issues.push(workflowIssue("INVALID_START_PORT", "connections", "Start only exposes the main output port."));
    if (exitIds.has(connection.targetNodeId)) {
      const fanoutKey = `${connection.sourceNodeId}\u0000${connection.sourceHandle}`;
      if (terminalFanouts.has(fanoutKey)) issues.push(workflowIssue("DUPLICATE_TERMINAL_FANOUT", "connections", "A node port may only connect to a single end node."));
      terminalFanouts.add(fanoutKey);
    }
    if ((!sourceIsStart && !nodeIds.has(connection.sourceNodeId)) || !nodeIds.has(connection.targetNodeId)) issues.push(workflowIssue("DANGLING_CONNECTION", "connections", "Connection references a missing node."));
  }
}

function referenceMatchesNode(nodeType: string, reference: ResourceReference) {
  if (nodeType === "knowledge_retrieval") return reference.resourceType === "rag" && reference.operation === "read" && !reference.bindingRole;
  if (nodeType === "model") return reference.resourceType === "model" && reference.operation === "use";
  if (nodeType === "code") return reference.resourceType === "sandbox_profile" && reference.operation === "use";
  if (nodeType === "declarative_http") return reference.bindingRole === "credential" && reference.resourceType === "credential" && reference.operation === "use";
  if (nodeType !== "agent") return false;
  if (reference.bindingRole === "model" && reference.resourceType === "model") return reference.operation === "use" && Boolean(reference.resourceVersionId);
  if (reference.bindingRole === "workspace_sandbox" && reference.resourceType === "sandbox_profile") return reference.operation === "use" && Boolean(reference.resourceVersionId);
  const role = reference.bindingRole;
  if (role === "mcp_tools") return reference.resourceType === "mcp_tool" && reference.operation === "use";
  if (role === "skills") return reference.resourceType === "skill" && reference.operation === "use";
  if (role === "knowledge") return reference.resourceType === "rag" && reference.operation === "read";
  return role === "long_term_memory" && reference.resourceType === "memory" && reference.operation === "use";
}

function isResourceNode(nodeType: string) {
  return ["model", "agent", "code", "declarative_http", "knowledge_retrieval"].includes(nodeType);
}

function isObject(value: unknown): value is Record<string, unknown> {
  return Boolean(value && typeof value === "object" && !Array.isArray(value));
}

function isUnsignedInteger(value: number, bits: 16 | 32) {
  const maximum = bits === 16 ? 0xffff : 0xffffffff;
  return Number.isInteger(value) && value >= 0 && value <= maximum;
}

function utf8Length(value: string) {
  return new TextEncoder().encode(value).length;
}

function workflowIssue(code: string, fieldPath: string, message: string): StudioIssue {
  return { code, fieldPath, message };
}
