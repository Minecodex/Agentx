import { describe, expect, it } from 'vitest'

import { SUPPORTED_CONTROLS } from '../forms/parameter-field'
import { studioManifest, studioManifestTypes } from './studio-catalog'

describe('generated Studio Catalog fixture', () => {
  it('contains only the thirteen registry-owned action nodes', () => {
    expect([...studioManifestTypes].sort()).toEqual([
      'agent', 'approval', 'code', 'declarative_http', 'if', 'list',
      'loop_over_items', 'merge', 'model', 'reply_message', 'send_message',
      'set', 'sub_workflow',
    ])
  })

  it('uses only controls implemented by the Studio', () => {
    for (const nodeType of studioManifestTypes) {
      for (const field of Object.values(studioManifest(nodeType).uiSchema.fields ?? {})) {
        expect(SUPPORTED_CONTROLS.has(field.control ?? ''), `${nodeType}:${field.control}`).toBe(true)
      }
    }
  })
})
