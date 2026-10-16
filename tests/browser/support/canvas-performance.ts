import { expect, type Locator, type Page } from '@playwright/test'

// Observe browser frames and input delivery; never read or inject Editor state.
export async function sampleCanvasMotion(page: Page, node: Locator) {
  await expect(node).toBeVisible()
  const box = await node.boundingBox()
  expect(box).toBeTruthy()
  await page.evaluate(async () => {
    await document.fonts.ready
    await new Promise<void>(resolve => requestAnimationFrame(() => requestAnimationFrame(() => resolve())))
  })
  await page.evaluate(() => {
    const target = window as Window & {__p8Motion?:{frames:number[];delays:number[]}}
    const samples = {frames:[] as number[],delays:[] as number[]}
    target.__p8Motion = samples
    let previous = performance.now()
    const end = previous + 1800
    const frame = (now:number) => {
      samples.frames.push(now-previous)
      previous = now
      if (now<end) requestAnimationFrame(frame)
      else document.removeEventListener('pointermove',input)
    }
    const input = () => {
      const at = performance.now()
      requestAnimationFrame(()=>samples.delays.push(performance.now()-at))
    }
    document.addEventListener('pointermove',input,{passive:true})
    requestAnimationFrame(frame)
  })
  await page.mouse.move(box!.x+box!.width/2,box!.y+box!.height/2)
  await page.mouse.down()
  for (let index=0;index<40;index+=1) {
    await page.mouse.move(box!.x+box!.width/2+index,box!.y+box!.height/2+Math.sin(index/4)*15)
    await page.waitForTimeout(12)
  }
  await page.mouse.up()
  await page.waitForTimeout(1900)
  const samples = await page.evaluate(()=>(window as Window & {__p8Motion?:{frames:number[];delays:number[]}}).__p8Motion!)
  const percentile = (values:number[],ratio:number) => {
    expect(values.length).toBeGreaterThan(0)
    const ordered=[...values].sort((a,b)=>a-b)
    return ordered[Math.min(ordered.length-1,Math.ceil(ordered.length*ratio)-1)]!
  }
  const metric={medianFps:1000/percentile(samples.frames,.5),p95InputDelayMs:percentile(samples.delays,.95)}
  return metric
}
