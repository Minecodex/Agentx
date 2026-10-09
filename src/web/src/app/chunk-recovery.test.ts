import { beforeEach, afterEach, describe, expect, it, vi } from 'vitest'

import { registerChunkRecovery } from './chunk-recovery'

describe('deployment chunk recovery', () => {
  let target: EventTarget
  let storage: Storage
  let reload: ReturnType<typeof vi.fn>

  beforeEach(() => {
    target = new EventTarget()
    storage = window.sessionStorage
    storage.clear()
    reload = vi.fn()
    vi.stubGlobal('window', {
      addEventListener: target.addEventListener.bind(target),
      sessionStorage: storage,
      location: { reload },
    })
  })

  afterEach(() => vi.unstubAllGlobals())

  it('reloads a stale tab once and allows a persistent failure to reach the error page', () => {
    registerChunkRecovery('/assets/index-old.js')
    const first = new Event('vite:preloadError', { cancelable: true })
    target.dispatchEvent(first)
    expect(reload).toHaveBeenCalledOnce()
    expect(first.defaultPrevented).toBe(true)

    const repeated = new Event('vite:preloadError', { cancelable: true })
    target.dispatchEvent(repeated)
    expect(reload).toHaveBeenCalledOnce()
    expect(repeated.defaultPrevented).toBe(false)
  })

  it('allows recovery after a subsequent deployment in the same tab', () => {
    storage.setItem('agentx.chunk-recovery', '/assets/index-old.js')
    registerChunkRecovery('/assets/index-new.js')
    target.dispatchEvent(new Event('vite:preloadError', { cancelable: true }))
    expect(reload).toHaveBeenCalledOnce()
  })

  it('does not enter a reload loop when tab storage is unavailable', () => {
    vi.stubGlobal('window', {
      addEventListener: target.addEventListener.bind(target),
      get sessionStorage() { throw new DOMException('Storage is blocked', 'SecurityError') },
      location: { reload },
    })
    registerChunkRecovery('/assets/index-old.js')
    const error = new Event('vite:preloadError', { cancelable: true })
    target.dispatchEvent(error)
    expect(reload).not.toHaveBeenCalled()
    expect(error.defaultPrevented).toBe(false)
  })
})
