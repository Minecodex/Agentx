import { test } from '@playwright/test'
import { writeFileSync } from 'node:fs'
import { resumeExistingCase, type Scenario } from '../plan9-page-support'

const input=JSON.parse(process.env.AGENTX_E2E_PLAN9_EXISTING_TASK!) as {scenario:Scenario;slug:string;taskId:string}
test('Personal MCP task continues through its generated Web page and actual approval center',async({browser},info)=>{
  test.setTimeout(600_000)
  const result=await resumeExistingCase(browser,input.scenario,input.slug,input.taskId,info)
  writeFileSync(process.env.AGENTX_E2E_PLAN9_EXISTING_TASK_REPORT!,JSON.stringify(result,null,2))
})
