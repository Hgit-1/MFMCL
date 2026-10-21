import { expect, test } from '@playwright/test'

test('installation controls invoke the backend and cancelled tasks can be retried', async ({ page }) => {
  await page.addInitScript(() => {
    const callbacks: Record<number, (event: unknown) => void> = {}
    const listeners: Record<string, number> = {}
    const state = { actions: [] as string[], installs: 0 }
    const task = { id: 'task', instanceId: 'demo', label: 'Minecraft install', state: 'running', completedBytes: 128, totalBytes: 1024, source: 'https://resources.download.minecraft.net/test', updatedAt: '' }
    let callbackId = 0
    Object.assign(window, { __testState: state, __TAURI_EVENT_PLUGIN_INTERNALS__: { unregisterListener: () => {} }, __TAURI_INTERNALS__: {
      transformCallback: (callback: (event: unknown) => void) => { callbacks[++callbackId] = callback; return callbackId },
      invoke: async (command: string, args: Record<string, unknown>) => {
        if (command === 'plugin:event|listen') { listeners[args.event as string] = args.handler as number; return 1 }
        if (command === 'get_settings') return { themeMode: 'dark', themePreset: 'moss', customAccent: '#a6d991', customBackground: null, beginnerMode: true, showGameLog: true, download: { source: 'official', maxConcurrentDownloads: 4, connectionsPerFile: 4 } }
        if (command === 'list_download_tasks') return [task]
        if (command === 'list_versions') return []
        if (command === 'list_accounts') return []
        if (command === 'list_instances') return [{ id: 'demo', name: 'Demo', gameVersion: '1.21.4', state: 'failed', loader: 'vanilla' }]
        if (command === 'control_download') {
          state.actions.push(args.action as string)
          task.state = args.action === 'pause' ? 'paused' : args.action === 'resume' ? 'running' : 'cancelled'
          callbacks[listeners['installation-task']]?.({ event: 'installation-task', id: 1, payload: { instanceId: 'demo', task: { ...task } } })
        }
        if (command === 'install_instance') state.installs++
        return 1
      },
    } })
  })
  await page.goto('/')
  await page.getByRole('button', { name: /下载任务/ }).click()
  const history = page.getByRole('region', { name: '安装任务与历史记录' })
  await expect(history.getByText('Minecraft install')).toBeVisible()
  await history.getByRole('button', { name: '暂停', exact: true }).click()
  await expect(history.getByText('已暂停 · 等待所有步骤完成后才可启动')).toBeVisible()
  await history.getByRole('button', { name: '继续', exact: true }).click()
  await expect(history.getByRole('button', { name: '暂停', exact: true })).toBeVisible()
  page.once('dialog', dialog => dialog.accept())
  await history.getByRole('button', { name: '取消', exact: true }).click()
  await expect(history.getByText('已取消', { exact: true })).toBeVisible()
  await history.getByRole('button', { name: '重试安装', exact: true }).click()
  await expect.poll(() => page.evaluate(() => (window as unknown as { __testState: { installs: number } }).__testState.installs)).toBe(1)
  expect(await page.evaluate(() => (window as unknown as { __testState: { actions: string[] } }).__testState.actions)).toEqual(['pause', 'resume', 'cancel'])
})
