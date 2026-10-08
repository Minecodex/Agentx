const reloadKey = 'agentx.chunk-recovery'

export function registerChunkRecovery(buildUrl: string) {
  window.addEventListener('vite:preloadError', (event) => {
    try {
      if (window.sessionStorage.getItem(reloadKey) === buildUrl) return
      window.sessionStorage.setItem(reloadKey, buildUrl)
    } catch {
      // Without persistent tab state, leave recovery to the route error page.
      return
    }
    event.preventDefault()
    window.location.reload()
  })
}
