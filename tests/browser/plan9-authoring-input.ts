import { expect, type Locator, type Page } from '@playwright/test'

export type Guidance = {runId:string;messageId:string;editorInstanceId:string;kind:'steering';text:string;replayed:boolean}

/** Normal creator UI input, scoped to the existing Run; no HTML or graph edits. */
export async function addGuidance(page:Page,panel:Locator,input:Locator,session:string,run:string,text:string):Promise<Guidance>{
  const path=`/api/v1/design-agent/sessions/${session}/runs/${run}/inputs`
  await input.fill(text)
  const action=panel.getByRole('button',{name:'追加指导',exact:true})
  await expect(action).toBeEnabled()
  const [response]=await Promise.all([
    page.waitForResponse(response=>new URL(response.url()).pathname===path&&response.request().method()==='POST'),
    action.click(),
  ])
  expect(response.ok(),await response.text()).toBe(true)
  const receipt=await response.json(),request=response.request().postDataJSON()
  expect(receipt).toMatchObject({accepted:true,runId:run,messageId:request.messageId})
  expect(request).toMatchObject({kind:'steering',text})
  expect(request.editorInstanceId).toMatch(/^[\da-f-]{36}$/)
  await expect(panel.locator(`[data-design-input-id="${receipt.messageId}"]`)).toContainText(text)
  return {...request,runId:run,replayed:receipt.replayed}
}
