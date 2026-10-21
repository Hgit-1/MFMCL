import { expect, test } from '@playwright/test'

test('every offline launch attempt asks for confirmation, cancellation never invokes native launch', async ({ page }) => {
  await page.addInitScript(() => {
    const state = { launches: 0 }
    Object.assign(window, { __TAURI_EVENT_PLUGIN_INTERNALS__: { unregisterListener: () => {} }, __testState: state, __TAURI_INTERNALS__: {
      transformCallback: () => 1,
      invoke: async (command: string) => {
        if (command === 'get_settings') return { themeMode: 'dark', themePreset: 'moss', customAccent: '#a6d991', customBackground: null, beginnerMode: true, showGameLog: true, download: { source: 'official', maxConcurrentDownloads: 4, connectionsPerFile: 4 } }
        if (command === 'list_versions') return [{ id: '1.21.4', type: 'release', releaseTime: '2024-12-03' }]
        if (command === 'list_instances') return [{ id: 'demo', name: 'Test instance', gameVersion: '1.21.4', loader: 'vanilla', instanceDir: '/instance', gameDir: '/instance/game', isolated: true, state: 'ready', createdAt: '' }]
        if (command === 'list_accounts') return [{ id: 'offline', displayName: 'Alex', kind: 'offline', selected: true, createdAt: '' }]
        if (command === 'launch_instance') { state.launches++; return }
        if (command === 'detect_java') return { path: '/java', majorVersion: 21, versionText: 'test Java' }
        return 1
      },
    } })
  })
  await page.goto('/')
  await expect(page.getByRole('button', { name: '启动游戏', exact: true })).toBeVisible()
  let confirmations = 0
  page.on('dialog', async dialog => {
    confirmations++
    expect(dialog.message()).toContain('离线')
    if (confirmations === 1) await dialog.dismiss()
    else await dialog.accept()
  })
  const launch = page.getByRole('button', { name: '启动游戏', exact: true })
  await launch.click()
  expect(await page.evaluate(() => (window as unknown as { __testState: { launches: number } }).__testState.launches)).toBe(0)
  await launch.click()
  await expect.poll(() => page.evaluate(() => (window as unknown as { __testState: { launches: number } }).__testState.launches)).toBe(1)
  await launch.click()
  await expect.poll(() => page.evaluate(() => (window as unknown as { __testState: { launches: number } }).__testState.launches)).toBe(2)
  expect(confirmations).toBe(3)
})
