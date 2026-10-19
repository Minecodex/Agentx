import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { fireEvent, render, screen } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'

import { DebugRunDialog } from './debug-run-dialog'

describe('DebugRunDialog', () => {
  it('submits parsed manual JSON as an explicit input source', () => {
    const onRun = vi.fn()
    render(<QueryClientProvider client={new QueryClient()}><DebugRunDialog mode="single_node" onClose={vi.fn()} onRun={onRun} open running={false} targetName="Code" /></QueryClientProvider>)

    fireEvent.change(screen.getByLabelText('Input'), { target: { value: '{"customerId":42}' } })
    fireEvent.click(screen.getByRole('button', { name: 'Run' }))

    expect(onRun).toHaveBeenCalledWith(
      { kind: 'manual', value: { customerId: 42 } },
      { customerId: 42 },
      {},
    )
  })
})
