import {render,screen} from '@testing-library/react'
import {expect,it,vi} from 'vitest'
import {i18n} from '../../app/i18n'
import {DesignAgentEntry} from './design-agent-entry'

it('keeps browser evidence, failed checks and stale drafts separate from a completed model turn',async()=>{
  await i18n.changeLanguage('en-US')
  const payload={type:'message_added',message:{role:'tool_result',isError:true,content:JSON.stringify({source:'isolated_browser',documentHash:'old',image:{width:1440,height:900},report:{errors:['A visible result was cleared'],actions:[]}})}}
  const {rerender}=render(<DesignAgentEntry payload={payload} currentHash="old" answered={false} answer={vi.fn()}/>)
  expect(screen.getByText('Browser checks found issues')).toBeVisible()
  expect(screen.getByText('A visible result was cleared')).toBeVisible()
  expect(screen.queryByText(/passed/i)).toBeNull()
  rerender(<DesignAgentEntry payload={payload} currentHash="new" answered={false} answer={vi.fn()}/>)
  expect(screen.getByText('The draft changed; this observation is outdated')).toBeVisible()
})
