import { beforeEach, describe, expect, it } from 'vitest'

import { createEditorStore } from './editor-store'
const useEditorStore = createEditorStore()

describe('draft editing transactions', () => {
  const document = (store: ReturnType<typeof createEditorStore>) => {
    const state = store.getState()
    return {start:state.start,nodes:state.nodes,edges:state.edges,end:state.end,settings:state.settings,viewport:state.viewport,boundaryLayouts:state.boundaryLayouts,annotations:state.annotations,groups:state.groups}
  }
  it('tracks layout and measured size edits while selection and equivalent parameters keep the document sequence', () => {
    const store = createEditorStore()
    const id = store.getState().addAction({editorKind:'action',nodeType:'if',typeVersion:1,label:'Decision',key:'decision',parameters:{cases:[]},contextWrites:[],resourceReferences:[],settings:{},disabled:false})
    const original = store.getState().localDocumentRevision
    store.getState().onNodesChange([{type:'select',id,selected:true}])
    expect(store.getState().localDocumentRevision).toBe(original)
    store.getState().onNodesChange([{type:'position',id,position:{x:123,y:456},dragging:true}])
    const moved = store.getState().localDocumentRevision
    expect(moved).toBeGreaterThan(original)
    expect(store.getState().markSaved(original)).toBe(false)
    store.getState().onNodesChange([{type:'position',id,position:{x:123,y:456},dragging:false}])
    expect(store.getState().localDocumentRevision).toBe(moved)
    store.getState().onNodesChange([{type:'dimensions',id,dimensions:{width:240,height:150}}])
    const measured = store.getState().localDocumentRevision
    expect(measured).toBeGreaterThan(moved)
    store.getState().updateNode(id,{parameters:{cases:[]}})
    expect(store.getState().localDocumentRevision).toBe(measured)
    store.getState().updateNode(id,{label:'Revised'})
    expect(store.getState().localDocumentRevision).toBeGreaterThan(measured)
  })
  it('keeps later edits dirty when a previous save finishes', () => {
    const store=createEditorStore()
    store.getState().addAnnotation({x:10,y:10},'First')
    const submitted=store.getState().localDocumentRevision
    store.getState().setStart({inputs:{type:'object',properties:{name:{type:'string'}}},contexts:{}})
    expect(store.getState().markSaved(submitted)).toBe(false)
    expect(store.getState().dirty).toBe(true)
    expect(store.getState().markSaved(store.getState().localDocumentRevision)).toBe(true)
  })
  it('applies a batch once, undoes it as one transaction, and never replays it after Undo', () => {
    const store=createEditorStore()
    const candidate={...document(store),annotations:[{id:'note',text:'AI note',x:10,y:10,width:240,height:160}],groups:[]}
    expect(store.getState().applyEditorCommand('command-1',0,candidate)).toBe('applied')
    expect(store.getState().past).toHaveLength(1)
    expect(store.getState().applyEditorCommand('command-1',0,candidate)).toBe('already_applied')
    store.getState().undo()
    expect(store.getState().annotations).toHaveLength(0)
    expect(store.getState().applyEditorCommand('command-1',0,candidate)).toBe('already_applied')
    expect(store.getState().annotations).toHaveLength(0)
  })
  it('rejects a command after a manual edit and isolates two draft stores', () => {
    const left=createEditorStore(),right=createEditorStore()
    const candidate=document(left)
    left.getState().addAnnotation({x:1,y:1},'Manual')
    expect(left.getState().applyEditorCommand('stale',0,candidate)).toBe('stale')
    expect(left.getState().annotations).toHaveLength(1)
    expect(right.getState().annotations).toHaveLength(0)
    expect(right.getState().dirty).toBe(false)
  })
  it('enforces a read-only version inside the dispatcher and manual mutations', () => {
    const store=createEditorStore(true)
    store.getState().hydrate(document(createEditorStore()))
    store.getState().addAnnotation({x:1,y:1},'Forbidden')
    store.getState().setEnd({completion:'all_complete',outputs:{},error:{outputs:{}}})
    expect(store.getState().annotations).toHaveLength(0)
    expect(store.getState().end.completion).toBe('first_return')
    expect(store.getState().applyEditorCommand('readonly',store.getState().localDocumentRevision,document(store))).toBe('read_only')
    expect(store.getState().dirty).toBe(false)
  })
})

describe('editor store exit nodes', () => {
  beforeEach(() => {
    useEditorStore.setState({
      nodes: [
        { id: 'trigger', type: 'manifest', position: { x: 0, y: 0 }, data: { editorKind: 'action', nodeType: 'set', typeVersion: 1, label: 'Set', key: 'set', parameters: {}, contextWrites: [], resourceReferences: [], settings: {}, disabled: false } },
        { id: 'exit-main', type: 'exit', position: { x: 400, y: 0 }, data: { editorKind: 'exit', key: 'exit', label: 'End', protected: true, parameters: { outputs: {}, errorOutputs: {} } } },
        { id: 'exit-extra', type: 'exit', position: { x: 400, y: 160 }, data: { editorKind: 'exit', key: 'exit_2', label: 'End 2', protected: false, parameters: { outputs: {}, errorOutputs: {} } } },
      ],
      edges: [], viewport: { x: 0, y: 0, zoom: 1 }, annotations: [], groups: [],
      settings: { executionOrder: 'deterministic', activationBudget: 10_000 },
      dirty: false, past: [], future: [], selectedId: undefined,
    })
  })

  it('adds a removable exit node and keeps it out of the action list', () => {
    const id = useEditorStore.getState().addExit()
    const state = useEditorStore.getState()
    const added = state.nodes.find((node) => node.id === id)
    expect(added?.data.editorKind).toBe('exit')
    expect(added && 'protected' in added.data && added.data.protected).toBe(false)
    expect(state.selectedId).toBe(id)
    expect(useEditorStore.getState().past.length).toBe(1)
  })

  it('refuses to remove the protected exit but removes manual ones', () => {
    const store = useEditorStore.getState()
    store.select('exit-main')
    useEditorStore.getState().removeSelected()
    expect(useEditorStore.getState().nodes.some((node) => node.id === 'exit-main')).toBe(true)

    useEditorStore.getState().select('exit-extra')
    useEditorStore.getState().removeSelected()
    expect(useEditorStore.getState().nodes.some((node) => node.id === 'exit-extra')).toBe(false)
  })

  it('drops remove changes that target protected exits', () => {
    useEditorStore.getState().onNodesChange([{ id: 'exit-main', type: 'remove' }])
    expect(useEditorStore.getState().nodes.some((node) => node.id === 'exit-main')).toBe(true)
  })
})

describe('editor store node creation', () => {
  beforeEach(() => {
    useEditorStore.setState({ nodes: [], edges: [], viewport: { x: 0, y: 0, zoom: 1 }, annotations: [], groups: [], settings: { executionOrder: 'deterministic', activationBudget: 10_000 }, dirty: false, past: [], future: [], selectedId: undefined })
  })

  it('selects a newly added action so it can be configured immediately', () => {
    useEditorStore.getState().addAction({ editorKind: 'action', nodeType: 'set', typeVersion: 1, label: 'Set', key: 'set', parameters: {}, contextWrites: [], resourceReferences: [], settings: {}, disabled: false })

    const state = useEditorStore.getState()
    expect(state.selectedId).toBe(state.nodes[0].id)
    expect(state.nodes[0].selected).toBe(true)
  })

  it('keeps stable upstream output selectors when a node key changes', () => {
    const source = useEditorStore.getState().addAction({ editorKind: 'action', nodeType: 'set', typeVersion: 1, label: 'Source', key: 'source', parameters: {}, contextWrites: [], resourceReferences: [], settings: {}, disabled: false })
    const reference = { kind: 'reference', selector: { namespace: 'outputs', sourceNodeId: source, port: 'main', run: { kind: 'current' }, item: { kind: 'first' }, path: ['value'] }, missingPolicy: { kind: 'error' } }
    const target = useEditorStore.getState().addAction({ editorKind: 'action', nodeType: 'set', typeVersion: 1, label: 'Target', key: 'target', parameters: { value: reference }, contextWrites: [], resourceReferences: [], settings: {}, disabled: false })
    useEditorStore.getState().updateNode(source, { key: 'renamed' })
    const node = useEditorStore.getState().nodes.find((item) => item.id === target)
    expect(node?.data.editorKind === 'action' && node.data.parameters.value).toEqual(reference)
  })

  it('merges consecutive parameter patches without losing another editor field', () => {
    const code = useEditorStore.getState().addAction({ editorKind: 'action', nodeType: 'code', typeVersion: 1, label: 'Code', key: 'code', parameters: { runner: 'python', outputExample: {} }, contextWrites: [], resourceReferences: [], settings: {}, disabled: false })
    useEditorStore.getState().updateNode(code, { parameters: { source: 'return input' } })
    useEditorStore.getState().updateNode(code, { parameters: { outputExample: { answer: '' } } })
    const node = useEditorStore.getState().nodes.find((item) => item.id === code)
    expect(node?.data.editorKind === 'action' && node.data.parameters).toMatchObject({ runner: 'python', source: 'return input', outputExample: { answer: '' } })
  })

  it('removes deleted dynamic branch edges in the same undoable edit', () => {
    const branch = useEditorStore.getState().addAction({ editorKind: 'action', nodeType: 'if', typeVersion: 1, label: 'IF', key: 'if', parameters: { cases: [{ id: 'keep', name: 'Keep', conditions: [] }, { id: 'remove', name: 'Remove', conditions: [] }] }, contextWrites: [], resourceReferences: [], settings: {}, disabled: false })
    const target = useEditorStore.getState().addAction({ editorKind: 'action', nodeType: 'set', typeVersion: 1, label: 'Target', key: 'target', parameters: {}, contextWrites: [], resourceReferences: [], settings: {}, disabled: false })
    useEditorStore.setState({
      edges: [
        { id: 'keep-edge', source: branch, sourceHandle: 'case:keep', target, targetHandle: 'main', type: 'studio', data: { edgeKind: 'execution', order: 0 } },
        { id: 'remove-edge', source: branch, sourceHandle: 'case:remove', target, targetHandle: 'main', type: 'studio', data: { edgeKind: 'execution', order: 1 } },
      ],
      past: [],
      future: [],
      dirty: false,
    })

    useEditorStore.getState().updateNode(branch, { parameters: { cases: [{ id: 'keep', name: 'Keep', conditions: [] }] } })
    expect(useEditorStore.getState().edges.map((edge) => edge.id)).toEqual(['keep-edge'])
    useEditorStore.getState().undo()
    expect(useEditorStore.getState().edges.map((edge) => edge.id)).toEqual(['keep-edge', 'remove-edge'])
  })

  it('switches Merge input modes by removing incompatible edges in one undoable edit', () => {
    const source = { editorKind: 'action' as const, nodeType: 'set', typeVersion: 1, label: 'Source', key: 'source', parameters: {}, contextWrites: [], resourceReferences: [], settings: {}, disabled: false }
    const left = useEditorStore.getState().addAction(source)
    const right = useEditorStore.getState().addAction({ ...source, key: 'right' })
    const merge = useEditorStore.getState().addAction({ ...source, nodeType: 'merge', label: 'Merge', key: 'merge', parameters: { mode: 'append' } })
    useEditorStore.setState({
      edges: [
        { id: 'append-left', source: left, sourceHandle: 'main', target: merge, targetHandle: 'main', type: 'studio', data: { edgeKind: 'execution', order: 0 } },
        { id: 'append-right', source: right, sourceHandle: 'main', target: merge, targetHandle: 'main:1', type: 'studio', data: { edgeKind: 'execution', order: 1 } },
      ],
      past: [], future: [], dirty: false,
    })

    useEditorStore.getState().updateNode(merge, { parameters: { mode: 'combine_by_key', leftField: 'id', rightField: 'id' } })
    expect(useEditorStore.getState().edges).toHaveLength(0)
    expect(useEditorStore.getState().past).toHaveLength(1)

    useEditorStore.getState().undo()
    expect(useEditorStore.getState().edges.map((edge) => edge.id)).toEqual(['append-left', 'append-right'])
    const restored = useEditorStore.getState().nodes.find((node) => node.id === merge)
    expect(restored?.data.editorKind === 'action' && restored.data.parameters.mode).toBe('append')
  })

  it('aligns a multi-selection as one undoable command', () => {
    const data = { editorKind: 'action' as const, nodeType: 'set', typeVersion: 1, label: 'Set', key: 'set', parameters: {}, contextWrites: [], resourceReferences: [], settings: {}, disabled: false }
    useEditorStore.getState().addAction(data, { x: 80, y: 40 })
    useEditorStore.getState().addAction(data, { x: 260, y: 160 })
    useEditorStore.setState((state) => ({ nodes: state.nodes.map((node) => ({ ...node, selected: true })), selectedId: undefined, past: [], future: [] }))

    useEditorStore.getState().alignSelected('left')
    expect(useEditorStore.getState().nodes.map((node) => node.position.x)).toEqual([80, 80])
    expect(useEditorStore.getState().past).toHaveLength(1)
    useEditorStore.getState().undo()
    expect(useEditorStore.getState().nodes.map((node) => node.position.x)).toEqual([80, 260])
  })

  it('keeps annotation and group edits in the same undo history', () => {
    const data = { editorKind: 'action' as const, nodeType: 'set', typeVersion: 1, label: 'Set', key: 'set', parameters: {}, contextWrites: [], resourceReferences: [], settings: {}, disabled: false }
    useEditorStore.getState().addAction(data, { x: 80, y: 40 })
    useEditorStore.getState().addAction(data, { x: 260, y: 40 })
    useEditorStore.setState((state) => ({ nodes: state.nodes.map((node) => ({ ...node, selected: true })), selectedId: undefined, past: [], future: [] }))
    useEditorStore.getState().addAnnotation({ x: 20, y: 20 })
    const annotationId = useEditorStore.getState().annotations[0].id
    useEditorStore.getState().updateAnnotation(annotationId, { width: 300, height: 180 })
    useEditorStore.getState().addGroup('Main')
    const groupId = useEditorStore.getState().groups[0].id
    useEditorStore.getState().toggleGroup(groupId)
    expect(useEditorStore.getState().groups[0].collapsed).toBe(true)
    useEditorStore.getState().undo()
    expect(useEditorStore.getState().groups[0].collapsed).toBe(false)
    useEditorStore.getState().undo()
    expect(useEditorStore.getState().annotations[0]).toMatchObject({ width: 300, height: 180 })
  })

  it('keeps viewport navigation outside document history', () => {
    useEditorStore.getState().beginEdit()
    useEditorStore.getState().setViewport({ x: -180, y: 90, zoom: 0.7 })
    expect(useEditorStore.getState().viewport).toEqual({ x: -180, y: 90, zoom: 0.7 })

    useEditorStore.getState().commitEdit()
    expect(useEditorStore.getState().past).toHaveLength(0)
    useEditorStore.getState().undo()
    expect(useEditorStore.getState().viewport).toEqual({ x: -180, y: 90, zoom: 0.7 })
  })

  it('adds missing boundary layouts on the first drag and restores them with undo', () => {
    useEditorStore.setState({ boundaryLayouts: [], past: [], future: [], dirty: false })

    useEditorStore.getState().beginEdit({ boundary: 'start' })
    useEditorStore.getState().updateBoundaryPosition('start', { x: 180, y: 120 })
    useEditorStore.getState().commitEdit()

    expect(useEditorStore.getState().boundaryLayouts).toEqual([{ boundary: 'start', x: 180, y: 120 }])
    expect(useEditorStore.getState().past).toHaveLength(1)
    expect(useEditorStore.getState().dirty).toBe(true)
    useEditorStore.getState().undo()
    expect(useEditorStore.getState().boundaryLayouts).toEqual([])
    useEditorStore.getState().redo()
    expect(useEditorStore.getState().boundaryLayouts).toEqual([{ boundary: 'start', x: 180, y: 120 }])
  })

  it('stores the Start boundary position', () => {
    useEditorStore.setState({ boundaryLayouts: [], past: [], future: [] })

    useEditorStore.getState().updateBoundaryPosition('start', { x: 80, y: 140 })

    expect(useEditorStore.getState().boundaryLayouts).toEqual([
      { boundary: 'start', x: 80, y: 140 },
    ])
  })

  it('adds and connects a quick-add node as one undoable command', () => {
    const data = { editorKind: 'action' as const, nodeType: 'set', typeVersion: 1, label: 'Set', key: 'set', parameters: {}, contextWrites: [], resourceReferences: [], settings: {}, disabled: false }
    const sourceId = useEditorStore.getState().addAction(data, { x: 20, y: 40 })
    useEditorStore.setState({ past: [], future: [] })

    useEditorStore.getState().addConnectedAction(data, { nodeId: sourceId, handleId: 'main', targetHandle: 'main' }, { x: 260, y: 40 })
    expect(useEditorStore.getState().nodes).toHaveLength(2)
    expect(useEditorStore.getState().edges[0]).toMatchObject({ source: sourceId, sourceHandle: 'main', targetHandle: 'main' })
    expect(useEditorStore.getState().past).toHaveLength(1)

    useEditorStore.getState().undo()
    expect(useEditorStore.getState().nodes).toHaveLength(1)
    expect(useEditorStore.getState().edges).toHaveLength(0)
  })

  it('undoes a Group move and Sticky Note resize from their gesture checkpoints', () => {
    const data = { editorKind: 'action' as const, nodeType: 'set', typeVersion: 1, label: 'Set', key: 'set', parameters: {}, contextWrites: [], resourceReferences: [], settings: {}, disabled: false }
    useEditorStore.getState().addAction(data, { x: 80, y: 40 })
    useEditorStore.getState().addAction(data, { x: 260, y: 40 })
    useEditorStore.setState((state) => ({ nodes: state.nodes.map((node) => ({ ...node, selected: true })), selectedId: undefined, past: [], future: [] }))
    useEditorStore.getState().addGroup('Movable')
    const groupId = useEditorStore.getState().groups[0].id
    useEditorStore.setState({ past: [], future: [] })
    useEditorStore.getState().beginEdit()
    useEditorStore.getState().moveGroup(groupId, { x: 40, y: 25 })
    useEditorStore.getState().commitEdit()
    expect(useEditorStore.getState().nodes.map((node) => node.position)).toEqual([{ x: 120, y: 65 }, { x: 300, y: 65 }])
    useEditorStore.getState().undo()
    expect(useEditorStore.getState().nodes.map((node) => node.position)).toEqual([{ x: 80, y: 40 }, { x: 260, y: 40 }])

    useEditorStore.getState().addAnnotation({ x: 20, y: 20 })
    const annotationId = useEditorStore.getState().annotations[0].id
    useEditorStore.setState({ past: [], future: [] })
    useEditorStore.getState().beginEdit({ annotationIds: [annotationId] })
    useEditorStore.getState().updateAnnotationFrame(annotationId, { width: 360, height: 220 })
    useEditorStore.getState().commitEdit()
    useEditorStore.getState().undo()
    expect(useEditorStore.getState().annotations[0]).toMatchObject({ width: 240, height: 160 })
  })

  it('deletes selected edges as one undoable command', () => {
    const data = { editorKind: 'action' as const, nodeType: 'set', typeVersion: 1, label: 'Set', key: 'set', parameters: {}, contextWrites: [], resourceReferences: [], settings: {}, disabled: false }
    const sourceId = useEditorStore.getState().addAction(data)
    const targetId = useEditorStore.getState().addAction(data)
    useEditorStore.getState().connect({ source: sourceId, sourceHandle: 'main', target: targetId, targetHandle: 'main' }, { edgeKind: 'execution', sourcePortKind: 'main' })
    useEditorStore.setState((state) => ({ edges: state.edges.map((edge) => ({ ...edge, selected: true })), past: [], future: [] }))

    useEditorStore.getState().removeSelectedEdges()
    expect(useEditorStore.getState().edges).toHaveLength(0)
    expect(useEditorStore.getState().past).toHaveLength(1)
    useEditorStore.getState().undo()
    expect(useEditorStore.getState().edges).toHaveLength(1)
  })

  it('keeps position frames out of GraphIndex revisions and reconnects as one command', () => {
    const data = { editorKind: 'action' as const, nodeType: 'set', typeVersion: 1, label: 'Set', key: 'set', parameters: {}, contextWrites: [], resourceReferences: [], settings: {}, disabled: false }
    const sourceId = useEditorStore.getState().addAction(data)
    const firstTarget = useEditorStore.getState().addAction(data)
    const secondTarget = useEditorStore.getState().addAction(data)
    useEditorStore.getState().connect({ source: sourceId, sourceHandle: 'main', target: firstTarget, targetHandle: 'main' }, { edgeKind: 'execution', sourcePortKind: 'main' })
    const edgeId = useEditorStore.getState().edges[0].id
    const revision = useEditorStore.getState().graphRevision

    useEditorStore.getState().onNodesChange([{ id: sourceId, type: 'position', position: { x: 420, y: 120 }, dragging: true }])
    expect(useEditorStore.getState().graphRevision).toBe(revision)
    useEditorStore.setState({ past: [], future: [] })
    useEditorStore.getState().reconnectEdge(edgeId, { source: sourceId, sourceHandle: 'main', target: secondTarget, targetHandle: 'main' })
    expect(useEditorStore.getState().edges[0].target).toBe(secondTarget)
    expect(useEditorStore.getState().past).toHaveLength(1)
    useEditorStore.getState().undo()
    expect(useEditorStore.getState().edges[0].target).toBe(firstTarget)
  })

  it('stores only the dragged node in a large-document history patch', () => {
    const data = { editorKind: 'action' as const, nodeType: 'set', typeVersion: 1, label: 'Set', key: 'set', parameters: {}, contextWrites: [], resourceReferences: [], settings: {}, disabled: false }
    const nodes: import('../model/types').StudioNode[] = Array.from({ length: 1_000 }, (_, index) => ({ id: `node-${index}`, type: 'manifest', position: { x: index, y: 0 }, data }))
    useEditorStore.setState({ nodes, past: [], future: [], graphRevision: 7 })
    useEditorStore.getState().beginEdit({ nodeIds: ['node-500'] })
    useEditorStore.getState().onNodesChange([{ id: 'node-500', type: 'position', position: { x: 900, y: 100 }, dragging: true }])
    useEditorStore.getState().commitEdit()

    expect(useEditorStore.getState().past[0].nodes).toHaveLength(1)
    expect(useEditorStore.getState().past[0].nodes[0].id).toBe('node-500')
    expect(useEditorStore.getState().graphRevision).toBe(7)
  })
})

describe('editor store loop containers', () => {
  const LOOP_POSITION = { x: 600, y: 120 }
  let loopId = ''
  let bodyId = ''
  let secondBodyId = ''

  beforeEach(() => {
    const loopData = { editorKind: 'action' as const, nodeType: 'loop_over_items', typeVersion: 1, label: 'Loop', key: 'loop', parameters: { parallelism: 4 }, contextWrites: [], resourceReferences: [], settings: {}, disabled: false }
    const bodyData = { editorKind: 'action' as const, nodeType: 'set', typeVersion: 1, label: 'Body', key: 'body', parameters: {}, contextWrites: [], resourceReferences: [], settings: {}, disabled: false }
    const outsideData = { editorKind: 'action' as const, nodeType: 'set', typeVersion: 1, label: 'Outside', key: 'outside', parameters: {}, contextWrites: [], resourceReferences: [], settings: {}, disabled: false }
    useEditorStore.setState({ nodes: [], edges: [], viewport: { x: 0, y: 0, zoom: 1 }, annotations: [], groups: [], settings: { executionOrder: 'deterministic', activationBudget: 10_000 }, dirty: false, past: [], future: [], selectedId: undefined })
    loopId = useEditorStore.getState().addAction(loopData, LOOP_POSITION)
    bodyId = useEditorStore.getState().addAction({ ...bodyData, parentId: loopId }, { x: 32, y: 128 })
    secondBodyId = useEditorStore.getState().addAction({ ...bodyData, parentId: loopId }, { x: 320, y: 128 })
    useEditorStore.getState().addAction(outsideData, { x: 40, y: 40 })
    useEditorStore.setState({ past: [], future: [], dirty: false })
  })

  it('records chip-origin connections as loop node fan-out', () => {
    useEditorStore.getState().connect({ source: `${loopId}::iteration-start`, sourceHandle: 'main', target: bodyId, targetHandle: 'main' }, { edgeKind: 'execution', order: 0, sourcePortKind: 'main' })
    const edge = useEditorStore.getState().edges[0]
    expect(edge.source).toBe(loopId)
    expect(edge.sourceHandle).toBe('main')
    expect(edge.target).toBe(bodyId)
  })

  it('normalizes chip sources when reconnecting an edge', () => {
    useEditorStore.getState().connect({ source: loopId, sourceHandle: 'main', target: bodyId, targetHandle: 'main' }, { edgeKind: 'execution', order: 0, sourcePortKind: 'main' })
    const edgeId = useEditorStore.getState().edges[0].id
    useEditorStore.getState().reconnectEdge(edgeId, { source: `${loopId}::iteration-start`, sourceHandle: 'main', target: secondBodyId, targetHandle: 'main' })
    expect(useEditorStore.getState().edges[0]).toMatchObject({ source: loopId, target: secondBodyId, sourceHandle: 'main' })
  })

  it('grows the container when a dropped child lands beyond its current frame', () => {
    const loop = useEditorStore.getState().nodes.find((node) => node.id === loopId)
    expect(loop?.width).toBeGreaterThanOrEqual(320 + 240 + 24)
    expect(loop?.height).toBeGreaterThanOrEqual(128 + 140 + 48)
  })

  it('persists a manual container resize as one undoable history entry', () => {
    const before = useEditorStore.getState().nodes.find((node) => node.id === loopId)
    useEditorStore.getState().beginEdit({ nodeIds: [loopId] })
    useEditorStore.getState().updateLoopFrame(loopId, { x: 560, y: 90, width: 760, height: 520 })
    useEditorStore.getState().commitEdit()

    const resized = useEditorStore.getState().nodes.find((node) => node.id === loopId)
    expect(resized).toMatchObject({ position: { x: 560, y: 90 }, width: 760, height: 520 })
    expect(useEditorStore.getState().past).toHaveLength(1)
    useEditorStore.getState().undo()
    const restored = useEditorStore.getState().nodes.find((node) => node.id === loopId)
    expect(restored).toMatchObject({ position: before?.position, width: before?.width, height: before?.height })
  })

  it('keeps children but clears membership and rebases positions when the loop is deleted', () => {
    const child = useEditorStore.getState().nodes.find((node) => node.id === bodyId)
    expect(child?.data.editorKind === 'action' && child.data.parentId).toBe(loopId)
    useEditorStore.getState().select(loopId)
    useEditorStore.getState().removeSelected()

    const state = useEditorStore.getState()
    expect(state.nodes.some((node) => node.id === loopId)).toBe(false)
    const released = state.nodes.find((node) => node.id === bodyId)
    expect(released?.data.editorKind === 'action' && released.data.parentId).toBeUndefined()
    // Relative body coordinates are rebased onto the canvas using the loop origin.
    expect(released?.position).toEqual({ x: LOOP_POSITION.x + 32, y: LOOP_POSITION.y + 128 })
  })

  it('releases children when loop removal flows through onNodesChange', () => {
    useEditorStore.getState().onNodesChange([{ id: loopId, type: 'remove' }])
    const released = useEditorStore.getState().nodes.find((node) => node.id === secondBodyId)
    expect(released?.data.editorKind === 'action' && released.data.parentId).toBeUndefined()
    expect(released?.position).toEqual({ x: LOOP_POSITION.x + 320, y: LOOP_POSITION.y + 128 })
  })

  it('detaches a dragged-out child as one undoable command', () => {
    useEditorStore.getState().detachFromContainer(bodyId)
    const detached = useEditorStore.getState().nodes.find((node) => node.id === bodyId)
    expect(detached?.data.editorKind === 'action' && detached.data.parentId).toBeUndefined()
    expect(detached?.position).toEqual({ x: LOOP_POSITION.x + 32, y: LOOP_POSITION.y + 128 })

    expect(useEditorStore.getState().past).toHaveLength(1)
    useEditorStore.getState().undo()
    const restored = useEditorStore.getState().nodes.find((node) => node.id === bodyId)
    expect(restored?.data.editorKind === 'action' && restored.data.parentId).toBe(loopId)
    expect(restored?.position).toEqual({ x: 32, y: 128 })
  })

  it('excludes container children from cross-level alignment', () => {
    const outside = useEditorStore.getState().nodes.find((node) => node.data.editorKind === 'action' && node.data.key === 'outside')
    useEditorStore.setState({ nodes: useEditorStore.getState().nodes.map((node) => ({ ...node, selected: true })), selectedId: undefined, past: [], future: [] })
    useEditorStore.getState().alignSelected('left')
    const state = useEditorStore.getState()
    // Free nodes align to the minimum free x; the parented child keeps its relative frame.
    expect(state.nodes.find((node) => node.id === outside!.id)?.position.x).toBe(40)
    expect(state.nodes.find((node) => node.id === bodyId)?.position.x).toBe(32)
  })
})
