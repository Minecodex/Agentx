import { describe, expect, it } from 'vitest'

import type { WorkflowDraft } from '../../../shared/api/types'
import { iterationChipId } from '../utils/connections'
import type { StudioDocument, StudioNode, WorkflowDefinition } from './types'
import { deserializeDraft, serializeStudio } from './serializer'

const LOOP = 'loop-1'
const CHIP = iterationChipId(LOOP)

function loopNode(): StudioNode {
  return {
    id: LOOP,
    type: 'manifest',
    position: { x: 600, y: 120 },
    width: 470,
    height: 300,
    data: { editorKind: 'action', nodeType: 'loop_over_items', typeVersion: 1, label: LOOP, key: LOOP, parameters: { parallelism: 4 }, contextWrites: [], resourceReferences: [], settings: {}, disabled: false },
  }
}

function actionNode(id: string, nodeType: string, parentId?: string): StudioNode {
  return {
    id,
    type: 'manifest',
    position: { x: 0, y: 0 },
    data: { editorKind: 'action', nodeType, typeVersion: 1, label: id, key: id, parentId, parameters: {}, contextWrites: [], resourceReferences: [], settings: {}, disabled: false },
  }
}

function documentWithChip(): StudioDocument {
  return {
    start: { inputs: { type: 'object', properties: {}, additionalProperties: false }, contexts: {} },
    nodes: [
      loopNode(),
      { ...actionNode('body', 'set', LOOP), position: { x: 40, y: 130 } },
      actionNode('outside', 'set'),
      // Edit-only chip that must never reach the Definition (defensive: chips live in the view layer only).
      { id: CHIP, type: 'iteration-chip', position: { x: 16, y: 56 }, data: { editorKind: 'iteration-chip', loopId: LOOP } } as unknown as StudioNode,
    ],
    edges: [
      { id: 'fanout', source: LOOP, sourceHandle: 'main', target: 'body', targetHandle: 'main', type: 'studio', data: { edgeKind: 'execution', order: 0 } },
      { id: 'after', source: 'outside', sourceHandle: 'main', target: LOOP, targetHandle: 'main', type: 'studio', data: { edgeKind: 'execution', order: 0 } },
      { id: 'chip-edge', source: CHIP, sourceHandle: 'main', target: 'body', targetHandle: 'main', type: 'studio', data: { edgeKind: 'execution', order: 1 } },
    ],
    end: { completion: 'first_return', outputs: {}, error: { outputs: {} } },
    viewport: { x: 0, y: 0, zoom: 1 },
    boundaryLayouts: [{ boundary: 'start', x: 40, y: 220 }],
    annotations: [],
    groups: [],
    settings: { executionOrder: 'deterministic', activationBudget: 10_000 },
  }
}

describe('studio serializer container roundtrip', () => {
  it('omits null query versions so manual multi-source selection has the canonical server hash', () => {
    const document = documentWithChip()
    const node = document.nodes[0]
    if (node.data.editorKind !== 'action') throw new Error('Expected action')
    node.data.resourceReferences = [
      { resourceType: 'rag', resourceId: 'query-a', operation: 'read' },
      { resourceType: 'rag', resourceId: 'query-b', operation: 'read', resourceVersionId: null },
    ]
    const saved = serializeStudio(document)
    expect(saved.definition.nodes[0].resourceReferences).toEqual([
      { resourceType: 'rag', resourceId: 'query-a', operation: 'read' },
      { resourceType: 'rag', resourceId: 'query-b', operation: 'read' },
    ])
    expect(JSON.stringify(serializeStudio(deserializeDraft(saved as unknown as WorkflowDraft)))).toBe(JSON.stringify(saved))
  })

  it('drops iteration chip nodes and their edges while keeping real connections untouched', () => {
    const { definition } = serializeStudio(documentWithChip())

    expect(definition.nodes.map((node) => node.id)).toEqual([LOOP, 'body', 'outside'])
    expect(definition.connections).toHaveLength(2)
    expect(definition.connections.map((connection) => connection.id)).toEqual(['fanout', 'after'])
    // Loop→body edges stay on the real loop node: the compiler reads them as iteration fan-out.
    expect(definition.connections.find((connection) => connection.id === 'fanout')).toMatchObject({ sourceNodeId: LOOP, targetNodeId: 'body' })
  })

  it('roundtrips parentId through serialize and deserialize', () => {
    const { definition, editorDocument } = serializeStudio(documentWithChip())
    expect(definition.nodes.find((node) => node.id === 'body')?.parentId).toBe(LOOP)
    expect(editorDocument.nodeLayouts.find((layout) => layout.nodeId === 'body')).toMatchObject({ x: 40, y: 130 })

    const draft = { definition, editorDocument } as unknown as WorkflowDraft
    const restored = deserializeDraft(draft)
    const body = restored.nodes.find((node) => node.id === 'body')
    expect(body?.data.editorKind === 'action' && body.data.parentId).toBe(LOOP)
    expect(restored.nodes.some((node) => node.id === CHIP)).toBe(false)
    expect(restored.edges.find((edge) => edge.id === 'fanout')?.source).toBe(LOOP)
  })

  it('serializes the persisted loop frame before stale measured dimensions', () => {
    const loop = { ...actionNode(LOOP, 'loop_over_items'), width: 720, height: 480, measured: { width: 470, height: 300 } }
    const source = documentWithChip()
    source.nodes = [loop]
    source.edges = []
    const serialized = serializeStudio(source)
    expect(serialized.editorDocument.nodeLayouts[0]).toMatchObject({ width: 720, height: 480 })
  })

  it('roundtrips a manually resized Loop frame across repeated saves', () => {
    const source = documentWithChip()
    source.nodes[0] = { ...source.nodes[0], width: undefined, height: undefined, measured: { width: 760, height: 520 } }
    const first = serializeStudio(source)
    expect(first.editorDocument.nodeLayouts.find((layout) => layout.nodeId === LOOP)).toMatchObject({ width: 760, height: 520 })

    const restored = deserializeDraft(first as unknown as WorkflowDraft)
    expect(restored.nodes.find((node) => node.id === LOOP)).toMatchObject({ width: 760, height: 520 })
    const second = serializeStudio(restored)
    expect(second.editorDocument.nodeLayouts.find((layout) => layout.nodeId === LOOP)).toMatchObject({ width: 760, height: 520 })
  })

  it('deserializes parentId from a definition produced outside the editor', () => {
    const definition = {
      schemaVersion: '8.0',
      start: { inputs: {}, contexts: {} },
      nodes: [
        { id: LOOP, key: 'loop', type: 'loop_over_items', typeVersion: 1, name: 'Loop', disabled: false, protected: false, parentId: undefined, parameters: {}, contextWrites: [], resourceReferences: [], settings: {} },
        { id: 'body', key: 'body', type: 'set', typeVersion: 1, name: 'Body', disabled: false, protected: false, parentId: LOOP, parameters: {}, contextWrites: [], resourceReferences: [], settings: {} },
      ],
      connections: [{ id: 'fanout', sourceNodeId: LOOP, sourceHandle: 'main', targetNodeId: 'body', targetHandle: 'main', order: 0 }],
      end: { completion: 'first_return', outputs: {}, error: { outputs: {} } },
      settings: { executionOrder: 'deterministic', activationBudget: 10_000 },
    } as unknown as WorkflowDefinition
    const restored = deserializeDraft({ definition, editorDocument: { nodeLayouts: [], boundaryLayouts: [], edges: [], annotations: [], groups: [], viewport: { x: 0, y: 0, zoom: 1 } } } as unknown as WorkflowDraft)
    expect(restored.nodes.find((node) => node.id === 'body')?.data).toMatchObject({ parentId: LOOP })
  })
})
