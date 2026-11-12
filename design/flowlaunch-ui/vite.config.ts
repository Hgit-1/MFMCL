import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
import { distributionPolicy } from './distribution-policy.mjs'

export default defineConfig({
  plugins: [react(), distributionPolicy()],
  build: {
    copyPublicDir: false,
  },
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
