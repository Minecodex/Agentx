import { afterAll, describe, expect, it } from 'vitest'

import { ApiClientError } from '../../shared/api/client'
import { i18n } from './index'
import { resources } from './resources'

function translationKeys(value: object, prefix = ''): string[] {
  return Object.entries(value).flatMap(([key, child]) => {
    const path = prefix ? `${prefix}.${key}` : key
    return child && typeof child === 'object' && !Array.isArray(child)
      ? translationKeys(child, path)
      : [path]
  })
}

function hasTranslationPath(value: object, path: string): boolean {
  let current: unknown = value
  for (const segment of path.split('.')) {
    if (!current || typeof current !== 'object' || !(segment in current)) return false
    current = (current as Record<string, unknown>)[segment]
  }
  return true
}

const sourceModules = import.meta.glob('../../**/*.{ts,tsx}', {
  eager: true,
  import: 'default',
  query: '?raw',
}) as Record<string, string>

const commonAllowedRoots = new Set([
  'search', 'all', 'status', 'actions', 'viewAll', 'create', 'previous', 'next', 'page',
  'noResults', 'noResultsHint', 'comingSoon', 'loading', 'loadFailed', 'active', 'inactive',
  'invited', 'published', 'draft', 'running', 'success', 'failed', 'pending', 'waiting',
  'completed', 'synced', 'syncing', 'untested', 'healthy', 'unhealthy', 'save', 'edit',
  'delete', 'copy', 'confirm', 'cancel', 'open', 'language', 'theme', 'updatedAt', 'owner',
  'name', 'version', 'members', 'description', 'close', 'prerequisites', 'deletion',
  'ready', 'degraded', 'unknown', 'succeeded', 'cancelled', 'queued', 'suspended', 'skipped',
  'timed_out', 'disabled', 'enabled', 'approved', 'rejected', 'claimed', 'unknownValue',
  'created', 'passed', 'error', 'blocked', 'terminated', 'orphaned', 'creating', 'unavailable',
  'reserved', 'schemaForm',
])

const dynamicRoots: Record<string, Set<string>> = {
  applications: new Set(['edit', 'key', 'webhook', 'schedule', 'private', 'department', 'company', 'draft', 'active', 'disabled', 'pinned', 'follow_deployment', 'manual_upgrade', 'fire_once', 'skip', 'webhookEdit', 'scheduleEdit', 'sessionUpgrade', 'upgrade', 'channels', 'addChannel', 'provider', 'providerHint', 'channelMode', 'channelModeCallback', 'channelModeStream', 'channelModeHint', 'channelSecretKeep', 'channelFields', 'connectionStatus', 'connectionPending', 'connectionConnected', 'connectionReconnecting', 'connectionDisconnected', 'connectionError', 'streamNoEndpoint', 'channelEnabled', 'channelDisabled', 'enableChannel', 'disableChannel', 'inputMappings', 'fixedInputs', 'channelDetails', 'productionEndpoint', 'endpointInactive', 'sourceMapping', 'multiConversationHint', 'mappingCount', 'noChannels', 'published', 'sourceField', 'workflowInput', 'removeMapping', 'addMapping', 'fixedInputName', 'fixedInputValue', 'removeFixedInput', 'addFixedInput']),
  datasets: new Set(['private', 'department', 'company', 'active', 'disabled']),
  knowledge: new Set(['healthy', 'unhealthy', 'untested', 'active', 'inactive', 'synced', 'syncing', 'failed', 'pending']),
  memory: new Set(['healthy', 'unhealthy', 'untested', 'active', 'inactive']),
  mcp: new Set(['healthy', 'unhealthy', 'untested', 'active', 'inactive']),
  models: new Set(['healthy', 'unhealthy', 'untested', 'active', 'inactive']),
  studio: new Set(['schemaTypes', 'units', 'formats', 'mergePolicies', 'contextOperations', 'contextScopes', 'errorFields', 'errorStrategies']),
  skills: new Set(['markdownEditor']),
  resourceGrants: new Set(['resourceTypes', 'operations']),
  evaluations: new Set(['aggregationOptions', 'ruleTypes']),
  organization: new Set(['dataScopes']),
  roles: new Set(['names', 'permissionLabels']),
  runtime: new Set(['quota', 'capability', 'capabilityStatus', 'components', 'retentionDataType', 'retentionStatus', 'retentionReasons']),
  approvals: new Set(['resumeStatuses', 'statuses', 'actionTypes', 'auditActions', 'resourceGrantStatuses']),
  executions: new Set(['executionTypes', 'triggerTypes', 'callKinds', 'sideEffects', 'stopReasons']),
  trace: new Set(['overview', 'events', 'raw', 'views', 'boundaries', 'kinds', 'contentKinds']),
}

const markdownEditorKeys = [
  'contentArea.editableMarkdown',
  'toolbar.undo', 'toolbar.redo', 'toolbar.blockTypes.heading', 'toolbar.blockTypes.paragraph',
  'toolbar.blockTypes.quote', 'toolbar.blockTypeSelect.placeholder', 'toolbar.blockTypeSelect.selectBlockTypeTooltip',
  'toolbar.bold', 'toolbar.removeBold', 'toolbar.italic', 'toolbar.removeItalic', 'toolbar.underline',
  'toolbar.removeUnderline', 'toolbar.bulletedList', 'toolbar.numberedList', 'toolbar.checkList',
  'toolbar.link', 'toolbar.table',
  'dialog.close', 'dialogControls.cancel', 'dialogControls.save',
  'createLink.cancelTooltip', 'createLink.saveTooltip', 'createLink.text', 'createLink.textTooltip',
  'createLink.title', 'createLink.titleTooltip', 'createLink.url', 'createLink.urlPlaceholder',
  'linkPreview.copied', 'linkPreview.copyToClipboard', 'linkPreview.edit', 'linkPreview.remove',
  'table.alignCenter', 'table.alignLeft', 'table.alignRight', 'table.columnMenu', 'table.deleteColumn',
  'table.deleteRow', 'table.deleteTable', 'table.insertColumnLeft', 'table.insertColumnRight',
  'table.insertRowAbove', 'table.insertRowBelow', 'table.rowMenu', 'table.textAlignment',
  'codeblock.delete', 'codeBlock.inlineLanguage', 'codeBlock.selectLanguage',
  'imageEditor.deleteImage', 'imageEditor.editImage', 'uploadImage.addViaUrlInstructions',
  'uploadImage.addViaUrlInstructionsNoUpload', 'uploadImage.alt', 'uploadImage.autoCompletePlaceholder',
  'uploadImage.dialogTitle', 'uploadImage.height', 'uploadImage.title', 'uploadImage.uploadInstructions',
  'uploadImage.width',
] as const

const applicationDialogKeys = ['edit', 'deployment', 'key', 'webhook', 'schedule', 'scheduleEdit', 'sessionUpgrade'] as const

describe('locale preference', () => {
  afterAll(async () => {
    await i18n.changeLanguage('zh-CN')
  })

  it('updates the document language and persists a manual selection', async () => {
    await i18n.changeLanguage('en-US')
    expect(document.documentElement.lang).toBe('en-US')
    expect(window.localStorage.getItem('agentx.locale')).toBe('en-US')
    expect(i18n.t('navigation.organization')).toBe('Departments & Users')

    await i18n.changeLanguage('zh-CN')
    expect(document.documentElement.lang).toBe('zh-CN')
    expect(window.localStorage.getItem('agentx.locale')).toBe('zh-CN')
    expect(i18n.t('navigation.organization')).toBe('部门与用户')
  })

  it('localizes API errors from stable error codes', async () => {
    await i18n.changeLanguage('zh-CN')
    expect(new ApiClientError(422, { code: 'INVALID_WORKFLOW_DEFINITION', message: 'Workflow definition is invalid', requestId: 'request-1' }).message).toBe('工作流定义无效')

    await i18n.changeLanguage('en-US')
    expect(new ApiClientError(422, { code: 'INVALID_WORKFLOW_DEFINITION', message: '工作流定义无效', requestId: 'request-1' }).message).toBe('The workflow definition is invalid')
    expect(new ApiClientError(400, { code: 'UNMAPPED_ERROR', message: 'Public validation message', requestId: 'request-2' }).message).toBe('Public validation message')
    expect(new ApiClientError(500, { code: 'INTERNAL_ERROR', message: 'private detail', requestId: 'request-3' }).message).toBe('The request could not be completed. Request ID: request-3')
  })

  it('explains network failures without displaying an empty request ID', async () => {
    const detail = { code: 'NETWORK_ERROR', message: 'Failed to fetch', requestId: '' }
    await i18n.changeLanguage('zh-CN')
    expect(new ApiClientError(0, detail).message).toBe('无法连接服务。请检查网络、服务地址或浏览器的安全提示后重试。')

    await i18n.changeLanguage('en-US')
    expect(new ApiClientError(0, detail).message).toBe('Unable to connect to the service. Check your network, the service address, or browser security warnings, then try again.')
  })

  it('keeps unidentified server failures useful without exposing private details', async () => {
    await i18n.changeLanguage('zh-CN')
    expect(new ApiClientError(503, { code: 'INTERNAL_ERROR', message: 'private detail', requestId: '   ' }).message).toBe('请求暂时无法完成，请稍后重试。')

    await i18n.changeLanguage('en-US')
    expect(new ApiClientError(503, { code: 'INTERNAL_ERROR', message: 'private detail', requestId: '' }).message).toBe('The request could not be completed. Please try again later.')
  })

  it('keeps business-domain translation structures aligned', () => {
    const zh = resources['zh-CN'].translation
    const en = resources['en-US'].translation
    expect(translationKeys(zh).sort()).toEqual(translationKeys(en).sort())
    expect(Object.keys(zh)).not.toEqual(expect.arrayContaining(['m2', 'm3', 'm4', 'm5', 'm7', 'pages']))
  })

  it('keeps model and skill identity fields semantically isolated', () => {
    expect(resources['zh-CN'].translation.models.fields.modelName).toBe('模型名称')
    expect(resources['zh-CN'].translation.models.fields.upstreamModelId).toBe('上游模型 ID')
    expect(resources['zh-CN'].translation.models.untested).toBe('未测试')
    expect(resources['zh-CN'].translation.skills.fields.alias).toBe('技能别名')
    expect(resources['en-US'].translation.models.fields.modelName).toBe('Model name')
    expect(resources['en-US'].translation.models.untested).toBe('Untested')
    expect(resources['en-US'].translation.skills.fields.alias).toBe('Skill alias')
  })

  it('enforces common vocabulary and prevents copied cross-domain roots', () => {
    for (const key of Object.keys(resources['zh-CN'].translation.common)) expect(commonAllowedRoots.has(key)).toBe(true)
    for (const key of Object.keys(resources['en-US'].translation.common)) expect(commonAllowedRoots.has(key)).toBe(true)
    const forbiddenByDomain: Record<string, string[]> = {
      skills: ['modelName', 'upstreamModelId', 'createModel', 'credentialType', 'providerType', 'createCredential', 'createMcp', 'createKnowledge', 'createMemory'],
      credentials: ['modelName', 'upstreamModelId', 'createModel', 'createMcp', 'createKnowledge', 'createMemory'],
      models: ['createCredential', 'createMcp', 'createKnowledge', 'createMemory'],
    }
    for (const locale of ['zh-CN', 'en-US'] as const) {
      const translation = resources[locale].translation as Record<string, Record<string, unknown>>
      for (const domain of Object.keys(translation)) {
        if (domain === 'common' || domain === 'errors') continue
        for (const root of forbiddenByDomain[domain] ?? []) expect(Object.prototype.hasOwnProperty.call(translation[domain], root)).toBe(false)
      }
    }
  })

  it('does not retain translation roots with no owning call site', () => {
    const used = new Map<string, Set<string>>()
    for (const source of Object.values(sourceModules)) {
      for (const match of source.matchAll(/\b(?:t|i18n\.t)\(\s*(['"])([^'"$]+)\1/g)) {
        const [domain, root] = match[2].split('.')
        if (!used.has(domain)) used.set(domain, new Set())
        if (root) used.get(domain)?.add(root)
      }
      for (const match of source.matchAll(/['"]((?:common|navigation|auth|workflows|studio|applications|executions|approvals|notifications|datasets|evaluations|runtime|trace|credentials|models|mcp|skills|knowledge|memory|sandbox|resourceGrants|organization|roles|errors)\.[A-Za-z0-9_.-]+)['"]/g)) {
        const [domain, root] = match[1].split('.')
        if (!used.has(domain)) used.set(domain, new Set())
        if (root) used.get(domain)?.add(root)
      }
    }
    for (const locale of ['zh-CN', 'en-US'] as const) {
      const translation = resources[locale].translation as Record<string, Record<string, unknown>>
      for (const [domain, values] of Object.entries(translation)) {
        if (domain === 'common' || domain === 'errors' || domain === 'notifications') continue
        for (const root of Object.keys(values)) expect(used.get(domain)?.has(root) || dynamicRoots[domain]?.has(root), `${domain}.${root}`).toBe(true)
      }
    }
  })

  it('resolves every statically named translation used by the frontend', () => {
    const usedKeys = new Set<string>()
    for (const source of Object.values(sourceModules)) {
      for (const match of source.matchAll(/\bt\(\s*(['"])([^'"$]+)\1/g)) usedKeys.add(match[2])
    }

    const missing = [...usedKeys].filter((key) =>
      !hasTranslationPath(resources['zh-CN'].translation, key)
      || !hasTranslationPath(resources['en-US'].translation, key),
    )
    expect(missing.sort()).toEqual([])
  })

  it('localizes every control exposed by the configured Markdown editor plugins', () => {
    for (const locale of ['zh-CN', 'en-US'] as const) {
      const translation = resources[locale].translation
      for (const key of markdownEditorKeys) {
        expect(hasTranslationPath(translation, `skills.markdownEditor.${key}`), `${locale}: ${key}`).toBe(true)
      }
    }
  })

  it('localizes every dynamically selected application dialog title', () => {
    for (const locale of ['zh-CN', 'en-US'] as const) {
      const translation = resources[locale].translation
      for (const key of applicationDialogKeys) {
        expect(hasTranslationPath(translation, `applications.${key}`), `${locale}: ${key}`).toBe(true)
      }
    }
  })

  it('does not retain English business terminology in zh-CN copy', () => {
    const forbidden = /\b(?:Workflow|Skill|Memory|Agent|Execution|Revision|Case|Environment)\b|Service Identity|Dataset Version|Iteration Ledger/
    for (const [key, value] of Object.entries(resources['zh-CN'].translation)) {
      for (const leaf of translationKeys({ [key]: value })) {
        const segments = leaf.split('.')
        let current: unknown = resources['zh-CN'].translation
        for (const segment of segments) current = (current as Record<string, unknown>)[segment]
        if (typeof current === 'string') expect(current, leaf).not.toMatch(forbidden)
      }
    }
  })

  it('forbids static English translation fallbacks and locale-implicit formatting', () => {
    const staticFallback = /\bt\(\s*(['"])([^'"$]+)\1\s*,\s*(['"])[\s\S]*?\3\s*\)/
    for (const [path, source] of Object.entries(sourceModules)) {
      expect(source, path).not.toMatch(staticFallback)
      if (!path.endsWith('/shared/lib/locale-format.ts')) {
        expect(source, path).not.toMatch(/\.(?:toLocaleString|toLocaleTimeString)\(\s*\)/)
      }
    }
  })

  it('forbids audited hardcoded business copy in JSX text and accessibility attributes', () => {
    const businessTerm = '(?:Workflow|Skill|Memory|Agent|Execution|Revision|Case|Environment|Service Identity)'
    const jsxText = new RegExp(`>[^<>{}]*\\b${businessTerm}\\b[^<>{}]*<`)
    const attribute = new RegExp(`(?:aria-label|title|placeholder)=["'][^"']*\\b${businessTerm}\\b[^"']*["']`)
    for (const [path, source] of Object.entries(sourceModules)) {
      if (path.includes('/app/i18n/')) continue
      expect(source, path).not.toMatch(jsxText)
      expect(source, path).not.toMatch(attribute)
    }
  })
})
