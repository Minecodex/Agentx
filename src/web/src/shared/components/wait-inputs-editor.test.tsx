import { fireEvent, render, screen, within } from '@testing-library/react'
import { useState } from 'react'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import { i18n } from '../../app/i18n'
import { WaitInputsEditor, type WaitInputs } from './wait-inputs-editor'

beforeEach(async () => { await i18n.changeLanguage('en-US') })

describe('preset workflow replies', () => {
  it('keeps each node sequence independent and preserves non-confirmation text', () => {
    const changed = vi.fn()
    function Form() {
      const [value,setValue] = useState<WaitInputs>({})
      return <WaitInputsEditor nodes={[{ path:'/a',nodeId:'a',name:'First' },{ path:'/child/b',nodeId:'b',name:'Child / Second' }]} value={value} onChange={next=>{ setValue(next);changed(next) }} />
    }
    const { container } = render(<Form />)
    const first = within(container.querySelector('[data-node-path="/a"]') as HTMLElement)
    fireEvent.click(first.getByRole('button',{ name:'Add reply' }))
    fireEvent.change(first.getByRole('textbox'),{ target:{value:'我再考虑一下'} })
    fireEvent.click(first.getByRole('button',{ name:'Add reply' }))
    fireEvent.change(first.getAllByRole('textbox')[1],{ target:{value:'确认'} })
    const child = within(container.querySelector('[data-node-path="/child/b"]') as HTMLElement)
    fireEvent.click(child.getByRole('button',{ name:'Add reply' }))
    fireEvent.change(child.getByRole('textbox'),{ target:{value:'客户的原始消息'} })
    expect(changed).toHaveBeenLastCalledWith({'/a':['我再考虑一下','确认'],'/child/b':['客户的原始消息']})
    fireEvent.click(first.getAllByRole('button',{ name:'Remove reply' })[0])
    expect(first.getByRole('textbox')).toHaveValue('确认')
    expect(changed).toHaveBeenLastCalledWith({'/a':['确认'],'/child/b':['客户的原始消息']})
  })

  it('omits the extra configuration when the workflow has no waits', () => {
    render(<WaitInputsEditor nodes={[]} value={{}} onChange={vi.fn()} />)
    expect(screen.queryByTestId('wait-inputs-editor')).toBeNull()
  })

  it('clears invalid JSON when switching to a different reply plan', () => {
    const valid = vi.fn()
    const nodes = [{ path: '/a', nodeId: 'a', name: 'First' }]
    const { rerender } = render(<WaitInputsEditor nodes={nodes} value={{ '/a': [{}] }} onChange={vi.fn()} onValidityChange={valid} />)
    fireEvent.change(screen.getByRole('textbox'), { target: { value: '{' } })
    expect(valid).toHaveBeenLastCalledWith(false)
    rerender(<WaitInputsEditor nodes={nodes} value={{}} onChange={vi.fn()} onValidityChange={valid} />)
    expect(valid).toHaveBeenLastCalledWith(true)
    expect(screen.queryByRole('textbox')).toBeNull()
  })
})
