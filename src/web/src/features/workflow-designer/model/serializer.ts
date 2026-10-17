import type { WorkflowDraft } from '../../../shared/api/types'
import type { EditorDocument, ExitNodeData, StudioDocument, StudioEdge, StudioNode, WorkflowDefinition } from './types'
import { emptyEditorDocument } from './types'
import { isIterationChipId } from '../utils/connections'

export const EXIT_NODE_TYPE = 'exit'

export function deserializeDraft(value: WorkflowDraft): StudioDocument {
  const definition = value.definition as WorkflowDefinition
  if (definition.schemaVersion !== '8.0') throw new Error(`Unsupported Workflow Definition ${String(definition.schemaVersion)}`)
  const editor = normalizeEditor(value.editorDocument)
  const layouts = new Map(editor.nodeLayouts.map((layout) => [layout.nodeId, layout]))
  const nodes: StudioNode[] = definition.nodes.map((item, index) => {
    const layout = layouts.get(item.id)
    if (item.type === EXIT_NODE_TYPE) {
      const data: ExitNodeData = { editorKind: 'exit', key: item.key ?? item.id, label: item.name, protected: item.protected ?? false, parameters: { outputs: (item.parameters as { outputs?: ExitNodeData['parameters']['outputs'] })?.outputs ?? {}, errorOutputs: (item.parameters as { errorOutputs?: ExitNodeData['parameters']['errorOutputs'] })?.errorOutputs ?? {} } }
      return { id: item.id, type: 'exit', editorCollapsed: layout?.collapsed, position: { x: layout?.x ?? 560, y: layout?.y ?? 220 }, width: layout?.width, height: layout?.height, data }
    }
    return {
      id: item.id,
      type: 'manifest',
      editorCollapsed: layout?.collapsed,
      position: { x: layout?.x ?? 120 + index * 260, y: layout?.y ?? 180 },
      width: layout?.width,
      height: layout?.height,
      data: { editorKind: 'action', nodeType: item.type, typeVersion: item.typeVersion, label: item.name, key: item.key ?? item.id, disabled: item.disabled, parameters: item.parameters ?? {}, parentId: item.parentId, contextWrites: item.contextWrites ?? [], resourceReferences: item.resourceReferences ?? [], settings: item.settings ?? {} },
    }
  })
  const edges: StudioEdge[] = definition.connections.map((connection) => ({ id: connection.id, source: connection.sourceNodeId, sourceHandle: connection.sourceHandle, target: connection.targetNodeId, targetHandle: connection.targetHandle, data: { edgeKind: 'execution', order: connection.order, labelPosition: editor.edges.find(edge => edge.edgeId === connection.id)?.labelPosition }, type: 'studio' }))
  return { start: definition.start ?? { inputs: {}, contexts: {} }, nodes, edges, end: normalizeEnd(definition.end), viewport: editor.viewport, boundaryLayouts: editor.boundaryLayouts, annotations: editor.annotations, groups: editor.groups, settings: definition.settings ?? { executionOrder: 'deterministic', activationBudget: 10_000 } }
}

export function serializeStudio(document: StudioDocument): { definition: WorkflowDefinition; editorDocument: EditorDocument } {
  // The iteration start chip is edit-only: drop chip nodes and their edges, keep everything else untouched.
  const nodes = document.nodes.filter((node) => !isIterationChipId(node.id))
  const edges = document.edges.filter((edge) => !isIterationChipId(edge.source) && !isIterationChipId(edge.target))
  const edgeById = new Map(edges.map(edge => [edge.id, edge]))
  const connections = edges.map((edge) => ({ id: edge.id, sourceNodeId: edge.source, sourceHandle: edge.sourceHandle ?? 'main', targetNodeId: edge.target, targetHandle: edge.targetHandle ?? 'main', order: edge.data?.order ?? 0 }))
  const grouped = new Map<string, typeof connections>()
  for (const connection of connections) { const key = `${connection.sourceNodeId}\u0000${connection.sourceHandle}`; const values = grouped.get(key) ?? []; values.push(connection); grouped.set(key, values) }
  for (const values of grouped.values()) values.sort((a, b) => a.order - b.order || a.id.localeCompare(b.id)).forEach((value, index) => { value.order = index })
  return {
    definition: {
      schemaVersion: '8.0',
    start: { inputs: document.start.inputs, contexts: Object.fromEntries(Object.entries(document.start.contexts).map(([name, field]) => [name, {...field,title:field.title ?? null,description:field.description ?? null,maxSize:field.maxSize ?? null}])) },
    settings: { ...document.settings, timeoutMs: document.settings.timeoutMs ?? null },
      nodes: nodes.map(node => node.data.editorKind === 'action'
        ? { id: node.id, key: node.data.key, type: node.data.nodeType, typeVersion: node.data.typeVersion, name: node.data.label, disabled: node.data.disabled, parentId: node.data.parentId, protected: false, parameters: node.data.parameters, contextWrites: node.data.contextWrites, resourceReferences: normalizedResourceReferences(node.data.resourceReferences), settings: normalizedNodeSettings(node.data.settings) }
        : { id: node.id, key: node.data.key, type: EXIT_NODE_TYPE, typeVersion: 1, name: node.data.label, disabled: false, protected: node.data.protected, parameters: node.data.parameters as unknown as Record<string, unknown>, contextWrites: [], resourceReferences: [], settings: normalizedNodeSettings({}) }),
      connections,
      end: document.end,
    },
    editorDocument: {
      nodeLayouts: nodes.map((node) => ({ nodeId: node.id, collapsed: node.editorCollapsed ?? false, x: node.position.x, y: node.position.y, width: node.width ?? node.measured?.width, height: node.height ?? node.measured?.height })),
      boundaryLayouts: document.boundaryLayouts,
      edges: connections.map((edge) => ({ edgeId: edge.id, labelPosition: edgeById.get(edge.id)?.data?.labelPosition })),
      annotations: document.annotations,
      groups: document.groups,
      viewport: document.viewport,
    },
  }
}

function normalizedResourceReferences(references: WorkflowDefinition['nodes'][number]['resourceReferences']) {
  // Match the Rust Definition contract: absent optional fields are omitted.
  // Picker options use null for unversioned resources; hashing must not retain it.
  return references.map(({ bindingRole, resourceVersionId, ...reference }) => ({
    ...reference,
    ...(bindingRole == null ? {} : { bindingRole }),
    ...(resourceVersionId == null ? {} : { resourceVersionId }),
  }))
}

function normalizeEditor(value: unknown): EditorDocument {
  const base = emptyEditorDocument()
  if (!value || typeof value !== 'object') return base
  const input = value as Partial<EditorDocument>
  return {
    ...base,
    ...input,
    annotations: (input.annotations ?? []).map((annotation) => ({ width: 240, height: 160, ...annotation })),
    groups: (input.groups ?? []).map((group) => ({ collapsed: false, ...group })),
    viewport: { ...base.viewport, ...(input.viewport ?? {}) },
  }
}

function normalizeEnd(value: WorkflowDefinition["end"] | undefined): WorkflowDefinition["end"] {
  return {
    completion: value?.completion ?? "first_return",
    outputs: value?.outputs ?? {},
    error: { outputs: value?.error?.outputs ?? {} },
  };
}

function normalizedNodeSettings(settings: Record<string, unknown>) {
  return {...settings,executeOnce:settings.executeOnce ?? false,alwaysOutputData:settings.alwaysOutputData ?? false,retryOnFail:settings.retryOnFail ?? false,maxTries:settings.maxTries ?? 1,waitBetweenTriesMs:settings.waitBetweenTriesMs ?? 0,timeoutMs:settings.timeoutMs ?? null}
}
