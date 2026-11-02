import { expect } from '@playwright/test'
import { existsSync, readFileSync, writeFileSync } from 'node:fs'
import { test, login, send, completed, saved, publishVersion, allEntries, type Created } from '../studio-builder-support'
import { useRuntimePortForward } from './playground-helpers'
import { addGuidance, type Guidance } from '../plan9-authoring-input'

type Blueprint={scene:string;workflowId:string;versionId:string;draftId:string;prompt:string;attemptLimit:number;steeringNodeName?:string}
type Result={scene:string;workflowId:string;passed:boolean;publishedVersionId?:string;attempts:unknown[]}
type SaveRejection={status:number;code?:string;message?:string;fieldErrors?:unknown[]}
const blueprints=JSON.parse(readFileSync(process.env.AGENTX_E2E_PLAN9_WORKFLOW_BLUEPRINTS!,'utf8')) as Blueprint[]
const reportPath=process.env.AGENTX_E2E_PLAN9_WORKFLOW_REPORT!
const results:Result[]=existsSync(reportPath)?JSON.parse(readFileSync(reportPath,'utf8')):[]

for(const blueprint of blueprints)test(`${blueprint.scene}: canvas AI creates the complete business workflow`,async({page},info)=>{
  test.setTimeout(2_100_000)
  const result:Result={scene:blueprint.scene,workflowId:blueprint.workflowId,passed:false,attempts:[]}
  results.push(result)
  const persist=()=>writeFileSync(reportPath,JSON.stringify(results,null,2))
  await useRuntimePortForward(page)
  const token=await login(page)
  let currentToken=Promise.resolve(token)
  page.on('response',response=>{
    if(response.url().endsWith('/api/v1/auth/refresh')&&response.request().method()==='POST'&&response.ok()){
      currentToken=response.json().then((body:{accessToken:string})=>{
        expect(typeof body.accessToken).toBe('string')
        return body.accessToken
      })
      // The evidence reader awaits this same promise and reports any failure.
      void currentToken.catch(()=>{})
    }
  })
  const target:Created={id:blueprint.workflowId,createdVersionId:blueprint.versionId,documentId:blueprint.draftId}
  let saveReply=Promise.resolve<SaveRejection|undefined>(undefined)
  page.on('response',response=>{
    if(!response.url().endsWith(`/workflows/${target.id}/drafts/${target.documentId}`)||response.request().method()!=='PUT')return
    saveReply=response.ok()?Promise.resolve(undefined):response.json().then(body=>({status:response.status(),code:body.code,message:String(body.message??'').slice(0,1600),fieldErrors:Array.isArray(body.fieldErrors)?body.fieldErrors.slice(0,10):undefined})).catch(()=>({status:response.status(),message:'Save failed without a JSON diagnostic'}))
  })
  const editor=`/workflows/${target.id}/versions/${target.createdVersionId}/editor`
  let error=''
  try{
    expect(Number.isInteger(blueprint.attemptLimit)&&blueprint.attemptLimit>=1&&blueprint.attemptLimit<=3,'Canvas authoring budget must include earlier workflow versions').toBe(true)
    for(let attempt=0;attempt<blueprint.attemptLimit;attempt++){
      await page.goto(editor)
      await expect(page.getByTestId('workflow-canvas')).toBeVisible()
      const recoveryDialog=page.getByRole('dialog',{name:'恢复本地修改',exact:true})
      const recoveredLocalCopy=await recoveryDialog.isVisible()
      if(recoveredLocalCopy){
        await recoveryDialog.getByRole('button',{name:'恢复',exact:true}).click()
        await expect(recoveryDialog).not.toBeVisible()
      }
      await page.getByRole('button',{name:'AI 创作',exact:true}).click()
      saveReply=Promise.resolve(undefined)
      const started=Date.now()
      const accepted=page.waitForResponse(response=>/\/design-agent\/sessions\/[^/]+\/runs$/.test(new URL(response.url()).pathname)&&response.request().method()==='POST')
      const recovery=error.includes('outcome_unknown')?' 上一创作 Run 的模型结果未知，保留原 Run、Effect 和费用；这是新的显式草稿修复请求。先读取当前已保存的画布和修订，不重放旧模型 Effect、编辑命令或业务操作。':''
      const run=await send(page,attempt===0?blueprint.prompt:`请修复当前已保存的工作流，先读取实际内容，不假设仍为空白。原业务需求：${blueprint.prompt}。真实创作、静态校验或发布诊断：${error.slice(0,6000)}。只修改画布并静态校验，不运行或发布。${recovery}`)
      const session=new URL((await accepted).url()).pathname.split('/').at(-2)!
      const evidence:{attempt:number;run:string;sessionId:string;entries:unknown[];elapsedMs:number;document?:unknown;error:string|null;collectionError?:string;saveRejection?:SaveRejection;recoveredLocalCopy:boolean;guidance?:Guidance}={attempt,run,sessionId:session,entries:[],elapsedMs:0,error:null,recoveredLocalCopy}
      result.attempts.push(evidence);persist()
      try{
        if(blueprint.steeringNodeName){
          const panel=page.getByTestId('studio-builder-panel')
          evidence.guidance=await addGuidance(page,panel,panel.getByLabel('描述工作流需求'),session,run,`补充本次创作要求：等待交付资料的 await_input 节点名称使用“${blueprint.steeringNodeName}”。保留全部业务输入输出、校验和真实等待，只在原工作流内继续创作。`);persist()
        }
        await completed(page,true,600_000)
        const document=await saved(page,await currentToken,target)
        if(blueprint.steeringNodeName)expect(document.definition.nodes).toContainEqual(expect.objectContaining({type:'await_input',name:blueprint.steeringNodeName}))
        await info.attach(`workflow-attempt${attempt}.json`,{body:JSON.stringify(document),contentType:'application/json'})
        const published=await publishVersion(page,await currentToken,target,target.createdVersionId)
        evidence.document=document
        result.publishedVersionId=published.workflowVersionId;result.passed=true;persist();break
      }catch(failure){
        error=failure instanceof Error?failure.message:String(failure)
        const rejected=await saveReply
        if(rejected){evidence.saveRejection=rejected;error+=`\nActual Save rejection: ${JSON.stringify(rejected)}`}
        evidence.error=error;persist()
        const stopped=await page.request.post(`/api/v1/design-agent/sessions/${session}/runs/${run}/cancel`,{headers:{Authorization:`Bearer ${await currentToken}`}})
        expect(stopped.ok(),await stopped.text()).toBe(true)
      }finally{
        evidence.elapsedMs=Date.now()-started;persist()
        try{evidence.entries=await allEntries(page,await currentToken,session)}
        catch(cause){evidence.collectionError=cause instanceof Error?cause.message:String(cause);throw cause}
        finally{persist()}
      }
    }
    expect(result.passed,error).toBe(true)
  }finally{persist();await info.attach('workflow-creation-result',{body:JSON.stringify(result),contentType:'application/json'})}
})
