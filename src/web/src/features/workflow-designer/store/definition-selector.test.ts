import { beforeEach, expect, it } from 'vitest'

import { serializeStudio } from '../model/serializer'
import type { StudioDocument } from '../model/types'
import { createDefinitionSelector } from './definition-selector'
import { createEditorStore } from './editor-store'
const useEditorStore = createEditorStore()

beforeEach(() => {
  const document: StudioDocument = {
    start: { inputs: {}, contexts: {} }, end: { completion: 'first_return', outputs: {}, error: { outputs: {} } },
    nodes: Array.from({ length: 1000 }, (_, index) => ({
      id: `node-${index}`, type: 'manifest', position: { x: index * 10, y: 0 },
      data: { editorKind: 'action', nodeType: 'set', typeVersion: 1, key: `set_${index}`, label: `Set ${index}`,
        parameters: {}, disabled: false, contextWrites: [], resourceReferences: [], settings: {} },
    })),
    edges: [], viewport: { x: 0, y: 0, zoom: 1 }, boundaryLayouts: [], annotations: [], groups: [],
    settings: { executionOrder: 'deterministic', activationBudget: 10000 },
  }
  useEditorStore.getState().hydrate(document)
})

it('keeps definition consumers stable through large-document drag, resize, selection and pan', () => {
  const select = createDefinitionSelector()
  const original = select(useEditorStore.getState())
  useEditorStore.getState().beginEdit()
  for (let frame = 1; frame <= 50; frame += 1) {
    useEditorStore.getState().onNodesChange([{ id: 'node-0', type: 'position', position: { x: frame, y: frame }, dragging: true }])
    expect(select(useEditorStore.getState())).toBe(original)
  }
  useEditorStore.getState().onNodesChange([{ id: 'node-0', type: 'dimensions', dimensions: { width: 100, height: 80 } }])
  useEditorStore.getState().onNodesChange([{ id: 'node-0', type: 'select', selected: true }])
  useEditorStore.getState().setViewport({ x: 50, y: 50, zoom: 0.5 })
  useEditorStore.getState().commitEdit()
  expect(select(useEditorStore.getState())).toBe(original)
  const persisted = serializeStudio(useEditorStore.getState())
  expect(persisted.editorDocument.nodeLayouts[0]).toMatchObject({ x: 50, y: 50 })
  expect(persisted.editorDocument.viewport).toEqual({ x: 50, y: 50, zoom: 0.5 })
})

it('refreshes parameter contracts even when GraphIndex revisions do not change', () => {
  const select = createDefinitionSelector()
  const original = select(useEditorStore.getState())
  const revision = useEditorStore.getState().graphRevision
  useEditorStore.getState().updateNode('node-0', { parameters: { output: 'changed' } })
  expect(useEditorStore.getState().graphRevision).toBe(revision)
  const updated = select(useEditorStore.getState())
  expect(updated).not.toBe(original)
  expect(serializeStudio(updated).definition.nodes[0].parameters).toEqual({ output: 'changed' })
  useEditorStore.getState().undo()
  expect(serializeStudio(select(useEditorStore.getState())).definition.nodes[0].parameters).toEqual({})
})

it('refreshes topology, boundary inputs and execution settings', () => {
  const select = createDefinitionSelector()
  let previous = select(useEditorStore.getState())
  const mutations = [
    () => useEditorStore.getState().connect({ source: 'node-0', target: 'node-1', sourceHandle: 'main', targetHandle: 'main' }, { edgeKind: 'execution', sourcePortKind: 'main' }),
    () => useEditorStore.getState().setStart({ inputs: { type: 'object', properties: { value: { type: 'string' } } }, contexts: {} }),
    () => useEditorStore.getState().setEnd({ completion: 'all_complete', outputs: {}, error: { outputs: {} } }),
    () => useEditorStore.setState({ settings: { executionOrder: 'deterministic', activationBudget: 20000 } }),
    () => useEditorStore.getState().updateNode('node-0', { parentId: 'node-1' }),
  ]
  for (const mutate of mutations) {
    mutate()
    const current = select(useEditorStore.getState())
    expect(current).not.toBe(previous)
    previous = current
  }
})
