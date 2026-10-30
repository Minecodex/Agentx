import ELK from 'elkjs/lib/elk.bundled.js'
import { describe, expect, it } from 'vitest'

import type { NodeManifest, StudioDocument, StudioEdge, StudioNode } from '../model/types'
import { createEditorStore } from '../store/editor-store'
import { serializeStudio } from '../model/serializer'
import { autoLayout as autoLayoutCore, layoutStudioDocumentCore } from './layout-core'
import { LAYOUT_LAYER_GAP, LAYOUT_NODE_GAP, LOOP_CONTAINER_MIN_HEIGHT, LOOP_CONTAINER_MIN_WIDTH, LOOP_CONTAINER_PADDING } from './layout-constants'
import { studioManifest, studioManifestTypes } from '../testing/studio-catalog'
import { computeGroupView } from './group-geometry'
const elk = new ELK()
const autoLayout = (nodes: StudioNode[], edges: StudioEdge[], manifests?: Map<string, NodeManifest>) => autoLayoutCore(nodes, edges, manifests, elk)
const layoutStudioDocument = (document: StudioDocument, manifests?: Map<string, NodeManifest>) => layoutStudioDocumentCore(document, manifests, elk)
const manifests = new Map(studioManifestTypes.map(type => [`${type}@${studioManifest(type).version}`, studioManifest(type)]))

describe('worker layout core baseline', () => {
  it('lays out 600 nodes deterministically within the layout budget', async () => {
    const nodes = Array.from({ length: 600 }, (_, index) => actionNode(`node-${index}`))
    const edges = Array.from({ length: 599 }, (_, index): StudioEdge => ({ id: `edge-${index}`, source: `node-${index}`, target: `node-${index + 1}`, type: 'studio', data: { edgeKind: 'execution', order: 0 } }))
    const started = performance.now()
    const first = await autoLayout(nodes, edges, manifests)
    const elapsed = performance.now() - started
    console.info(JSON.stringify({ benchmark: 'ELK fixed ports', nodes: 600, elapsedMs: Math.round(elapsed) }))
    const second = await autoLayout(nodes, edges, manifests)

    expect(elapsed).toBeLessThan(1_500)
    expect(first).toHaveLength(600)
    expect(first.map((node) => node.position)).toEqual(second.map((node) => node.position))
    expect(new Set(first.map((node) => `${node.position.x}:${node.position.y}`)).size).toBe(600)
  })
})

function actionNode(id: string): StudioNode { return { id, type: 'manifest', position: { x: 0, y: 0 }, data: { editorKind: 'action', nodeType: 'set', typeVersion: 1, label: id, key: id, parameters: {}, contextWrites: [], resourceReferences: [], settings: {}, disabled: false } } }

describe('container-aware auto layout', () => {
  it('arranges collapsed proxies using their ports while translating hidden members together', async () => {
    const state = createEditorStore().getState()
    const a = { ...actionNode('a'), position: { x: 800, y: 500 } }
    const b = { ...actionNode('b'), position: { x: 1600, y: 650 } }
    const c = actionNode('c')
    const draft = { ...state, nodes: [a, b, c], edges: [edgeOf('start', '__start__', 'a'), edgeOf('internal', 'a', 'b'), edgeOf('out', 'b', 'c')], groups: [{ id: 'g', label: 'Group', nodeIds: ['a', 'b'], collapsed: true, color: 'blue' }] }
    const result = await layoutStudioDocument(draft, manifests)
    const view = computeGroupView(result.groups[0], new Map(result.nodes.map(node => [node.id, node])), manifests)!
    const at = (id: string) => result.nodes.find(node => node.id === id)!.position
    expect(view.width).toBe(240)
    expect(at('c').x - view.position.x - view.width).toBeCloseTo(LAYOUT_LAYER_GAP)
    expect(at('c').y).toBeCloseTo(view.position.y)
    expect(at('b').x - at('a').x).toBe(b.position.x - a.position.x)
    expect(at('b').y - at('a').y).toBe(b.position.y - a.position.y)
    expect(serializeStudio(result).definition).toEqual(serializeStudio(draft).definition)
  })

  it('expands a compact loop with its hidden group and undoes container growth as one transaction', async () => {
    const store = createEditorStore()
    const state = store.getState()
    const a = { ...actionNode('a'), position: { x: 304, y: 84 }, data: { ...actionNode('a').data, parentId: 'loop' } } as StudioNode
    const b = { ...actionNode('b'), position: { x: 640, y: 84 }, data: { ...actionNode('b').data, parentId: 'loop' } } as StudioNode
    const document = await layoutStudioDocument({ ...state, nodes: [{ ...loopNode('loop'), width: 1600, height: 400 }, a, b], edges: [edgeOf('inner', 'a', 'b')], groups: [{ id: 'g', label: 'Group', nodeIds: ['a', 'b'], collapsed: true }] }, manifests)
    store.getState().hydrate(document)
    const width = () => store.getState().nodes.find(node => node.id === 'loop')!.width!
    const compactWidth = width()
    const positions = () => store.getState().nodes.filter(node => node.id !== 'loop').map(node => node.position)
    const before = positions()
    store.getState().toggleGroup('g')
    expect(width()).toBeGreaterThan(compactWidth)
    expect(positions()).toEqual(before)
    store.getState().undo()
    expect(width()).toBe(compactWidth)
    expect(store.getState().groups[0].collapsed).toBe(true)
    store.getState().redo()
    const expandedWidth = width()
    store.getState().beginEdit({ nodeIds: ['a', 'b'] })
    store.getState().moveGroup('g', { x: 300, y: 0 })
    store.getState().commitEdit()
    expect(width()).toBeGreaterThan(expandedWidth)
    store.getState().undo()
    expect(width()).toBe(expandedWidth)
    expect(positions()).toEqual(before)
    expect(serializeStudio(store.getState()).definition).toEqual(serializeStudio(document).definition)
  })
  it('preserves condition row order even when nodes and edges were created in a different order', async () => {
    const branch = { ...actionNode('branch'), data: { ...actionNode('branch').data, nodeType: 'if', parameters: { cases: [{ id: 'high' }, { id: 'low' }] } } } as StudioNode
    const nodes = [branch, actionNode('fallback'), actionNode('low'), actionNode('high')]
    const edges = [{ ...edgeOf('low-edge', 'branch', 'low'), sourceHandle: 'case:low' }, { ...edgeOf('else-edge', 'branch', 'fallback'), sourceHandle: 'else' }, { ...edgeOf('high-edge', 'branch', 'high'), sourceHandle: 'case:high' }]
    const result = await autoLayout(nodes, edges, manifests)
    const y = (id: string) => result.find(node => node.id === id)!.position.y
    expect(y('high')).toBeLessThan(y('low'))
    expect(y('low')).toBeLessThan(y('fallback'))
  })

  it('keeps a dedicated error End near its condition while preserving normal branch order', async () => {
    const branch = { ...actionNode('branch'), data: { ...actionNode('branch').data, nodeType: 'if', parameters: { cases: [{ id: 'a' }, { id: 'b' }, { id: 'c' }] } } } as StudioNode
    const errorEnd: StudioNode = { id: 'error-end', type: 'exit', position: { x: 0, y: 0 }, data: { editorKind: 'exit', key: 'error_end', label: 'Error End', protected: false, parameters: { outputs: {}, errorOutputs: {} } } }
    const nodes = [branch, ...['a', 'b', 'c', 'fallback'].map(actionNode), errorEnd]
    const edges = [...['a', 'b', 'c'].map(id => ({ ...edgeOf(id, 'branch', id), sourceHandle: `case:${id}` })), { ...edgeOf('fallback', 'branch', 'fallback'), sourceHandle: 'else' }, { ...edgeOf('error', 'branch', 'error-end'), sourceHandle: 'error', targetHandle: 'error' }]
    const result = await autoLayout(nodes, edges, manifests)
    const at = (id: string) => result.find(node => node.id === id)!.position
    expect(at('a').y).toBeLessThan(at('b').y)
    expect(at('b').y).toBeLessThan(at('c').y)
    expect(at('c').y).toBeLessThan(at('fallback').y)
    expect(at('error-end').x - at('branch').x - 280).toBeCloseTo(LAYOUT_LAYER_GAP)
    expect(Math.abs(at('error-end').y + 52 - at('branch').y - 52)).toBeLessThanOrEqual(280)
  })

  it('lays out Start, branches and Exit with readable gaps without changing business data', async () => {
    const state = createEditorStore().getState()
    const draft = { ...state, nodes: [...state.nodes], edges: [...state.edges] }
    draft.nodes.push({ id: 'exit', type: 'exit', position: { x: 0, y: 0 }, data: { editorKind: 'exit', key: 'exit', label: 'End', protected: true, parameters: { outputs: {}, errorOutputs: {} } } }, actionNode('left'), actionNode('right'))
    draft.edges = [edgeOf('start-left', '__start__', 'left'), edgeOf('start-right', '__start__', 'right'), edgeOf('left-exit', 'left', draft.nodes[0].id), edgeOf('right-exit', 'right', draft.nodes[0].id)]
    const before = JSON.stringify(serializeStudio(draft))
    const result = await layoutStudioDocument(draft, manifests)
    expect(JSON.stringify(serializeStudio(draft))).toBe(before)
    expect(serializeStudio(result).definition).toEqual(JSON.parse(before).definition)
    const start = result.boundaryLayouts[0]
    for (const node of result.nodes.filter(node => node.id === 'left' || node.id === 'right')) {
      expect(node.position.x - start.x - 184).toBeGreaterThanOrEqual(LAYOUT_LAYER_GAP)
    }
    const [left, right] = ['left', 'right'].map(id => result.nodes.find(node => node.id === id)!)
    expect(Math.abs(left.position.y - right.position.y)).toBeGreaterThanOrEqual(LAYOUT_NODE_GAP + 104 - 0.001)
    expect(await layoutStudioDocument(draft, manifests)).toEqual(result)
  })

  it('keeps error-only nodes below normal nodes without moving shared destinations into the error lane', async () => {
    const nodes = ['root', 'normal', 'failure', 'shared'].map(actionNode)
    const edges = [edgeOf('ok', 'root', 'normal'), { ...edgeOf('bad', 'root', 'failure'), sourceHandle: 'error' }, edgeOf('join', 'normal', 'shared'), edgeOf('recover', 'failure', 'shared')]
    const result = await autoLayout(nodes, edges, manifests)
    const at = (id: string) => result.find(node => node.id === id)!.position
    expect(at('failure').y).toBeGreaterThanOrEqual(Math.max(at('normal').y, at('shared').y) + 104 + LAYOUT_NODE_GAP)
    expect(at('shared').x).toBeGreaterThan(at('normal').x)
  })

  it('sizes each loop container around its laid-out body', async () => {
    const loop: StudioNode = loopNode('loop')
    const first: StudioNode = actionNode('body-1')
    const second: StudioNode = actionNode('body-2')
    const outside: StudioNode = actionNode('outside')
    const nodes = [
      { ...loop, position: { x: 600, y: 80 } },
      { ...first, position: { x: 1800, y: 900 }, data: { ...first.data, parentId: 'loop' } },
      { ...second, position: { x: 2200, y: 1800 }, data: { ...second.data, parentId: 'loop' } },
      outside,
    ]
    const edges = [
      edgeOf('fanout', 'loop', 'body-1'),
      edgeOf('chain', 'body-1', 'body-2'),
      edgeOf('after', 'loop', 'outside'),
    ]

    const layouted = await autoLayout(nodes, edges, manifests)
    const byId = new Map(layouted.map((node) => [node.id, node]))
    const bodyFirst = byId.get('body-1')!
    const bodySecond = byId.get('body-2')!
    const container = byId.get('loop')!

    // Children keep container-relative coordinates below the header and chip.
    expect(bodyFirst.position.x).toBeGreaterThanOrEqual(LOOP_CONTAINER_PADDING.left)
    expect(bodyFirst.position.y).toBeGreaterThanOrEqual(LOOP_CONTAINER_PADDING.top)
    expect(bodySecond.position.x).toBeGreaterThan(bodyFirst.position.x)

    // Loose bound: the frame must cover the content plus padding.
    expect(container.width ?? 0).toBeGreaterThanOrEqual(bodySecond.position.x + 240 + LOOP_CONTAINER_PADDING.right)
    expect(container.height ?? 0).toBeGreaterThanOrEqual(bodySecond.position.y + LOOP_CONTAINER_PADDING.bottom)
    expect(container.width ?? 0).toBeGreaterThanOrEqual(LOOP_CONTAINER_MIN_WIDTH)
    expect(container.height ?? 0).toBeGreaterThanOrEqual(LOOP_CONTAINER_MIN_HEIGHT)

    // Roots are laid out on the outer canvas and stay apart from the body frame.
    const freeNode = byId.get('outside')!
    expect(Math.abs(freeNode.position.x - bodyFirst.position.x)).toBeGreaterThan(50)
    expect(freeNode.position.x - container.position.x - container.width!).toBeCloseTo(LAYOUT_LAYER_GAP)
  })
})

function edgeOf(id: string, source: string, target: string): StudioEdge { return { id, source, target, sourceHandle: 'main', targetHandle: 'main', type: 'studio', data: { edgeKind: 'execution', order: 0 } } }

function loopNode(id: string): StudioNode {
  return { id, type: 'manifest', position: { x: 0, y: 0 }, data: { editorKind: 'action', nodeType: 'loop_over_items', typeVersion: 1, label: id, key: id, parameters: {}, contextWrites: [], resourceReferences: [], settings: {}, disabled: false } }
}
