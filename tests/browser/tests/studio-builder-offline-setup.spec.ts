import { expect } from '@playwright/test'
import { writeFile } from 'node:fs/promises'
import { fixtureDraftId } from '../draft-fixtures'
import { test, create, createVersion, login, publishVersion } from '../studio-builder-support'

test('user publishes Workflow and business Agent versions before Control-offline verification', async ({page}) => {
  const token = await login(page)
  const created = await create(page, `P8 offline Workflow ${Date.now()}`)
  const version = await createVersion(page, created)
  await publishVersion(page, token, created, version.id)
  const agentId = process.env.AGENTX_E2E_OFFLINE_AGENT_WORKFLOW!
  const agentVersion = process.env.AGENTX_E2E_OFFLINE_AGENT_VERSION!
  expect(agentId).toBeTruthy()
  expect(agentVersion).toBeTruthy()
  await publishVersion(page, token, {id:agentId,createdDraftId:await fixtureDraftId(page,agentId)}, agentVersion)
  await writeFile(process.env.AGENTX_E2E_OFFLINE_SETUP_OUTPUT!, JSON.stringify({workflowId:created.id,versionId:version.id,agentWorkflowId:agentId,agentVersionId:agentVersion},null,2))
})
