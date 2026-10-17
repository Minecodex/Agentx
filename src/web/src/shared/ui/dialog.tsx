import * as DialogPrimitive from '@radix-ui/react-dialog'
import { createContext, useContext, useState, type ComponentProps } from 'react'

import { cn } from '../lib/cn'

export const Dialog = DialogPrimitive.Root
export const DialogTrigger = DialogPrimitive.Trigger
export const DialogClose = DialogPrimitive.Close

const DialogLayerContext = createContext<HTMLElement | null>(null)

export function useDialogLayer() {
  return useContext(DialogLayerContext)
}

type DialogContentProps = ComponentProps<typeof DialogPrimitive.Content> & { title: string; description?: string }

export function DialogContent({ className, children, title, description, onEscapeKeyDown, ...props }: DialogContentProps) {
  const [layer, setLayer] = useState<HTMLElement | null>(null)

  return (
    <DialogPrimitive.Portal>
      <DialogPrimitive.Overlay className="fixed inset-0 z-[200] bg-background/70 backdrop-blur-sm" />
      <DialogPrimitive.Content className={cn('fixed left-1/2 top-1/2 z-[210] w-[min(560px,calc(100vw-48px))] -translate-x-1/2 -translate-y-1/2 overflow-visible rounded-lg border border-border bg-surface p-0 text-foreground shadow-2xl outline-none', className)} onEscapeKeyDown={(event) => {
        if (layer?.querySelector('[role="combobox"][aria-expanded="true"]')) {
          event.preventDefault()
          return
        }
        onEscapeKeyDown?.(event)
      }} ref={setLayer} {...props}>
        <DialogPrimitive.Title className="sr-only">{title}</DialogPrimitive.Title>
        {description && <DialogPrimitive.Description className="sr-only">{description}</DialogPrimitive.Description>}
        <DialogLayerContext.Provider value={layer}>
          <div className="max-h-[calc(100dvh-32px)] overflow-y-auto rounded-[inherit]">{children}</div>
        </DialogLayerContext.Provider>
      </DialogPrimitive.Content>
    </DialogPrimitive.Portal>
  )
}
