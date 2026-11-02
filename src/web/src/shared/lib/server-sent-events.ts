/** Consume complete SSE data frames with backpressure for durable editor commands. */
export async function consumeServerSentEvents(response: Response, receive:(data:string)=>Promise<void>) {
  const reader=response.body?.getReader()
  if(!reader)throw new Error('Empty event stream')
  const decoder=new TextDecoder()
  let buffer=''
  try {
    for(;;){
      const item=await reader.read()
      if(item.done)break
      buffer=(buffer+decoder.decode(item.value,{stream:true})).replaceAll('\r\n','\n')
      if(buffer.length>8*1024*1024)throw new Error('Event frame exceeds the editor limit')
      let separator:number
      while((separator=buffer.indexOf('\n\n'))>=0){
        const frame=buffer.slice(0,separator);buffer=buffer.slice(separator+2)
        const data=frame.split('\n').filter(line=>line.startsWith('data:')).map(line=>line.slice(5).trimStart()).join('\n')
        if(data)await receive(data)
      }
    }
  } finally {reader.releaseLock()}
}
