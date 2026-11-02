import {canonicalDocument} from './document-hash'

/** Explain a rejected round trip without exposing parameter values or changing the document. */
export function documentRepresentationIssue(expected:unknown,actual:unknown):string|null{
  const paths:string[]=[]
  const visit=(left:unknown,right:unknown,path:string)=>{
    if(paths.length>=8||canonicalDocument(left)===canonicalDocument(right))return
    if(Array.isArray(left)&&Array.isArray(right)&&left.length!==right.length){paths.push(path||'/');return}
    if(left&&right&&typeof left==='object'&&typeof right==='object'&&Array.isArray(left)===Array.isArray(right)){
      const a=left as Record<string,unknown>,b=right as Record<string,unknown>
      for(const key of [...new Set([...Object.keys(a),...Object.keys(b)])].sort()){
        visit(a[key],b[key],`${path}/${key.replaceAll('~','~0').replaceAll('/','~1')}`)
        if(paths.length>=8)break
      }
    }else paths.push(path||'/')
  }
  visit(expected,actual,'')
  return paths.length?`Document cannot be represented without changes by this editor at ${paths.join(', ').slice(0,1600)}. Read the current document and use the registered node schema; no changes were applied.`:null
}
