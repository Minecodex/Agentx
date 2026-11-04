import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
import tailwindcss from '@tailwindcss/vite'

export default defineConfig({
  plugins: [react(), tailwindcss()],
  define: { 'process.env.NODE_ENV': JSON.stringify('production') },
  build: {
    outDir: '../../.local/dist/design-preview',
    lib: { entry: 'src/design-preview/main.tsx', name: 'AgentxDesignPreview', formats: ['iife'], fileName: () => 'preview.js', cssFileName: 'preview' },
  },
})
