import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { fireEvent, render, screen } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'

import parameters from '../testing/ai-node-parameters.fixture.json'
import { ToastProvider } from '../../../shared/ui/toast'
import { DesignEffectsContext } from '../model/design-effects'
import { studioManifest, studioManifestTypes } from '../testing/studio-catalog'
import type { ActionNodeData, ReferenceCatalog } from '../model/types'
import { NodeInspector } from './node-inspector'

const catalog: ReferenceCatalog = {
  inputs: ['question', 'rows'].map(name => ({ id: name, label: name, path: `inputs.${name}`, type: name === 'rows' ? 'array' : 'string', children: [], selector: { namespace: 'inputs', path: [name], run: { kind: 'current' }, item: { kind: 'current' } } })),
  contexts: [], outputs: [],
}

describe('AI-created node inspector contract', () => {
  it('covers every built-in action manifest', () => {
    expect(Object.keys(parameters).sort()).toEqual([...studioManifestTypes].sort())
  })
  it.each(Object.entries(parameters))('opens and edits %s with compact bindings from the public protocol', (nodeType, values) => {
    const manifest = studioManifest(nodeType)
    const data: ActionNodeData = { editorKind: 'action', nodeType, typeVersion: manifest.version, label: nodeType, key: nodeType, parameters: values, contextWrites: [], resourceReferences: [], settings: {}, disabled: false }
    const onChange = vi.fn()
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } })
    const before = JSON.stringify(data)
    const view = render(<QueryClientProvider client={client}><ToastProvider><DesignEffectsContext.Provider value={true}>
      <NodeInspector data={data} manifest={manifest} nodeId={nodeType} onChange={onChange} onDelete={vi.fn()} referenceCatalog={catalog} resources={{}} />
    </DesignEffectsContext.Provider></ToastProvider></QueryClientProvider>)
    expect(screen.getByTestId('node-details-view')).toBeInTheDocument()
    expect(screen.queryByText(/Unsupported UI control|不支持的 UI 控件|Something went wrong/)).not.toBeInTheDocument()
    const basics = screen.getByTestId('node-basics')
    fireEvent.click(basics.querySelector('summary')!)
    const name = basics.querySelector('input')!
    fireEvent.change(name, { target: { value: `${nodeType} edited` } })
    fireEvent.blur(name)
    expect(onChange).toHaveBeenCalled()
    expect(JSON.stringify(data)).toBe(before)
    view.unmount()
    client.clear()
  })
})
