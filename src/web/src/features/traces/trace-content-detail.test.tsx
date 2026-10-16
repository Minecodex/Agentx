import { render, screen } from '@testing-library/react'
import { afterEach, expect, it, vi } from 'vitest'

import { ThemeProvider } from '../../app/providers/theme-provider'
import { TraceContents } from './trace-content-detail'
import { DesignEffectsContext } from '../workflow-designer/model/design-effects'

afterEach(() => vi.unstubAllGlobals())

it('renders stored plugin evidence without loading executable UI inside a design editor', () => {
  const fetcher = vi.fn()
  vi.stubGlobal('fetch', fetcher)
  render(<ThemeProvider><DesignEffectsContext.Provider value><TraceContents contents={[{
    eventId: 'design-proof', kind: 'plugin_content', occurredAt: '2026-10-14T00:00:00Z', contentRef: null,
    preview: { nodeType: 'acme.mapper', typeVersion: 1, bundleDigest: `sha256:${'a'.repeat(64)}`, contentType: 'acme.mapper/table', contentVersion: 1, data: { storedProof: 42 } },
  }]} executionId="execution-1" /></DesignEffectsContext.Provider></ThemeProvider>)
  expect(screen.getByText('storedProof')).toBeInTheDocument()
  expect(screen.getByText('42')).toBeInTheDocument()
  expect(fetcher).not.toHaveBeenCalled()
})

it('keeps standard plugin Trace data visible when the frozen renderer is missing', async () => {
  vi.stubGlobal('fetch', vi.fn(async () => new Response(JSON.stringify({ code: 'PLUGIN_RENDERER_MISSING' }), { status: 404, headers: { 'Content-Type': 'application/json' } })))
  render(<ThemeProvider><TraceContents contents={[{
    eventId: 'event-plugin', kind: 'plugin_content', occurredAt: '2026-09-06T00:00:00Z', contentRef: null,
    preview: { nodeType: 'acme.mapper', typeVersion: 1, bundleDigest: `sha256:${'a'.repeat(64)}`, contentType: 'acme.mapper/table', contentVersion: 1, data: { rows: 3 } },
  }]} executionId="execution-1" /></ThemeProvider>)

  expect(await screen.findByText(/标准数据仍可查看|Standard data remains visible/)).toBeInTheDocument()
  expect(screen.getByText('rows')).toBeInTheDocument()
  expect(screen.getByText('3')).toBeInTheDocument()
})
