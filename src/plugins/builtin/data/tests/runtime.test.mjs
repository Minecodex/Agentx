import assert from 'node:assert/strict'
import test from 'node:test'
import { execute, resolveDefinition } from '../dist/runtime.js'

const execution = (nodeType) => ({ nodeType, executionId: 'e', nodeExecutionId: 'n', attemptId: 'a', runIndex: 0, iterationIndex: 0, idempotencyKey: 'i', deadline: new Date(Date.now() + 1000).toISOString() })

test('Set preserves item metadata and applies per-item values', async () => {
  const result = await execute({ execution: execution('set'), inputs: { main: [{ json: { id: 1 }, metadata: { source: 'test' } }] }, parameters: {}, perItemParameters: [{ values: { state: 'ready' } }] })
  assert.deepEqual(result.outputs.main, [{ json: { id: 1, state: 'ready' }, metadata: { source: 'test' } }])
})

test('List filters, sorts, and takes in order', async () => {
  const result = await execute({ execution: execution('list'), inputs: {}, parameters: { input: [{ id: 1 }, { id: 9 }, { id: 7 }], sort: [{ direction: 'desc', nulls: 'last' }], takeN: 2 }, perItemParameters: [{ filter: { conditions: [{ condition: true }] }, sort: [{ selector: 1 }] }, { filter: { conditions: [{ condition: true }] }, sort: [{ selector: 9 }] }, { filter: { conditions: [{ condition: true }] }, sort: [{ selector: 7 }] }] })
  assert.deepEqual(result.outputs.main[0].json.items, [{ id: 9 }, { id: 7 }])
})

test('Set and List resolve their output contracts inside the TypeScript package', async () => {
  const upstream = {
    main: { schema: { type: 'object', properties: { id: { type: 'integer' } }, required: ['id'], additionalProperties: false } },
    $inputs: { type: 'object' },
    $nodes: {},
  }
  const set = await resolveDefinition({ keepOnlySet: false, values: { kind: 'object', fields: { state: { kind: 'literal', value: 'ready' } } } }, upstream, { nodeType: 'set' })
  assert.deepEqual(set.outputSchema.required.sort(), ['id', 'state'])
  assert.equal(set.outputSchema.properties.state.type, 'string')
  const list = await resolveDefinition({ input: { kind: 'literal', value: [{ id: 1 }] } }, upstream, { nodeType: 'list' })
  assert.equal(list.outputSchema.properties.items.type, 'array')
})


test('List empty input and no matching rows are successful empty results', async () => {
  for (const input of [[], [{ id: 1 }]]) {
    const result = await execute({ execution: execution('list'), inputs: {}, parameters: { input }, perItemParameters: input.map(() => ({ filter: { conditions: [{ condition: false }] } })) })
    assert.equal(result.status, 'completed')
    assert.deepEqual(result.outputs.main[0].json.items, [])
  }
})

test('Set accepts no incoming items without raising a node error', async () => {
  const result = await execute({ execution: execution('set'), inputs: { main: [] }, parameters: { values: {} }, perItemParameters: [] })
  assert.equal(result.status, 'completed')
})
