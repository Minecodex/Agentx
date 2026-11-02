import { expect, test, type Page, type TestInfo } from '@playwright/test'
import { readFileSync } from 'node:fs'
import { downloadPageFile } from '../plan9-page-support'

async function validDownload(page:Page,info:TestInfo){
  page.setDefaultTimeout(2_000)
  await page.setContent('<a data-action="download-file" href="data:text/plain,Plan9%20synthetic%20download" download="report.txt">Download</a>')
  const file=await downloadPageFile(page,page.mainFrame()),path=info.outputPath('synthetic-report.txt')
  await file.saveAs(path)
  expect(readFileSync(path,'utf8')).toBe('Plan9 synthetic download')
}

test('a missing stable download action is a repairable error without an unhandled event timeout',async({page},info)=>{
  page.setDefaultTimeout(150)
  await page.setContent('<button>Unmarked download</button>')
  await expect(downloadPageFile(page,page.mainFrame())).rejects.toThrow(/data-action="download-file"/)
  // Tiny synthetic fixtures exercise the harness only; no generated HTML or business task is changed.
  await validDownload(page,info)
})

test('a marked action that produces no download fails normally and a later real download still works',async({page},info)=>{
  page.setDefaultTimeout(150)
  await page.setContent('<button data-action="download-file">No download handler</button>')
  await expect(downloadPageFile(page,page.mainFrame())).rejects.toThrow(/download/)
  await validDownload(page,info)
})
