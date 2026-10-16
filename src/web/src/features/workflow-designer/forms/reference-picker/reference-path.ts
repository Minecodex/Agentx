import { selectorWithDefaults } from "../../model/selector";
import type {
  JsonSchemaProperty,
  NodeManifest,
  ReferenceCatalog,
  ReferenceEntry,
  StudioDocument,
  ValueSelector,
} from "../../model/types";
import { selectorsEqual } from "../../model/selector";
import { isIterationEndId, loopIdOfIterationEnd } from "../../utils/connections";

const EMPTY_CATALOG: ReferenceCatalog = {
  inputs: [],
  outputs: [],
  contexts: [],
};

type Localize = (key: string, fallback: string) => string;

export function buildReferenceCatalog(
  document: Pick<StudioDocument, "start" | "nodes" | "edges">,
  manifests: Map<string, NodeManifest>,
  targetNodeId?: string,
  localize: Localize = (_key, fallback) => fallback,
  targetPort?: "main" | "error",
): ReferenceCatalog {
  const catalog = structuredClone(EMPTY_CATALOG);
  const inputSchema = asSchema(document.start.inputs);
  catalog.inputs = schemaEntries(
    inputSchema,
    "inputs",
    "inputs",
    false,
    baseSelector("inputs"),
  );
  catalog.contexts = Object.entries(document.start.contexts).map(
    ([name, definition]) => {
      const path = appendSegment("contexts", name);
      const schema = asSchema(definition.schema);
      return {
        id: path,
        label: name,
        path,
        selector: baseSelector("contexts", [name]),
        type: primaryType(schema),
        schema,
        nullable: false,
        sensitive: definition.sensitive,
        scope: definition.scope,
        example: definition.default,
        children: schemaEntries(
          schema,
          path,
          path,
          definition.sensitive,
          baseSelector("contexts", [name]),
        ),
      };
    },
  );
  const predecessorIds = targetNodeId
    ? reachablePredecessors(document, targetNodeId, targetPort)
    : new Set<string>();
  catalog.outputs = document.nodes.flatMap((node) => {
    const nodeData = node.data;
    if (nodeData.editorKind !== "action" || !predecessorIds.has(node.id)) return [];
    const manifest = resolveNodeManifest(manifests, node.id, nodeData.nodeType, nodeData.typeVersion, nodeData.parameters);
    if (!manifest) return [];
    const nodePath = appendSegment("outputs", nodeData.key);
    const instancePorts = instanceOutputPorts(manifest, nodeData.parameters, localize);
    const projectedSchemaForPort = (port: InstanceOutputPort) => effectiveNodeOutputSchema(document, node.id, port.name, manifests, localize, new Set());
    const ports = instancePorts.map((port) => {
      const path = appendSegment(nodePath, port.name);
      const cardinality = manifest.outputCardinality?.[port.schemaName] ?? "many";
      const projectedSchema = projectedSchemaForPort(port);
      const itemFields = recommendAiText(outputEntries(
        nodeData.nodeType,
        projectedSchema,
        `${path}.current.json`,
        `${path}.current.json`,
        false,
        outputSelector(node.id, port.name, { kind: "current" }),
      ), nodeData.nodeType);
      const selectors: ReferenceEntry[] = [];
      if (manifest.selectorCapabilities?.supportsCurrent !== false)
        selectors.push(selector("current", path, itemFields, cardinality, node.id, port.name, projectedSchema));
      if (manifest.selectorCapabilities?.supportsFirstLast !== false) {
        selectors.push(
          selector("first", path, itemFields, cardinality, node.id, port.name, projectedSchema),
          selector("last", path, itemFields, cardinality, node.id, port.name, projectedSchema),
        );
      }
      if (manifest.selectorCapabilities?.supportsAll !== false)
        selectors.push({
          id: `${path}.all()`,
          label: "all()",
          path: `${path}.all()`,
          selector: outputSelector(node.id, port.name, { kind: "all" }),
          type: "array",
          schema: { type: "array", items: projectedSchema },
          cardinality,
          nullable: false,
          children: [],
        });
      return {
        id: path,
        label: port.label,
        path,
        cardinality,
        nullable: cardinality === "zero_or_one" || cardinality === "zero_or_many",
        children: selectors,
      };
    });
    const runs =
      manifest.selectorCapabilities?.supportsRunSelection === false
        ? []
        : [
            {
              id: `${nodePath}.runs`,
              label: "runs",
              path: `${nodePath}.runs`,
              children: [
                {
                  id: `${nodePath}.runs["0"]`,
                  label: "run 0",
                  path: `${nodePath}.runs["0"]`,
                  children: instancePorts.map((port) => {
                    const path = `${nodePath}.runs["0"].${port.name}[0].json`;
                    const fields = recommendAiText(outputEntries(
                      nodeData.nodeType,
                      projectedSchemaForPort(port),
                      path,
                      path,
                      false,
                      outputSelector(
                        node.id,
                        port.name,
                        { kind: "index", index: 0 },
                        { kind: "index", index: 0 },
                      ),
                    ), nodeData.nodeType);
                    return {
                      id: path,
                      label: port.label,
                      path,
                      selector: outputSelector(
                        node.id,
                        port.name,
                        { kind: "index", index: 0 },
                        { kind: "index", index: 0 },
                      ),
                      type: "object",
                      schema: projectedSchemaForPort(port),
                      cardinality:
                        manifest.outputCardinality?.[port.schemaName] ?? "many",
                      nullable: true,
                      children: fields,
                    };
                  }),
                },
              ],
            } satisfies ReferenceEntry,
          ];
    return [
      {
        id: nodePath,
        label: nodeData.key,
        path: nodePath,
        sourceNodeType: nodeData.nodeType,
        children: [...ports, ...runs],
      },
    ];
  });
  catalog.execution = executionEntries(localize);
  const virtualLoopId = targetNodeId && isIterationEndId(targetNodeId) ? loopIdOfIterationEnd(targetNodeId) : undefined;
  const selected = targetNodeId ? document.nodes.find((node) => node.id === (virtualLoopId ?? targetNodeId)) : undefined;
  const selectedData = selected?.data.editorKind === "action" ? selected.data : undefined;
  const loop = selectedData?.nodeType === "loop_over_items"
    ? selectedData
    : selectedData?.parentId
      ? document.nodes.find((node) => node.id === selectedData.parentId && node.data.editorKind === "action" && node.data.nodeType === "loop_over_items")?.data
      : undefined;
  if (loop?.editorKind === "action") catalog.loop = loopEntries(document, manifests, localize, loop.parameters.input);
  if (selectedData?.nodeType === "list") catalog.item = itemEntries(catalog, selectedData.parameters.input);
  return catalog;
}

function loopEntries(
  document: Pick<StudioDocument, "start" | "nodes" | "edges">,
  manifests: Map<string, NodeManifest>,
  localize: Localize,
  input: unknown,
): ReferenceEntry[] {
  const inputSchema = bindingSchema(document, input, manifests, localize, new Set());
  const schema = primaryType(inputSchema) === "array" ? asSchema(inputSchema.items) : {};
  const itemSelector = baseSelector("loop", ["item"]);
  const itemsSelector = baseSelector("loop", ["items"]);
  return [
    {
      id: "loop.item",
      label: "item",
      path: "loop.item",
      selector: itemSelector,
      type: primaryType(schema) ?? "object",
      schema,
      nullable: false,
      children: schemaEntries(schema, "loop.item", "loop.item", false, itemSelector),
    },
    {
      id: "loop.items",
      label: "items",
      path: "loop.items",
      selector: itemsSelector,
      type: "array",
      schema: primaryType(inputSchema) === "array" ? inputSchema : { type: "array", items: schema },
      nullable: false,
      children: [],
    },
    {
      id: "loop.index",
      label: "index",
      path: "loop.index",
      selector: baseSelector("loop", ["index"]),
      type: "integer",
      schema: { type: "integer" },
      nullable: false,
      children: [],
    },
  ];
}

function itemEntries(
  catalog: ReferenceCatalog,
  input: unknown,
): ReferenceEntry[] {
  const schema = arrayItemSchema(catalog, input);
  return schemaEntries(schema, "item", "item", false, baseSelector("item"));
}

function arrayItemSchema(catalog: ReferenceCatalog, input: unknown): JsonSchemaProperty {
  const selector = dynamicSelector(input);
  if (!selector) return {};
  const schema = findSelectorSchema(catalog, selector);
  return primaryType(schema) === "array" ? asSchema(schema.items) : {};
}

function findSelectorSchema(catalog: ReferenceCatalog, selector: ValueSelector): JsonSchemaProperty {
  const visit = (entries: ReferenceEntry[]): JsonSchemaProperty | undefined => {
    for (const entry of entries) {
      if (entry.selector && selectorsEqual(entry.selector, selector)) return entry.schema;
      const child = visit(entry.children);
      if (child) return child;
    }
    return undefined;
  };
  for (const entries of Object.values(catalog)) {
    const schema = visit(entries ?? []);
    if (schema) return schema;
  }
  return {};
}

function dynamicSelector(value: unknown): ValueSelector | undefined {
  if (!value || typeof value !== "object") return undefined;
  const dynamic = value as { kind?: unknown; selector?: unknown };
  if (dynamic.kind === "reference") return dynamic.selector as ValueSelector;
  return undefined;
}

function executionEntries(localize: Localize): ReferenceEntry[] {
  const label = (name: string, fallback: string) =>
    localize(`studio.executionReferences.${name}`, fallback);
  const partial = label("partial", "Empty for some trigger types");
  const leaf = (
    name: string,
    fallback: string,
    path: Array<string | number>,
    type: string,
    nullable = false,
  ): ReferenceEntry => ({
    id: `execution.${path.join(".")}`,
    label: label(name, fallback),
    path: `execution.${path.join(".")}`,
    selector: baseSelector("execution", path),
    type,
    schema: { type },
    nullable,
    description: nullable ? partial : undefined,
    children: [],
  });
  const group = (name: string, fallback: string, children: ReferenceEntry[]): ReferenceEntry => ({
    id: `execution.group.${name}`,
    label: label(name, fallback),
    path: `execution.${name}`,
    children,
  });
  return [{
    id: "execution.root",
    label: label("root", "Execution information"),
    path: "execution",
    children: [
      group("execution", "Execution", [
        leaf("executionId", "Execution ID", ["id"], "string"),
        leaf("startedAt", "Started at", ["startedAt"], "string"),
        leaf("parentExecutionId", "Parent execution ID", ["parentExecutionId"], "string", true),
      ]),
      group("node", "Current node", [
        leaf("nodeId", "Node ID", ["node", "id"], "string"),
        leaf("nodeExecutionId", "Node execution ID", ["node", "executionId"], "string"),
        leaf("runIndex", "Run index", ["node", "runIndex"], "integer"),
        leaf("itemIndex", "Item index", ["node", "itemIndex"], "integer", true),
        leaf("loopIterationIndex", "Loop iteration index", ["node", "loopIterationIndex"], "integer", true),
      ]),
      group("workflow", "Workflow", [
        leaf("workflowId", "Workflow ID", ["workflow", "id"], "string"),
        leaf("workflowName", "Workflow name", ["workflow", "name"], "string"),
        leaf("workflowVersionId", "Version ID", ["workflow", "versionId"], "string"),
        leaf("workflowVersionNumber", "Version number", ["workflow", "versionNumber"], "integer"),
        leaf("ownerDepartmentId", "Owner department ID", ["workflow", "ownerDepartment", "id"], "string", true),
        leaf("ownerDepartmentName", "Owner department name", ["workflow", "ownerDepartment", "name"], "string", true),
      ]),
      group("trigger", "Trigger", [
        leaf("triggerType", "Trigger type", ["trigger", "type"], "string"),
        leaf("triggerSourceId", "Trigger source ID", ["trigger", "sourceId"], "string", true),
        leaf("triggerName", "Trigger name", ["trigger", "name"], "string", true),
      ]),
      group("initiator", "Initiator", [
        leaf("initiatorType", "Initiator type", ["initiator", "type"], "string"),
        leaf("userId", "User ID", ["initiator", "user", "id"], "string", true),
        leaf("userName", "User name", ["initiator", "user", "name"], "string", true),
        leaf("departmentId", "Department ID", ["initiator", "department", "id"], "string", true),
        leaf("departmentName", "Department name", ["initiator", "department", "name"], "string", true),
        leaf("roleIds", "Role IDs", ["initiator", "roles", "ids"], "array", true),
        leaf("roleCodes", "Role codes", ["initiator", "roles", "codes"], "array", true),
        leaf("roleNames", "Role names", ["initiator", "roles", "names"], "array", true),
        leaf("roleAssignments", "Role assignments", ["initiator", "roles", "assignments"], "array", true),
      ]),
      group("applicationSession", "Application and session", [
        leaf("applicationId", "Application ID", ["application", "id"], "string", true),
        leaf("invocationId", "Invocation ID", ["invocation", "id"], "string", true),
        leaf("sessionId", "Session ID", ["session", "id"], "string", true),
        leaf("externalUserId", "External user ID", ["session", "externalUserId"], "string", true),
      ]),
    ],
  }];
}

function recommendAiText(entries: ReferenceEntry[], nodeType: string): ReferenceEntry[] {
  if (nodeType !== "model" && nodeType !== "agent") return entries;
  return entries.map((entry) => entry.label === "text" ? { ...entry, recommended: true } : entry);
}

function reachablePredecessors(
  document: Pick<StudioDocument, "nodes" | "edges">,
  targetNodeId: string,
  targetPort?: "main" | "error",
) {
  const incoming = new Map<string, string[]>();
  for (const edge of document.edges)
    if (edge.data?.edgeKind === "execution")
      incoming.set(edge.target, [
        ...(incoming.get(edge.target) ?? []),
        edge.source,
      ]);
  const virtualLoopId = isIterationEndId(targetNodeId) ? loopIdOfIterationEnd(targetNodeId) : undefined;
  const bodyIds = virtualLoopId
    ? new Set(document.nodes.flatMap((node) => node.data.editorKind === "action" && node.data.parentId === virtualLoopId ? [node.id] : []))
    : undefined;
  const result = new Set<string>();
  const pending = bodyIds
    ? [...bodyIds].filter((nodeId) => !document.edges.some((edge) => edge.data?.edgeKind === "execution" && edge.source === nodeId && bodyIds.has(edge.target)))
    : targetPort
    ? document.edges
        .filter((edge) => edge.data?.edgeKind === "execution" && edge.target === targetNodeId && edge.targetHandle === targetPort)
        .map((edge) => edge.source)
    : [...(incoming.get(targetNodeId) ?? [])];
  while (pending.length) {
    const id = pending.pop()!;
    if (bodyIds && !bodyIds.has(id)) continue;
    if (result.has(id)) continue;
    result.add(id);
    pending.push(...(incoming.get(id) ?? []));
  }
  return result;
}

function selector(
  name: "current" | "first" | "last",
  portPath: string,
  fields: ReferenceEntry[],
  cardinality: ReferenceEntry["cardinality"],
  sourceNodeId: string,
  port: string,
  schema: JsonSchemaProperty,
): ReferenceEntry {
  const path = `${portPath}.${name}`;
  return {
    id: path,
    label: name,
    path,
    selector: outputSelector(sourceNodeId, port, { kind: name }),
    type: "object",
    schema,
    cardinality,
    nullable: cardinality === "zero_or_one" || cardinality === "zero_or_many",
    children: fields.map((field) =>
      rebase(field, `${portPath}.current`, path, { kind: name }),
    ),
  };
}

function rebase(
  entry: ReferenceEntry,
  from: string,
  to: string,
  item: ValueSelector["item"],
): ReferenceEntry {
  const path = entry.path.replace(from, to);
  return {
    ...entry,
    id: path,
    path,
    selector: entry.selector ? { ...entry.selector, item } : undefined,
    children: entry.children.map((child) => rebase(child, from, to, item)),
  };
}

function schemaEntries(
  schema: JsonSchemaProperty,
  parentPath: string,
  idPrefix: string,
  sensitive = false,
  parentSelector?: ValueSelector,
): ReferenceEntry[] {
  return Object.entries(schema.properties ?? {}).map(([name, child]) => {
    const path = appendSegment(parentPath, name);
    const id = appendSegment(idPrefix, name);
    const selector = parentSelector
      ? { ...parentSelector, path: [...(parentSelector.path ?? []), name] }
      : undefined;
    const children = schemaEntries(child, path, id, sensitive, selector);
    return {
      id,
      label: name,
      path,
      selector,
      type: primaryType(child),
      schema: child,
      nullable: !(schema.required ?? []).includes(name),
      sensitive,
      children,
    };
  });
}

function outputEntries(
  nodeType: string,
  schema: JsonSchemaProperty,
  parentPath: string,
  idPrefix: string,
  sensitive: boolean,
  parentSelector?: ValueSelector,
): ReferenceEntry[] {
  if (nodeType !== "code" || !schema.properties?.structuredOutput) {
    return schemaEntries(schema, parentPath, idPrefix, sensitive, parentSelector);
  }
  const structured = schema.properties.structuredOutput;
  const selector = parentSelector ? { ...parentSelector, path: [...(parentSelector.path ?? []), "structuredOutput"] } : undefined;
  const business = schemaEntries(structured, parentPath, idPrefix, sensitive, selector);
  const full: ReferenceEntry = {
    id: `${idPrefix}.completeResult`,
    label: "完整结果",
    path: `${parentPath}.structuredOutput`,
    selector,
    type: "object",
    schema: structured,
    nullable: false,
    children: [],
  };
  const diagnosticsSchema: JsonSchemaProperty = {
    type: "object",
    properties: Object.fromEntries(Object.entries(schema.properties).filter(([name]) => name !== "structuredOutput")),
  };
  const diagnostics = schemaEntries(diagnosticsSchema, `${parentPath}.diagnostics`, `${idPrefix}.diagnostics`, sensitive, parentSelector);
  return [...business, full, { id: `${idPrefix}.diagnostics`, label: "诊断", path: `${parentPath}.diagnostics`, children: diagnostics }];
}

function primaryType(schema: JsonSchemaProperty): string | undefined {
  return Array.isArray(schema.type) ? schema.type.find((type) => type !== "null") ?? schema.type[0] : schema.type;
}

function effectiveNodeOutputSchema(
  document: Pick<StudioDocument, "start" | "nodes" | "edges">,
  nodeId: string,
  portName: string,
  manifests: Map<string, NodeManifest>,
  localize: Localize,
  visiting: Set<string>,
): JsonSchemaProperty {
  if (visiting.has(nodeId)) return {};
  const nextVisiting = new Set(visiting).add(nodeId);
  const node = document.nodes.find((candidate) => candidate.id === nodeId);
  if (!node || node.data.editorKind !== "action") return {};
  const data = node.data;
  const manifest = resolveNodeManifest(manifests, nodeId, data.nodeType, data.typeVersion, data.parameters);
  if (!manifest) return {};
  const port = instanceOutputPorts(manifest, data.parameters, localize).find((candidate) => candidate.name === portName);
  if (!port) return {};
  const declared = asSchema(manifest.outputPortSchemas?.[port.schemaName] ?? manifest.outputSchema);
  const structured = port.schemaName === "main"
    ? data.nodeType === "model" && data.parameters.responseMode === "json_schema"
      ? data.parameters.structuredSchema
      : data.nodeType === "code" && data.parameters.outputExample && typeof data.parameters.outputExample === "object" && !Array.isArray(data.parameters.outputExample)
        ? inferSchema(data.parameters.outputExample)
        : undefined
    : undefined;
  let schema = dynamicPortSchema(
    structured && typeof structured === "object" && !Array.isArray(structured)
      ? { ...declared, properties: { ...declared.properties, structuredOutput: asSchema(structured) } }
      : declared,
    port,
  );
  if (port.schemaName === "main" && data.nodeType === "set") {
    schema = data.parameters.keepOnlySet === true
      ? { type: "object", properties: {}, required: [], additionalProperties: false }
      : incomingItemSchema(document, nodeId, manifests, localize, nextVisiting);
    const values = data.parameters.values && typeof data.parameters.values === "object" && !Array.isArray(data.parameters.values)
      ? data.parameters.values as Record<string, unknown>
      : {};
    const properties = { ...(schema.properties ?? {}) };
    const required = new Set(schema.required ?? []);
    for (const [name, value] of Object.entries(values)) {
      properties[name] = bindingSchema(document, value, manifests, localize, nextVisiting);
      required.add(name);
    }
    schema = { ...schema, type: "object", properties, required: [...required] };
  }
  if (port.schemaName === "main" && data.nodeType === "list") {
    const input = bindingSchema(document, data.parameters.input, manifests, localize, nextVisiting);
    schema = { type: "object", properties: { items: input }, required: ["items"], additionalProperties: false };
  }
  if (port.schemaName === "main" && data.nodeType === "loop_over_items") {
    const item = bindingSchema(document, data.parameters.outputSelector, manifests, localize, nextVisiting);
    schema = { type: "object", properties: { items: { type: "array", items: item } }, required: ["items"], additionalProperties: false };
  }
  return schema;
}

function incomingItemSchema(
  document: Pick<StudioDocument, "start" | "nodes" | "edges">,
  targetNodeId: string,
  manifests: Map<string, NodeManifest>,
  localize: Localize,
  visiting: Set<string>,
): JsonSchemaProperty {
  const incoming = document.edges.find((edge) => edge.data?.edgeKind === "execution" && edge.target === targetNodeId);
  if (!incoming) return { type: "object", properties: {} };
  if (incoming.source === "__start__") return asSchema(document.start.inputs);
  return effectiveNodeOutputSchema(document, incoming.source, incoming.sourceHandle ?? "main", manifests, localize, visiting);
}

function bindingSchema(
  document: Pick<StudioDocument, "start" | "nodes" | "edges">,
  value: unknown,
  manifests: Map<string, NodeManifest>,
  localize: Localize,
  visiting: Set<string>,
): JsonSchemaProperty {
  if (value && typeof value === "object" && !Array.isArray(value)) {
    const binding = value as { kind?: unknown; value?: unknown; selector?: ValueSelector; items?: unknown[]; fields?: Record<string, unknown> };
    if (binding.kind === "literal") return inferSchema(binding.value);
    if (binding.kind === "reference" && binding.selector) return selectorSchema(document, binding.selector, manifests, localize, visiting);
    if (binding.kind === "template") return { type: "string" };
    if (binding.kind === "array") {
      const schemas = (binding.items ?? []).map((item) => bindingSchema(document, item, manifests, localize, visiting));
      const first = schemas[0];
      return { type: "array", items: first && schemas.every((schema) => JSON.stringify(schema) === JSON.stringify(first)) ? first : {} };
    }
    if (binding.kind === "object") return {
      type: "object",
      properties: Object.fromEntries(Object.entries(binding.fields ?? {}).map(([name, child]) => [name, bindingSchema(document, child, manifests, localize, visiting)])),
      required: Object.keys(binding.fields ?? {}),
      additionalProperties: false,
    };
  }
  return inferSchema(value);
}

function selectorSchema(
  document: Pick<StudioDocument, "start" | "nodes" | "edges">,
  selectorValue: ValueSelector,
  manifests: Map<string, NodeManifest>,
  localize: Localize,
  visiting: Set<string>,
): JsonSchemaProperty {
  const selector = selectorWithDefaults(selectorValue);
  let schema = selector.namespace === "inputs"
    ? asSchema(document.start.inputs)
    : selector.namespace === "contexts"
      ? asSchema(document.start.contexts[String(selector.path[0])]?.schema)
      : selector.namespace === "outputs" && selector.sourceNodeId
        ? effectiveNodeOutputSchema(document, selector.sourceNodeId, selector.port ?? "main", manifests, localize, visiting)
        : {};
  const path = selector.namespace === "contexts" ? selector.path.slice(1) : selector.path;
  for (const segment of path) {
    schema = typeof segment === "number" ? asSchema(schema.items) : asSchema(schema.properties?.[segment]);
  }
  return selector.item.kind === "all" ? { type: "array", items: schema } : schema;
}

function inferSchema(value: unknown): JsonSchemaProperty {
  if (value === null) return { type: "null" };
  if (Array.isArray(value)) return { type: "array", items: value.length ? inferSchema(value[0]) : {} };
  if (typeof value === "object") {
    const entries = Object.entries(value as Record<string, unknown>);
    return { type: "object", properties: Object.fromEntries(entries.map(([name, child]) => [name, inferSchema(child)])), required: entries.map(([name]) => name), additionalProperties: false };
  }
  if (typeof value === "number") return { type: Number.isInteger(value) ? "integer" : "number" };
  return { type: typeof value };
}

function resolveNodeManifest(
  manifests: Map<string, NodeManifest>,
  nodeId: string,
  nodeType: string,
  version: number,
  parameters: Record<string, unknown>,
) {
  const exact = manifests.get(`node:${nodeId}`);
  if (exact) return exact;
  if (nodeType === "sub_workflow" && typeof parameters.workflowVersionId === "string") {
    const derived = `workflow.${parameters.workflowVersionId.replaceAll("-", "")}@${version}`;
    const manifest = manifests.get(derived);
    if (manifest) return manifest;
  }
  return manifests.get(`${nodeType}@${version}`);
}

type InstanceOutputPort = {
  name: string;
  label: string;
  schemaName: string;
  decisionId?: string;
};

function instanceOutputPorts(
  manifest: NodeManifest,
  parameters: Record<string, unknown>,
  localize: Localize,
): InstanceOutputPort[] {
  return manifest.outputPorts.flatMap((port) => {
    if (port.variadic && port.name === "case") {
      const cases = Array.isArray(parameters.cases) ? parameters.cases : [];
      return cases.flatMap((value, index) => {
        if (!value || typeof value !== "object" || Array.isArray(value)) return [];
        const branch = value as { id?: unknown; name?: unknown };
        if (typeof branch.id !== "string" || !branch.id) return [];
        const label = typeof branch.name === "string" && branch.name.trim()
          ? branch.name.trim()
          : localize("studio.conditionBuilder.branch", `Branch ${index + 1}`);
        return [{ name: `case:${branch.id}`, label, schemaName: "case" }];
      });
    }
    if (port.variadic && port.name === "decision") {
      const configured = Array.isArray(parameters.buttons) ? parameters.buttons : [];
      const buttons = configured.length ? configured : [
        { id: "approved", label: localize("studio.card.approveDefault", "Approve") },
        { id: "rejected", label: localize("studio.card.rejectDefault", "Reject") },
      ];
      return buttons.flatMap((value) => {
        if (!value || typeof value !== "object" || Array.isArray(value)) return [];
        const button = value as { id?: unknown; label?: unknown };
        if (typeof button.id !== "string" || !button.id) return [];
        return [{
          name: `decision:${button.id}`,
          label: typeof button.label === "string" && button.label.trim() ? button.label.trim() : button.id,
          schemaName: "decision",
          decisionId: button.id,
        }];
      });
    }
    return [{ name: port.name, label: port.name, schemaName: port.name }];
  });
}

function dynamicPortSchema(
  schema: JsonSchemaProperty,
  port: InstanceOutputPort,
): JsonSchemaProperty {
  if (!port.decisionId || !schema.properties?.decision) return schema;
  return {
    ...schema,
    properties: {
      ...schema.properties,
      decision: { ...schema.properties.decision, enum: [port.decisionId] },
    },
  };
}

function asSchema(value: unknown): JsonSchemaProperty {
  return value && typeof value === "object" && !Array.isArray(value)
    ? (value as JsonSchemaProperty)
    : {};
}

function baseSelector(
  namespace: ValueSelector["namespace"],
  path: Array<string | number> = [],
): ValueSelector {
  return {
    namespace,
    run: { kind: "current" },
    item: { kind: "current" },
    path,
  };
}

function outputSelector(
  sourceNodeId: string,
  port: string,
  item: ValueSelector["item"],
  run: ValueSelector["run"] = { kind: "current" },
): ValueSelector {
  return {
    namespace: "outputs",
    sourceNodeId,
    port,
    run,
    item,
    path: [],
  };
}

const CEL_RESERVED = new Set([
  "as",
  "break",
  "const",
  "continue",
  "else",
  "false",
  "for",
  "function",
  "if",
  "import",
  "in",
  "let",
  "loop",
  "namespace",
  "null",
  "package",
  "return",
  "true",
  "var",
  "void",
  "while",
]);

function appendSegment(parent: string, segment: string) {
  return /^[A-Za-z_][A-Za-z0-9_]*$/.test(segment) && !CEL_RESERVED.has(segment)
    ? `${parent}.${segment}`
    : `${parent}[${JSON.stringify(segment)}]`;
}
