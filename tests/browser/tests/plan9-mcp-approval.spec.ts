import { test, expect } from '@playwright/test'
import { writeFileSync } from 'node:fs'
import { Reviewers, type Decision, type Scenario } from '../plan9-page-support'

test('MCP task receives a real supervisor decision in the approval center', async ({ browser }, info) => {
  const request = JSON.parse(process.env.AGENTX_E2E_PLAN9_APPROVAL!) as {id:string;decision:Decision;scenario:Scenario}
  const reviewers = new Reviewers(browser, info)
  try {
    const evidence = await reviewers.decide(request.id, request.decision, 0, request.scenario)
    expect(evidence.approvalId).toBe(request.id)
    writeFileSync(process.env.AGENTX_E2E_PLAN9_APPROVAL_REPORT!, JSON.stringify(evidence, null, 2))
  } finally {
    await reviewers.close()
  }
})
