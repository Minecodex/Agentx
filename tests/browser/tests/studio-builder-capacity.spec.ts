import { expect } from '@playwright/test'
import { test, derive, editorPath, fixtureCanvas, manualRun, send } from '../studio-builder-support'

test('P8-E2E-20/25: four concurrent model calls are bounded and waiting designs leave business execution available', async ({page,context}) => {
  const {token,created}=await fixtureCanvas(page,'P8 design capacity')
  const canvases=[]
  for(let index=0;index<5;index+=1){
    const draft=await derive(page,created,'',`Capacity ${index}`)
    const canvas=await context.newPage()
    await canvas.goto(editorPath(draft))
    await expect(canvas.getByTestId('workflow-canvas')).toBeVisible()
    await canvas.getByRole('button',{name:'AI 创作',exact:true}).click()
    canvases.push({page:canvas,draft})
  }
  try{
    await Promise.all(canvases.slice(0,4).map((canvas,index)=>send(canvas.page,`P8FIX: slow question capacity_window label=Capacity${index}`)))
    for(const canvas of canvases.slice(0,4))await expect(canvas.page.getByTestId('studio-builder-panel').getByText('Which label should the node use?',{exact:true})).toBeVisible({timeout:60_000})
    const fifth=canvases[4]!
    const panel=fifth.page.getByTestId('studio-builder-panel')
    await panel.getByLabel('描述工作流需求').fill('P8FIX: question label=MustBeRejected')
    const pending=fifth.page.waitForResponse(response=>/\/studio-builder\/sessions\/.+\/runs$/.test(response.url())&&response.request().method()==='POST')
    await panel.getByRole('button',{name:'发送',exact:true}).click()
    const busy=await pending
    expect(busy.status(),await busy.text()).toBe(409)
    expect((await busy.json()).code).toBe('BUILDER_CAPACITY_BUSY')
    const business=await manualRun(fifth.page,token,fifth.draft)
    expect(business.source).toMatchObject({kind:'draft_revision',draftId:fifth.draft.createdDraftId,revision:0})
    for(const canvas of canvases.slice(0,4))await canvas.page.getByTestId('studio-builder-panel').getByRole('button',{name:'停止',exact:true}).click()
  }finally{
    await Promise.all(canvases.map(canvas=>canvas.page.close()))
  }
})
