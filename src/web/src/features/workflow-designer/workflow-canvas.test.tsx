import * as Tooltip from '@radix-ui/react-tooltip'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { MemoryRouter, Route, Routes } from 'react-router-dom'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import { ToastProvider } from '../../shared/ui/toast'
import { i18n } from '../../app/i18n'
import { ThemeProvider } from '../../app/providers/theme-provider'
import { deserializeDraft, serializeStudio } from './model/serializer'
import { samePersistedDraft } from './model/draft-persistence'
import type { NodeManifest, WorkflowDefinition } from './model/types'
import { createEditorStore, EditorStoreProvider } from './store/editor-store'
const useEditorStore = createEditorStore()
import { WorkflowCanvasEditor } from './workflow-canvas'
import { createCanvasRenderStore } from './store/canvas-render-store'
const renderStore = createCanvasRenderStore()
vi.mock('./store/canvas-render-store', async (importOriginal) => {
  const actual = await importOriginal<typeof import('./store/canvas-render-store')>()
  return { ...actual, CanvasRenderProvider: ({ children }: { children: React.ReactNode }) => <actual.CanvasRenderProvider store={renderStore}>{children}</actual.CanvasRenderProvider> }
})

const editorDocument = { nodeLayouts: [{ nodeId: 'trigger', x: 100, y: 100 }, { nodeId: 'exit', x: 560, y: 100 }], boundaryLayouts: [], edges: [], annotations: [], groups: [], viewport: { x: 0, y: 0, zoom: 1 } }
const draft = {
  versionId:'version-1',versionNumber:1,publicationStatus:'unpublished',archived:false,generation:0, baseVersionId: null, id: 'draft-1', workflowId: 'workflow-1', revision: 1, schemaVersion: '8.0', definitionHash: 'sha256:def', editorHash: 'sha256:editor', updatedAt: '2026-08-02T10:00:00Z', editorDocument,
  definition: { schemaVersion: '8.0', start: { inputs: {}, contexts: {} }, settings: { executionOrder: 'deterministic', activationBudget: 10000 }, nodes: [{ id: 'trigger', key: 'source', type: 'set', typeVersion: 1, name: 'Set', disabled: false, protected: false, parameters: {}, contextWrites: [], resourceReferences: [], settings: {} }, { id: 'exit', key: 'exit', type: 'exit', typeVersion: 1, name: 'End', disabled: false, protected: true, parameters: { outputs: {}, errorOutputs: {} }, contextWrites: [], resourceReferences: [], settings: {} }], connections: [{ id: 'start-trigger', sourceNodeId: '__start__', sourceHandle: 'main', targetNodeId: 'trigger', targetHandle: 'main', order: 0 }, { id: 'trigger-exit', sourceNodeId: 'trigger', sourceHandle: 'main', targetNodeId: 'exit', targetHandle: 'main', order: 0 }], end: { completion: "first_return", outputs: {}, error: { outputs: { } } } },
}

let requiredAgentBinding = false
let saveConflict = false

const manifest = (nodeType: string, executionStyle = 'action'): NodeManifest => ({
  protocolVersion: '3.0', nodeType, version: nodeType === 'agent' ? 2 : 1, displayName: nodeType === 'agent' ? 'Agent' : 'Manual Trigger', description: '', category: nodeType === 'agent' ? 'ai' : 'triggers', keywords: [nodeType], iconKey: nodeType === 'agent' ? 'bot' : 'mouse-pointer-click', executionStyle: executionStyle as NodeManifest['executionStyle'], capability: nodeType === 'agent' ? 'agent' : 'builtin', readiness: 'any', inputPorts: nodeType === 'agent' ? [{ name: 'main', kind: 'main', required: true, variadic: false }] : [], outputPorts: [{ name: 'main', kind: 'main', required: false, variadic: false }], bindingSlots: nodeType === 'agent' ? [{ name: 'model', resourceType: 'model', placement: 'inspector', required: requiredAgentBinding, multiple: false }] : [], parameterSchema: { type: 'object', properties: {} }, uiSchema: { canvas: { role: nodeType === 'agent' ? 'agent' : 'trigger' } }, plugin: nodeType === 'agent' ? { packageId: 'agentx/core', packageVersion: '1.0.0', bundleDigest: 'sha256:test', runtimeEntry: 'native', runtimeSource: '' } : undefined, providers: [], credentials: [], retryPolicy: { retryable: false, maxAttempts: 1, initialBackoffMs: 0, maxBackoffMs: 0 }, sandboxRequired: false, supportsMock: true, sideEffectLevel: 'none',
})

function json(value: unknown, status = 200) { return new Response(JSON.stringify(value), { status, headers: { 'Content-Type': 'application/json' } }) }

vi.mock('../../app/providers/auth-provider', () => ({useAuth: () => ({user: {id:'user-1',companyId:'tenant-1'},hasPermission:()=>true})}))
describe('workflow studio shell', () => {
  beforeEach(() => {
    requiredAgentBinding = false
    saveConflict = false
    localStorage.clear()
    useEditorStore.setState({ nodes: [], edges: [], viewport: { x: 0, y: 0, zoom: 1 }, annotations: [], groups: [], settings: { executionOrder: 'deterministic', activationBudget: 10_000 }, dirty: false, past: [], future: [], selectedId: undefined })
    vi.stubGlobal('fetch', vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const path = new URL(String(input), 'http://agentx.test').pathname
      if (path.endsWith('/drafts/draft-1') && init?.method === 'PUT') {
        if (saveConflict) return json({ code: 'DRAFT_REVISION_CONFLICT', message: 'Draft changed', requestId: 'request-1' }, 409)
        return json({ ...draft, revision: draft.revision + 1 })
      }
      if (path.endsWith('/drafts/draft-1')) return json(draft)
      if (path.endsWith('/node-definitions')) return json({ items: [{ nodeType: 'set', version: 1 }, { nodeType: 'agent', version: 2 }] })
      if (path.includes('/node-definitions/set/')) return json({ nodeType: 'set', version: 1, manifest: manifest('set') })
      if (path.includes('/node-definitions/agent/')) return json({ nodeType: 'agent', version: 2, manifest: manifest('agent') })
      if (path.endsWith('/workflows/workflow-1')) return json({ id: 'workflow-1', name: 'Conflict Workflow' })
      if (path.endsWith('/environments') || path.endsWith('/versions') || path.endsWith('/mcp/tools')) return json([])
      return json({ items: [], page: 1, pageSize: 100, total: 0 })
    }))
  })

  it('waits for the authoritative revision before hydrating a cached editor after publication', async () => {
    const fallback=vi.mocked(fetch).getMockImplementation()!
    let release!: (value:Response)=>void
    const response=new Promise<Response>(resolve=>{release=resolve})
    vi.mocked(fetch).mockImplementation((input,init)=>String(input).endsWith('/drafts/draft-1') && !init?.method ? response : fallback(input,init))
    const queryClient=new QueryClient({defaultOptions:{queries:{retry:false}}})
    queryClient.setQueryData(['workflow-document','workflow-1','draft-1'],draft)
    render(<ThemeProvider><QueryClientProvider client={queryClient}><Tooltip.Provider><ToastProvider><MemoryRouter initialEntries={['/workflows/workflow-1/versions/version-1/editor']}><Routes><Route path="/workflows/:workflowId/versions/:versionId/editor" element={<EditorStoreProvider store={useEditorStore}><WorkflowCanvasEditor documentId="draft-1" /></EditorStoreProvider>}/></Routes></MemoryRouter></ToastProvider></Tooltip.Provider></QueryClientProvider></ThemeProvider>)
    expect(useEditorStore.getState().nodes).toEqual([])
    const fresh=structuredClone(draft)
    fresh.revision=4
    fresh.definition.nodes[0]!.name='Published baseline'
    await act(async()=>{release(json(fresh));await response})
    await waitFor(()=>expect(useEditorStore.getState().nodes.find(node=>node.id==='trigger')?.data.label).toBe('Published baseline'))
    act(()=>useEditorStore.getState().updateNode('trigger',{label:'New edit'}))
    fireEvent.click(screen.getByRole('button',{name:'Save'}))
    await waitFor(()=>expect(vi.mocked(fetch).mock.calls.some(([,init])=>init?.method==='PUT'&&JSON.parse(String(init.body)).expectedRevision===4)).toBe(true))
  })

  it('keeps version details read-only for toolbar, node mutations and keyboard shortcuts', async () => {
    const fallback=vi.mocked(fetch).getMockImplementation()!
    vi.mocked(fetch).mockImplementation((input,init)=>String(input).endsWith('/versions/version-1')
      ? Promise.resolve(json({...draft,contentHash:draft.definitionHash,sourceRevision:1,nodeManifests:{}})) : fallback(input,init))
    const queryClient=new QueryClient({defaultOptions:{queries:{retry:false}}})
    render(<ThemeProvider><QueryClientProvider client={queryClient}><Tooltip.Provider><ToastProvider><MemoryRouter initialEntries={['/workflows/workflow-1/versions/version-1']}><Routes><Route path="/workflows/:workflowId/versions/:versionId" element={<EditorStoreProvider store={useEditorStore} readOnly><WorkflowCanvasEditor viewOnly /></EditorStoreProvider>}/></Routes></MemoryRouter></ToastProvider></Tooltip.Provider></QueryClientProvider></ThemeProvider>)
    await screen.findByTestId('workflow-canvas')
    await waitFor(()=>expect(useEditorStore.getState().nodes).toHaveLength(2))
    const original=serializeStudio(useEditorStore.getState())
    for(const name of ['Save','Run','Restore','AI creation']) expect(screen.queryByRole('button',{name})).not.toBeInTheDocument()
    expect(screen.queryByTestId('node-creator-shell')).not.toBeInTheDocument()
    act(()=>useEditorStore.getState().updateNode('trigger',{label:'Forbidden'}))
    fireEvent.keyDown(window,{key:'Delete'})
    fireEvent.keyDown(window,{key:'s',ctrlKey:true})
    expect(serializeStudio(useEditorStore.getState())).toEqual(original)
    expect(vi.mocked(fetch).mock.calls.filter(([,init])=>init?.method==='POST'||init?.method==='PUT')).toHaveLength(0)
  })

  it('restores the authoritative document only after confirmation and clears history', async () => {
    const fallback=vi.mocked(fetch).getMockImplementation()!
    let restored=false
    vi.mocked(fetch).mockImplementation((input,init)=>{
      if(String(input).endsWith('/versions/version-1/restore')) {restored=true;return Promise.resolve(json({revision:2,versionId:'version-1'}))}
      if(restored&&String(input).endsWith('/drafts/draft-1'))return Promise.resolve(json({...draft,revision:2}))
      return fallback(input,init)
    })
    const queryClient=new QueryClient({defaultOptions:{queries:{retry:false}}})
    render(<ThemeProvider><QueryClientProvider client={queryClient}><Tooltip.Provider><ToastProvider><MemoryRouter initialEntries={['/workflows/workflow-1/versions/version-1/editor']}><Routes><Route path="/workflows/:workflowId/versions/:versionId/editor" element={<EditorStoreProvider store={useEditorStore}><WorkflowCanvasEditor documentId="draft-1" /></EditorStoreProvider>}/></Routes></MemoryRouter></ToastProvider></Tooltip.Provider></QueryClientProvider></ThemeProvider>)
    await screen.findByTestId('workflow-canvas')
    act(()=>useEditorStore.getState().updateNode('trigger',{label:'Unsaved modification'}))
    fireEvent.click(screen.getByRole('button',{name:'Restore'}))
    fireEvent.click(await screen.findByRole('button',{name:'Cancel'}))
    expect(restored).toBe(false)
    expect(useEditorStore.getState().dirty).toBe(true)
    fireEvent.click(screen.getByRole('button',{name:'Restore'}))
    fireEvent.click(await screen.findByRole('button',{name:'Confirm restore'}))
    await waitFor(()=>expect(useEditorStore.getState().dirty).toBe(false))
    expect(restored).toBe(true)
    expect(useEditorStore.getState().past).toEqual([])
    expect(useEditorStore.getState().future).toEqual([])
    expect(serializeStudio(useEditorStore.getState())).toEqual(serializeStudio(deserializeDraft(draft)))
  })

  it('keeps resolved render contracts stable when a drag only changes toolbar and position state', async () => {
    const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } })
    render(<ThemeProvider><QueryClientProvider client={queryClient}><Tooltip.Provider><ToastProvider><MemoryRouter initialEntries={['/workflows/workflow-1/versions/version-1/editor']}><Routes><Route element={<EditorStoreProvider store={useEditorStore}><WorkflowCanvasEditor documentId="draft-1" /></EditorStoreProvider>} path="/workflows/:workflowId/versions/:versionId/editor" /></Routes></MemoryRouter></ToastProvider></Tooltip.Provider></QueryClientProvider></ThemeProvider>)
    await screen.findByTestId('workflow-canvas')
    const contracts = renderStore.getState().manifests
    expect(contracts.size).toBeGreaterThan(0)
    act(() => useEditorStore.getState().onNodesChange([{ id: 'trigger', type: 'position', position: { x: 140, y: 180 }, dragging: true }]))
    await screen.findByText(/Unsaved/)
    expect(renderStore.getState().manifests).toBe(contracts)
  })

  it('keeps edits made during an earlier save dirty and available to the next save', async () => {
    const fallback = vi.mocked(fetch).getMockImplementation()!
    let finishSave: ((response: Response) => void) | undefined
    vi.mocked(fetch).mockImplementation((input, init) => {
      if (String(input).endsWith('/drafts/draft-1') && init?.method === 'PUT') {
        return new Promise<Response>(resolve => { finishSave = resolve })
      }
      return fallback(input, init)
    })
    const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } })
    render(<ThemeProvider><QueryClientProvider client={queryClient}><Tooltip.Provider><ToastProvider><MemoryRouter initialEntries={['/workflows/workflow-1/versions/version-1/editor']}><Routes><Route element={<EditorStoreProvider store={useEditorStore}><WorkflowCanvasEditor documentId="draft-1" /></EditorStoreProvider>} path="/workflows/:workflowId/versions/:versionId/editor" /></Routes></MemoryRouter></ToastProvider></Tooltip.Provider></QueryClientProvider></ThemeProvider>)
    fireEvent.change(await screen.findByRole('textbox', { name: 'Search nodes' }), { target: { value: 'set' } })
    fireEvent.click(await screen.findByTestId('palette-action-set'))
    fireEvent.click(screen.getByRole('button', { name: 'Save' }))
    await waitFor(() => expect(finishSave).toBeDefined())
    const movedNode = useEditorStore.getState().nodes.at(-1)!.id
    act(() => useEditorStore.getState().onNodesChange([{ id: movedNode, type: 'position', position: { x: 990, y: 330 }, dragging: true }]))
    const editedNodes = useEditorStore.getState().nodes.length
    await act(async () => { finishSave!(json({ ...draft, revision: 1 })) })
    await waitFor(() => expect(queryClient.getQueryData<{ revision: number }>(['workflow-document', 'workflow-1', 'draft-1'])?.revision).toBe(1))
    expect(useEditorStore.getState().nodes).toHaveLength(editedNodes)
    expect(useEditorStore.getState().nodes.find(node => node.id === movedNode)?.position).toEqual({ x: 990, y: 330 })
    expect(useEditorStore.getState().dirty).toBe(true)
  })

  it('compares persisted data while ignoring selection changes', () => {
    const saved = deserializeDraft(draft)
    const selection = { ...saved, nodes: saved.nodes.map(node => ({ ...node, selected: true })) }
    expect(samePersistedDraft(saved, selection)).toBe(true)
    const moved = { ...saved, nodes: saved.nodes.map(node => ({ ...node, position: { x: node.position.x + 20, y: node.position.y } })) }
    expect(samePersistedDraft(saved, moved)).toBe(false)
  })

  it('keeps local edits and offers all explicit conflict choices', async () => {
    saveConflict = true
    const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } })
    render(<ThemeProvider><QueryClientProvider client={queryClient}><Tooltip.Provider><ToastProvider><MemoryRouter initialEntries={['/workflows/workflow-1/versions/version-1/editor']}><Routes><Route element={<EditorStoreProvider store={useEditorStore}><WorkflowCanvasEditor documentId="draft-1" /></EditorStoreProvider>} path="/workflows/:workflowId/versions/:versionId/editor" /></Routes></MemoryRouter></ToastProvider></Tooltip.Provider></QueryClientProvider></ThemeProvider>)

    fireEvent.change(await screen.findByRole('textbox', { name: 'Search nodes' }), { target: { value: 'set' } })
    fireEvent.click(screen.getByTestId('palette-action-set'))
    fireEvent.click(screen.getByRole('button', { name: 'Save' }))

    expect((await screen.findAllByText('Draft revision conflict')).length).toBeGreaterThan(0)
    expect(screen.getByRole('button', { name: 'Keep local copy' })).toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Load server revision' })).toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Overwrite server draft' })).toBeInTheDocument()
    await waitFor(() => expect(screen.getByText(/Unsaved/)).toBeInTheDocument())
  })

  it('saves an Agent draft with an unconfigured required model slot', async () => {
    requiredAgentBinding = true
    const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } })
    render(<ThemeProvider><QueryClientProvider client={queryClient}><Tooltip.Provider><ToastProvider><MemoryRouter initialEntries={['/workflows/workflow-1/versions/version-1/editor']}><Routes><Route element={<EditorStoreProvider store={useEditorStore}><WorkflowCanvasEditor documentId="draft-1" /></EditorStoreProvider>} path="/workflows/:workflowId/versions/:versionId/editor" /></Routes></MemoryRouter></ToastProvider></Tooltip.Provider></QueryClientProvider></ThemeProvider>)

    fireEvent.change(await screen.findByRole('textbox', { name: 'Search nodes' }), { target: { value: 'agent' } })
    fireEvent.click(screen.getByTestId('palette-action-agent'))
    act(() => {
      const agent = useEditorStore.getState().nodes.find(node => node.data.editorKind === 'action' && node.data.nodeType === 'agent')!
      useEditorStore.getState().updateNode(agent.id, { parameters: { sessionPolicy: { mode: 'invocation' } } })
    })
    fireEvent.click(screen.getByRole('button', { name: 'Save' }))
    await waitFor(() => {
      const draftWrites = vi.mocked(fetch).mock.calls.filter(([input, init]) => new URL(String(input), 'http://agentx.test').pathname.endsWith('/drafts/draft-1') && init?.method === 'PUT')
      expect(draftWrites.length).toBeGreaterThan(0)
      const submitted = JSON.parse(String(draftWrites[0][1]?.body)) as { definition: WorkflowDefinition }
      expect(submitted.definition.nodes.find(node => node.type === 'agent')?.resourceReferences).toEqual([])
    })
  })

  it('does not submit or autosave a structurally invalid definition', async () => {
    const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } })
    render(<ThemeProvider><QueryClientProvider client={queryClient}><Tooltip.Provider><ToastProvider><MemoryRouter initialEntries={['/workflows/workflow-1/versions/version-1/editor']}><Routes><Route element={<EditorStoreProvider store={useEditorStore}><WorkflowCanvasEditor documentId="draft-1" /></EditorStoreProvider>} path="/workflows/:workflowId/versions/:versionId/editor" /></Routes></MemoryRouter></ToastProvider></Tooltip.Provider></QueryClientProvider></ThemeProvider>)

    await waitFor(() => expect(useEditorStore.getState().nodes).toHaveLength(2))
    vi.mocked(fetch).mockClear()
    act(() => {
      useEditorStore.getState().setEnd({
        completion: "first_return",
        outputs: { 'Invalid-name': { schema: { type: 'string' }, required: false, sensitive: false } },
        error: { outputs: { } },
      })
    })
    const save = screen.getByRole('button', { name: 'Save' })
    await waitFor(() => expect(save).toBeEnabled())
    fireEvent.click(save)

    expect(await screen.findByText('INVALID_END_OUTPUT_KEY')).toBeInTheDocument()
    await new Promise((resolve) => window.setTimeout(resolve, 1600))
    const draftWrites = vi.mocked(fetch).mock.calls.filter(([input, init]) => new URL(String(input), 'http://agentx.test').pathname.endsWith('/drafts/draft-1') && init?.method === 'PUT')
    expect(draftWrites).toHaveLength(0)
  })

  it('locates a validation issue in the selected node details', async () => {
    requiredAgentBinding = true
    const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } })
    render(<ThemeProvider><QueryClientProvider client={queryClient}><Tooltip.Provider><ToastProvider><MemoryRouter initialEntries={['/workflows/workflow-1/versions/version-1/editor']}><Routes><Route element={<EditorStoreProvider store={useEditorStore}><WorkflowCanvasEditor documentId="draft-1" /></EditorStoreProvider>} path="/workflows/:workflowId/versions/:versionId/editor" /></Routes></MemoryRouter></ToastProvider></Tooltip.Provider></QueryClientProvider></ThemeProvider>)

    fireEvent.change(await screen.findByRole('textbox', { name: 'Search nodes' }), { target: { value: 'agent' } })
    fireEvent.click(screen.getByTestId('palette-action-agent'))
    act(() => {
      const agent = useEditorStore.getState().nodes.find(node => node.data.editorKind === 'action' && node.data.nodeType === 'agent')!
      useEditorStore.getState().updateNode(agent.id, { parameters: { sessionPolicy: { mode: 'invocation' } }, resourceReferences: [1, 2].map(id => ({ resourceType: 'model', bindingRole: 'model', operation: 'use', resourceId: `model-${id}`, resourceVersionId: `version-${id}` })) })
    })
    fireEvent.click(screen.getByRole('button', { name: 'Save' }))
    const issue = await screen.findByText('AGENT_RESOURCE_SLOT_INVALID')
    expect(issue.closest('button')).toHaveTextContent(/model/i)

    await i18n.changeLanguage('zh-CN')
    await waitFor(() => expect(issue.closest('button')).toHaveTextContent(/模型|model/i))
    fireEvent.click(issue.closest('button')!)

    const field = document.querySelector<HTMLElement>('[data-field-path="resourceReferences.model"]')
    const picker = field?.querySelector<HTMLElement>('[role="combobox"]')
    await waitFor(() => expect(picker).toHaveFocus())
    expect(field).toHaveClass('studio-field-located')
    await i18n.changeLanguage('en-US')
  })

})

describe('workflow studio serializer', () => {
  it('rejects Definition 6.0 without conversion or fallback', () => {
    expect(() => deserializeDraft({
      ...draft,
      definition: { ...draft.definition, schemaVersion: '6.0' },
    } as never)).toThrow(/Unsupported Workflow Definition 6\.0/)
  })

  it('keeps slot references directly on the node without canvas attachment nodes', () => {
    const definition: WorkflowDefinition = {
      schemaVersion: '8.0', start: { inputs: {}, contexts: {} }, settings: { executionOrder: 'deterministic', activationBudget: 10000 }, connections: [], end: { completion: "first_return", outputs: {}, error: { outputs: { } } },
      nodes: [{ id: 'agent-1', key: 'agent', type: 'agent', typeVersion: 2, name: 'Agent', disabled: false, protected: false, parameters: { sessionPolicy: { mode: 'invocation' } }, contextWrites: [], settings: {}, resourceReferences: [{ bindingRole: 'model', resourceType: 'model', resourceId: '11111111-1111-4111-8111-111111111111', resourceVersionId: '22222222-2222-4222-8222-222222222222', operation: 'use' }, { bindingRole: 'mcp_tools', resourceType: 'mcp_tool', resourceId: '33333333-3333-4333-8333-333333333333', resourceVersionId: '44444444-4444-4444-8444-444444444444', operation: 'use' }] }],
    }
    const source = { ...draft, definition, editorDocument: { ...editorDocument, nodeLayouts: [{ nodeId: 'agent-1', x: 410, y: 230 }] } }
    const document = deserializeDraft(source)
    const serialized = serializeStudio(document)

    expect(document.nodes).toHaveLength(1)
    expect(serialized.definition.schemaVersion).toBe('8.0')
    expect(serialized.definition.nodes).toHaveLength(1)
    expect(serialized.definition.nodes[0]).not.toHaveProperty('position')
    expect(serialized.definition.nodes[0].resourceReferences).toEqual(definition.nodes[0].resourceReferences)
    expect(serialized.editorDocument).not.toHaveProperty('bindingLayouts')
    expect(serialized.editorDocument).not.toHaveProperty('bindingEdges')
  })

  it('normalizes connection order independently for every source port', () => {
    const document = deserializeDraft({
      ...draft,
      definition: {
        ...draft.definition,
        nodes: [
          ...draft.definition.nodes,
          { ...draft.definition.nodes[0], id: 'a' },
          { ...draft.definition.nodes[0], id: 'b' },
          { ...draft.definition.nodes[0], id: 'c' },
        ],
        connections: [
          { id: 'main-b', sourceNodeId: 'trigger', sourceHandle: 'main', targetNodeId: 'b', targetHandle: 'main', order: 8 },
          { id: 'error-a', sourceNodeId: 'trigger', sourceHandle: 'error', targetNodeId: 'a', targetHandle: 'main', order: 9 },
          { id: 'main-c', sourceNodeId: 'trigger', sourceHandle: 'main', targetNodeId: 'c', targetHandle: 'main', order: 2 },
        ],
      },
      editorDocument: { ...editorDocument, nodeLayouts: ['trigger', 'a', 'b', 'c'].map((nodeId, index) => ({ nodeId, x: index * 100, y: 100 })) },
    })
    const connections = serializeStudio(document).definition.connections
    expect(connections.find((edge) => edge.id === 'main-c')?.order).toBe(0)
    expect(connections.find((edge) => edge.id === 'main-b')?.order).toBe(1)
    expect(connections.find((edge) => edge.id === 'error-a')?.order).toBe(0)
  })
})
