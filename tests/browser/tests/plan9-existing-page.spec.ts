import {test} from '@playwright/test'
import {writeFileSync} from 'node:fs'
import {executeCase,type Scenario} from '../plan9-page-support'
const input=JSON.parse(process.env.AGENTX_E2E_PLAN9_ENTRANCE_WEB!) as {scenario:Scenario;suffix:string;report:string}
test('The existing AI page continues while the independent MCP entrance is disabled',async({browser},info)=>{
  test.setTimeout(600_000)
  const results:unknown[]=[]
  try{for(let index=0;index<2;index++)results.push(await executeCase(browser,input.scenario,input.suffix,index,info,0))}
  finally{writeFileSync(input.report,JSON.stringify(results,null,2))}
})
