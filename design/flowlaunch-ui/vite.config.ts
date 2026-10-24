import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'

export default defineConfig({
  plugins: [react()],
  server: {
    allowedHosts: ['temp.mdfzminecraft.top'],
  },
  resolve: {
    alias: {
      '@': '/src',
      '@components': '/components',
      '@styles': '/styles',
      '@hooks': '/hooks',
      '@utils': '/utils'
    }
  }
})
