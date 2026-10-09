import { useEffect, useRef, useState } from 'react'

import { apiRequest } from '../../../shared/api/client'

export type ModelTail = { attemptId: string; nodeKey: string; deltaText: string; reasoningDeltaText?: string | null }
type DeltaPage = { items: Array<{ sequence: number; payload: ModelTail }>; nextCursor: number }

// Previews retain the most recent 20 attempts and 12,000 characters per
// field. They never become node output, Pin or Mock data.
export function mergeModelTails(current: ModelTail[], items: DeltaPage['items']): ModelTail[] {
  const tails = new Map(current.map((tail) => [tail.attemptId, { ...tail }]))
  for (const { payload } of items) {
    const previous = tails.get(payload.attemptId)
    tails.set(payload.attemptId, { ...payload, deltaText: ((previous?.deltaText ?? '') + payload.deltaText).slice(-12_000), reasoningDeltaText: ((previous?.reasoningDeltaText ?? '') + (payload.reasoningDeltaText ?? '')).slice(-12_000) })
  }
  return [...tails.values()].slice(-20)
}

export function useModelDeltas(executionId: string | undefined, running: boolean) {
  const [tails, setTails] = useState<ModelTail[]>([])
  const [error, setError] = useState<string>()
  const active = useRef(running)
  active.current = running
  useEffect(() => {
    setTails([])
    setError(undefined)
    if (!executionId) return
    let cancelled = false
    let cursor = 0
    let timer: number | undefined
    const poll = async () => {
      try {
        const page = await apiRequest<DeltaPage>(`/executions/${executionId}/model-deltas?after=${cursor}&limit=1000`)
        if (cancelled) return
        cursor = page.nextCursor
        setTails((current) => mergeModelTails(current, page.items))
        setError(undefined)
        if (page.items.length === 1000 || active.current) timer = window.setTimeout(poll, page.items.length === 1000 ? 0 : 650)
      } catch (cause) {
        if (!cancelled) { setError((cause as Error).message); if (active.current) timer = window.setTimeout(poll, 2000) }
      }
    }
    void poll()
    return () => { cancelled = true; if (timer) window.clearTimeout(timer) }
  }, [executionId])
  return { tails, error }
}
