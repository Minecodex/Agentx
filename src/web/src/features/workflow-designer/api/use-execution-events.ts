import { useEffect, useRef, useState } from 'react'

import { apiRequest } from '../../../shared/api/client'
import type { Execution } from '../../../shared/api/types'
import { loadExecutionEvents, type ExecutionEvent } from './studio-api'
import { mergeRuntimeStatuses, RuntimeStatusBatcher } from './runtime-status-batcher'

const terminal = new Set(['succeeded', 'failed', 'cancelled', 'timed_out'])

export function useExecutionEvents(executionId?: string) {
  const [events, setEvents] = useState<ExecutionEvent[]>([])
  const [running, setRunning] = useState(false)
  const [error, setError] = useState<string>()
  const [nodeStatuses, setNodeStatuses] = useState<Map<string, string>>(new Map())
  const cursor = useRef(0)

  useEffect(() => {
    setEvents([])
    setError(undefined)
    setNodeStatuses(new Map())
    cursor.current = 0
    setRunning(Boolean(executionId))
    if (!executionId) return
    let cancelled = false
    let timer: number | undefined
    let quietPolls = 0
    let finished = false
    const statusBatcher = new RuntimeStatusBatcher((updates) => setNodeStatuses((current) => mergeRuntimeStatuses(current, updates)))
    const mergeNodeStatuses = (items: ExecutionEvent[]) => {
      const updates: Array<[string, string]> = []
      for (const event of items) {
        const summary = event.summary && typeof event.summary === 'object' ? event.summary as Record<string, unknown> : undefined
        const nodeId = summary?.nodeId ?? summary?.node_id
        if (typeof nodeId !== 'string') continue
        updates.push([nodeId, event.status])
      }
      statusBatcher.merge(updates)
    }
    const poll = async () => {
      // Events and status are independent reads: a failed refresh of one must
      // not discard the other, hide prior events, or prevent terminal status.
      const [detail, eventPage] = await Promise.allSettled([
        apiRequest<Execution>(`/executions/${executionId}`),
        loadExecutionEvents(executionId, cursor.current),
      ])
      if (cancelled) return
      const failure = [detail, eventPage].find((result) => result.status === 'rejected')
      setError(failure?.status === 'rejected' ? String(failure.reason instanceof Error ? failure.reason.message : failure.reason) : undefined)
      if (detail.status === 'fulfilled') {
        finished = terminal.has(detail.value.status)
        setRunning(!finished)
      }
      const page = eventPage.status === 'fulfilled' ? eventPage.value : undefined
      if (page?.items.length) {
        quietPolls = 0
        cursor.current = page.nextCursor ?? page.items.at(-1)?.sequence ?? cursor.current
        setEvents((current) => [...current, ...page.items.filter((item) => !current.some((value) => value.sequence === item.sequence))].sort((a, b) => a.sequence - b.sequence))
        mergeNodeStatuses(page.items)
        if (page.items.some((event) => event.eventType.startsWith('execution.') && terminal.has(event.status))) {
          finished = true
          setRunning(false)
        }
      } else quietPolls += 1
      if (failure || !finished) timer = window.setTimeout(poll, failure || quietPolls > 6 ? 2000 : 650)
      else if (page?.items.length === 200) timer = window.setTimeout(poll, 0)
    }
    void poll()
    return () => { cancelled = true; if (timer) window.clearTimeout(timer); statusBatcher.dispose() }
  }, [executionId])
  return { events, nodeStatuses, running, setRunning, error }
}
