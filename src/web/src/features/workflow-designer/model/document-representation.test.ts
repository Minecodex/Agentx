import {expect,it} from 'vitest'
import {documentRepresentationIssue} from './document-representation'

it('identifies omitted Exit defaults without disclosing mapping values',()=>{
  const parameters={outputs:{receipt:{kind:'literal',value:'PRIVATE_RECEIPT'}}}
  const before={definition:{nodes:[{parameters}]}},after={definition:{nodes:[{parameters:{...parameters,errorOutputs:{}}}]}}
  const issue=documentRepresentationIssue(before,after)!
  expect(issue).toContain('/definition/nodes/0/parameters/errorOutputs')
  expect(issue).not.toContain('PRIVATE_RECEIPT')
  expect(before.definition.nodes[0].parameters).not.toHaveProperty('errorOutputs')
})

it('preserves numeric canonical equality and reports real structural or value changes',()=>{
  expect(documentRepresentationIssue({amount:900},{amount:900.0})).toBeNull()
  expect(documentRepresentationIssue({protected:true},{protected:false})).toContain('/protected')
  expect(documentRepresentationIssue({items:[1]},{items:{0:1}})).toContain('/items')
  expect(documentRepresentationIssue({items:[1]},{items:[1,undefined]})).toContain('/items')
  expect(documentRepresentationIssue({'a/b~c':1},{'a/b~c':2})).toContain('/a~1b~0c')
})

it('bounds diagnostics for a large rejected document',()=>{
  const before=Object.fromEntries(Array.from({length:50},(_,index)=>[`field${index}`,false]))
  const after=Object.fromEntries(Object.keys(before).map(key=>[key,true]))
  expect(documentRepresentationIssue(before,after)!.match(/\/field/g)).toHaveLength(8)
})
