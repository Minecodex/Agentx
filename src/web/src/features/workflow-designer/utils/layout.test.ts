import { describe, expect, it } from 'vitest'

import type { StudioEdge, StudioNode } from '../model/types'
import { createEditorStore } from '../store/editor-store'
import { serializeStudio } from '../model/serializer'
import { autoLayout, largeGraphLayout, layoutStudioDocument, LAYOUT_LAYER_GAP, LAYOUT_NODE_GAP, LARGE_GRAPH_LAYOUT_THRESHOLD, LOOP_CONTAINER_MIN_HEIGHT, LOOP_CONTAINER_MIN_WIDTH, LOOP_CONTAINER_PADDING } from './layout'

describe('large Workflow layout baseline', () => {
  it('lays out 600 nodes deterministically within the degradation budget', async () => {
    expect(LARGE_GRAPH_LAYOUT_THRESHOLD).toBeLessThan(600)
    const nodes = Array.from({ length: 600 }, (_, index) => actionNode(`node-${index}`))
    const edges = Array.from({ length: 599 }, (_, index): StudioEdge => ({ id: `edge-${index}`, source: `node-${index}`, target: `node-${index + 1}`, type: 'studio', data: { edgeKind: 'execution', order: 0 } }))
    const started = performance.now()
    const first = await autoLayout(nodes, edges)
    const elapsed = performance.now() - started
    const second = await autoLayout(nodes, edges)

    expect(elapsed).toBeLessThan(1_500)
    expect(first).toHaveLength(600)
    expect(first.map((node) => node.position)).toEqual(second.map((node) => node.position))
    expect(new Set(first.map((node) => `${node.position.x}:${node.position.y}`)).size).toBe(600)
  })
})

function actionNode(id: string): StudioNode { return { id, type: 'manifest', position: { x: 0, y: 0 }, data: { editorKind: 'action', nodeType: 'set', typeVersion: 1, label: id, key: id, parameters: {}, contextWrites: [], resourceReferences: [], settings: {}, disabled: false } } }

describe('container-aware auto layout', () => {
  it('lays out Start, branches and Exit with readable gaps without changing business data', async () => {
    const state = createEditorStore().getState()
    const draft = { ...state, nodes: [...state.nodes], edges: [...state.edges] }
    draft.nodes.push({ id: 'exit', type: 'exit', position: { x: 0, y: 0 }, data: { editorKind: 'exit', key: 'exit', label: 'End', protected: true, parameters: { outputs: {}, errorOutputs: {} } } }, actionNode('left'), actionNode('right'))
    draft.edges = [edgeOf('start-left', '__start__', 'left'), edgeOf('start-right', '__start__', 'right'), edgeOf('left-exit', 'left', draft.nodes[0].id), edgeOf('right-exit', 'right', draft.nodes[0].id)]
    const before = JSON.stringify(serializeStudio(draft))
    const result = await layoutStudioDocument(draft)
    expect(JSON.stringify(serializeStudio(draft))).toBe(before)
    expect(serializeStudio(result).definition).toEqual(JSON.parse(before).definition)
    const start = result.boundaryLayouts[0]
    for (const node of result.nodes.filter(node => node.id === 'left' || node.id === 'right')) {
      expect(node.position.x - start.x - 240).toBeGreaterThanOrEqual(LAYOUT_LAYER_GAP)
    }
    const [left, right] = ['left', 'right'].map(id => result.nodes.find(node => node.id === id)!)
    expect(Math.abs(left.position.y - right.position.y)).toBeGreaterThanOrEqual(LAYOUT_NODE_GAP + 62 - 0.001)
    expect(await layoutStudioDocument(draft)).toEqual(result)
  })

  it('uses actual widths and cumulative heights in large graphs without mutating input', () => {
    const nodes = [{ ...actionNode('wide'), width: 700, height: 400 }, { ...actionNode('short'), height: 62 }, actionNode('after')]
    const before = JSON.stringify(nodes)
    const result = largeGraphLayout(nodes, [edgeOf('edge', 'wide', 'after')])
    const [wide, short, after] = result
    expect(after.position.x - wide.position.x - 700).toBeGreaterThanOrEqual(LAYOUT_LAYER_GAP)
    expect(short.position.y - wide.position.y - 400).toBeGreaterThanOrEqual(LAYOUT_NODE_GAP)
    expect(JSON.stringify(nodes)).toBe(before)
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

    const layouted = await autoLayout(nodes, edges)
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
