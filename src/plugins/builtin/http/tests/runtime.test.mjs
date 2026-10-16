import assert from 'node:assert/strict'
import test from 'node:test'
import { execute } from '../dist/runtime.js'

test('HTTP delegates the resolved request to the host with an idempotency key', async () => {
  let request
  const result = await execute({ parameters: { method: 'POST', url: 'https://example.test', body: { ok: true } }, execution: { idempotencyKey: 'attempt:1' }, http: async (value) => { request = value; return { status: 200, body: { accepted: true } } } })
  assert.equal(request.idempotencyKey, 'attempt:1')
  assert.deepEqual(result.outputs.main[0].json.body, { accepted: true })
})


test('HTTP successful empty bodies are preserved', async () => {
  for (const [status, body] of [[204, null], [200, ''], [200, []]]) {
    const result = await execute({ inputs: {}, parameters: { method: 'GET', url: 'https://example.test' }, execution: { idempotencyKey: 'empty' }, http: async () => ({ status, body }) })
    assert.equal(result.status, 'completed')
    assert.deepEqual(result.outputs.main[0].json.body, body)
  }
})
