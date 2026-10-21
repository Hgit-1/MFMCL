import { defineConfig } from '@playwright/test'
export default defineConfig({
  testDir: './tests', fullyParallel: true,
  use: { baseURL: 'http://127.0.0.1:1420', viewport: { width: 1280, height: 820 }, screenshot: 'only-on-failure', trace: 'retain-on-failure' },
  webServer: { command: 'npm run dev -- --host 127.0.0.1 --port 1420 --strictPort', url: 'http://127.0.0.1:1420', reuseExistingServer: !process.env.CI },
})
