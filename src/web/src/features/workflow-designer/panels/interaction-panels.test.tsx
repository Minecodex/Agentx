import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { render, screen } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import { i18n } from '../../../app/i18n'
import { ToastProvider } from '../../../shared/ui/toast'
import type { NodeManifest } from '../model/types'
import catalog from '../testing/studio-catalog.fixture.json'
import { AwaitInputPanel } from './inspectors/await-input-panel'
import { ReplyMessagePanel } from './inspectors/reply-message-panel'
import type { ActionPanelProps } from './inspectors/panel-shell'

function panel(nodeType: string): ActionPanelProps {
  return {
    manifest: catalog.find(node => node.nodeType === nodeType) as unknown as NodeManifest,
    data: { editorKind: 'action', nodeType, typeVersion: 1, label: nodeType, key: nodeType, parameters: {}, contextWrites: [], resourceReferences: [], settings: {}, disabled: false },
    fieldErrors: {}, providerOptions: {}, resources: {}, onChange: vi.fn(),
  }
}

beforeEach(async () => { await i18n.changeLanguage('en-US') })

describe('workflow interaction configuration', () => {
  it('places the one optional Hook field after execution exits and documents raw input for every entry', () => {
    render(<QueryClientProvider client={new QueryClient()}><ToastProvider><AwaitInputPanel {...panel('await_input')} /></ToastProvider></QueryClientProvider>)
    const root = screen.getByTestId('await-input-panel')
    const exits = screen.getByTestId('await-input-exits')
    const hook = screen.getByTestId('await-input-hook')
    expect(exits.compareDocumentPosition(hook) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
    expect(root.lastElementChild).toBe(hook)
    expect(root.querySelectorAll('[data-field-path="parameters.hookUrl"]')).toHaveLength(1)
    expect(root.querySelector('[data-field-path="parameters.candidateUserId"]')).toBeNull()
    expect(screen.getByTestId('await-input-documentation')).toHaveTextContent('Datasets, manual runs, designer debugging, Playground and schedules')
    expect(root).toHaveTextContent('Content passes through unchanged')
    expect(root).toHaveTextContent('Idempotency-Key')
  })

  it('configures a progress message without a Hook or a target-channel picker', () => {
    render(<QueryClientProvider client={new QueryClient()}><ToastProvider><ReplyMessagePanel {...panel('reply_message')} /></ToastProvider></QueryClientProvider>)
    const root = screen.getByTestId('reply-message-panel')
    expect(root.querySelector('[data-field-path="parameters.content"]')).not.toBeNull()
    expect(root.querySelector('[data-field-path="parameters.hookUrl"]')).toBeNull()
    expect(root).toHaveTextContent('Emit an intermediate message and continue')
  })
})
