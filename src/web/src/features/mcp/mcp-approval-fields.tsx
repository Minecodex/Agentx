import type { TFunction } from 'i18next'
import { apiRequest } from '../../shared/api/client'
import type { McpTool } from '../../shared/api/types'
import type { EntityFormField } from '../../shared/components/entity-form-dialog'
import { SearchableMultiSelect, type MultiSelectOption } from '../../shared/components/searchable-multi-select'

export function approvalPolicyFields(t:TFunction,tool:McpTool):EntityFormField[]{
  const rule=tool.approvalRule
  return [
    {name:'approvalMode',label:t('applicationMcp.toolApproval'),type:'select',defaultValue:rule?'required':'none',options:[{value:'none',label:t('applicationMcp.approvalOff')},{value:'required',label:t('applicationMcp.approvalOn')}],description:t('applicationMcp.protectedHint')},
    ...(['user','role'] as const).map(kind=>({name:`approval_${kind}`,label:t(kind==='user'?'applicationMcp.approverUsers':'applicationMcp.approverRoles'),defaultValue:JSON.stringify(kind==='user'?rule?.userIds??[]:rule?.roleIds??[]),visible:(values:Record<string,string>)=>values.approvalMode==='required',render:({value,update}:{value:string;update:(value:string)=>void})=><SearchableMultiSelect label={t(kind==='user'?'applicationMcp.approverUsers':'applicationMcp.approverRoles')} queryKey={`mcp-approver-${kind}`} value={JSON.parse(value||'[]') as string[]} onChange={values=>update(JSON.stringify(values))} loadOptions={search=>apiRequest<MultiSelectOption[]>(`/mcp/approval-candidates?kind=${kind}&search=${encodeURIComponent(search)}`)}/>})),
    {name:'approvalTimeout',label:t('applicationMcp.approvalTimeout'),type:'number',defaultValue:String(rule?.timeoutSeconds??3600),min:60,max:86400,required:true,visible:values=>values.approvalMode==='required'},
  ]
}
