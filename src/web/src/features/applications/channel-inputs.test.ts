import { describe, expect, it } from 'vitest'

import type { ApplicationDeployment, ApplicationWebhook, WebhookProviderTemplate, WorkflowVersion } from '../../shared/api/types'
import { channelInputs, channelSchemas, defaultChannelMappings, initialChannelMappings } from './channel-inputs'
import { channelConfigFields } from './webhook-channel-fields'

const schema = { type: 'object', properties: { prompt: { type: 'string' }, user: { type: 'string' }, context: { type: 'string' }, limit: { type: 'integer' } }, required: ['prompt', 'user'] }
const mapping = (target: string, source = 'message.text') => ({ target, source, missingPolicy: 'error' })
const values = (mappings: unknown[], fixed = {}) => ({ inputMappings: JSON.stringify(mappings), fixedInputs: JSON.stringify(fixed) })
const t = (key: string) => key

describe('channel input contracts', () => {
  it('uses a published workflow version before the first application deployment', () => {
    const version = { versionNumber: 1, definition: { start: { inputs: schema }, end: { outputs: { result: { schema: { type: 'string' } }, metadata: { schema: { type: 'object' } } } } } } as WorkflowVersion
    expect(channelSchemas(undefined, [version])).toEqual({ input: schema, output: { properties: { result: { type: 'string' }, metadata: { type: 'object' } } }, version: 1 })
    expect(channelSchemas(undefined, [])).toBeUndefined()
  })

  it('keeps a deployed contract even when a newer workflow version exists', () => {
    const deployment = { inputSchema: schema, outputSchema: { properties: { result: {} } }, workflowVersionNumber: 1 } as ApplicationDeployment
    const future = { versionNumber: 2, definition: {} } as WorkflowVersion
    expect(channelSchemas(deployment, [future])).toEqual({ input: schema, output: deployment.outputSchema, version: 1 })
  })

  it('creates only declared required inputs and leaves every source for the user to choose', () => {
    expect(defaultChannelMappings(schema)).toEqual([mapping('prompt', ''), mapping('user', '')])
    expect(defaultChannelMappings({ properties: { optional: {} } })).toEqual([])
  })

  it('preserves edited optional mappings and fixed values while exposing uncovered required inputs', () => {
    expect(initialChannelMappings(schema, [mapping('context')], { user: 'Alice' })).toEqual([mapping('context'), mapping('prompt', '')])
  })

  it('rejects missing required inputs and incomplete sources before a request is sent', () => {
    expect(() => channelInputs(values([mapping('prompt')]), schema, t)).toThrow('applications.requiredMappingsMissing')
    expect(() => channelInputs(values([mapping('prompt', ''), mapping('user')]), schema, t)).toThrow('applications.mappingSourceRequired')
    expect(() => channelInputs(values([mapping('prompt', 'raw.'), mapping('user')]), schema, t)).toThrow('applications.mappingSourceRequired')
  })

  it('accepts optional inputs and typed constants without requiring every optional field', () => {
    expect(channelInputs(values([mapping('prompt'), mapping('context', 'conversation.id')], { user: 'Alice', limit: '4' }), schema, t)).toMatchObject({ fixedInputs: { user: 'Alice', limit: 4 } })
  })

  it('rejects unknown targets, duplicate mappings and conflicts with fixed inputs', () => {
    for (const input of [values([mapping('unknown')]), values([mapping('prompt'), mapping('prompt')]), values([mapping('prompt')], { prompt: 'constant' })]) {
      expect(() => channelInputs(input, schema, t)).toThrow('applications.mappingTargetInvalid')
    }
  })
})

describe('channel secret edits', () => {
  const templates = [
    { provider: 'dingtalk', mode: 'callback', fields: [{ key: 'secret', sensitive: true, required: true }] },
    { provider: 'dingtalk', mode: 'stream', fields: [{ key: 'clientSecret', sensitive: true, required: true }] },
    { provider: 'feishu', mode: 'stream', fields: [{ key: 'appSecret', sensitive: true, required: true }] },
  ] as WebhookProviderTemplate[]
  const existing = { providerType: 'dingtalk', channelMode: 'callback', configFields: {} } as ApplicationWebhook

  it('requires credentials for a new channel', () => {
    const fields = channelConfigFields(t, templates).filter((field) => field.name.startsWith('channel_'))
    expect(fields.every((field) => field.required)).toBe(true)
  })

  it('preserves credentials when editing the same provider and mode', () => {
    const secret = channelConfigFields(t, templates, existing).find((field) => field.name === 'channel_secret')!
    expect(secret.required).toBe(false)
    expect(secret.defaultValue).toBe('')
    expect(secret.placeholder).toBe('applications.channelSecretKeep')
  })

  it('requires fresh credentials when switching provider or mode', () => {
    const fields = channelConfigFields(t, templates, existing)
    for (const name of ['channel_clientSecret', 'channel_appSecret']) {
      const secret = fields.find((field) => field.name === name)!
      expect(secret.required).toBe(true)
      expect(secret.placeholder).toBe('')
    }
  })
})
