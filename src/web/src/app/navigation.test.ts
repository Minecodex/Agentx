import { describe, expect, it } from 'vitest'

import { resources } from './i18n/resources'
import { navigationItems } from './navigation'

describe('navigation contract', () => {
  it('contains every primary route exactly once', () => {
    const paths = navigationItems.map((item) => item.path)
    const expectedPaths = ['/', '/workflows', '/applications', '/playground', '/executions', '/approvals', '/notifications', '/datasets', '/insights', '/evaluations', '/runtime', '/agent-sessions', '/credentials', '/models', '/mcp', '/canvas-plugins', '/skills', '/knowledge', '/memory', '/sandbox-profiles', '/resource-grants', '/organization', '/roles']
    expect(paths).toHaveLength(expectedPaths.length)
    expect(new Set(paths).size).toBe(paths.length)
    expect(paths).toEqual(expect.arrayContaining(expectedPaths))
  })

  it('provides a Chinese and English label for every item', () => {
    for (const item of navigationItems) {
      const key = item.labelKey.replace('navigation.', '') as keyof typeof resources['zh-CN']['translation']['navigation']
      expect(resources['zh-CN'].translation.navigation[key]).toBeTruthy()
      expect(resources['en-US'].translation.navigation[key]).toBeTruthy()
    }
  })
})
