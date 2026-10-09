import { describe, expect, it } from 'vitest'

import type { Trace, TraceSpan } from '../../shared/api/types'
import { mergeTracePages, traceRefetchInterval } from './use-trace'

describe('Trace pagination merge', () => {
  it('deduplicates updated spans and preserves stable tuple order', () => {
    const first = span('a', '2026-08-19T00:00:02Z', 'running')
    const second = span('b', '2026-08-19T00:00:01Z', 'succeeded')
    const pages = [page([first, second]), page([{ ...first, status: 'failed' }])]
    expect(mergeTracePages(pages).map((value) => [value.spanId, value.status])).toEqual([
      ['b', 'succeeded'],
      ['a', 'failed'],
    ])
  })
})

describe('Trace refresh policy', () => {
  it('keeps polling after diagnostics fail so a recovered ClickHouse becomes visible', () => {
    expect(traceRefetchInterval('error')).toBe(3_000)
    expect(traceRefetchInterval('success', [page([])])).toBe(false)
  })
})

function page(spans: TraceSpan[]): Trace {
  return { executionId: 'execution', traceId: 'trace', expectedWatermark: 2, ingestedWatermark: 2, expectedEventCount: 2, ingestedEventCount: 2, complete: true, degraded: false, warningCode: null, totalSpans: spans.length, nextCursor: null, spans }
}

function span(spanId: string, startedAt: string, status: string): TraceSpan {
  return { spanId, parentSpanId: null, spanKind: 'execution', spanName: spanId, status, startedAt, endedAt: null, durationMs: null, costMicros: 0, hasDetails: true }
}
