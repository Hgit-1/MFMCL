import { expect, test } from '@playwright/test'

test('compatibility findings surface static metadata issues without executing mods', async ({ page }) => {
  await page.addInitScript(() => {
    const state = { inspections: 0 }
    Object.assign(window, { __testState: state, __TAURI_EVENT_PLUGIN_INTERNALS__: { unregisterListener: () => {} }, __TAURI_INTERNALS__: {
      transformCallback: () => 1,
      invoke: async (command: string) => {
        if (command === 'get_settings') return { themeMode: 'dark', themePreset: 'moss', customAccent: '#a6d991', customBackground: null, beginnerMode: true, showGameLog: true, updatePolicy: 'off', download: { source: 'official', maxConcurrentDownloads: 4, connectionsPerFile: 4 } }
        if (command === 'list_versions' || command === 'list_accounts' || command === 'loader_versions') return []
        if (command === 'list_instances') return [{ id: 'demo', name: 'Fabric 实例', gameVersion: '1.21.4', loader: 'fabric@0.16.10', instanceDir: '/i', gameDir: '/i/game', isolated: true, state: 'ready', createdAt: '' }]
        if (command === 'list_mods') return [
          { fileName: 'a.jar', enabled: true, size: 4096 },
          { fileName: 'b.jar', enabled: true, size: 2048 },
        ]
        if (command === 'inspect_mods') {
          state.inspections++
          return [
            { severity: 'error', code: 'missing_dependency', message: 'a.jar 缺少依赖 example-lib。', files: ['a.jar'] },
            { severity: 'warning', code: 'conflicts', message: 'a.jar 与 b.jar 声明冲突。', files: ['a.jar', 'b.jar'] },
          ]
        }
        return 1
      },
    } })
  })
  await page.goto('/')
  await page.getByRole('button', { name: '实例库', exact: true }).click()
  await page.getByRole('button', { name: /管理实例/ }).click()
  await page.getByRole('button', { name: '检查兼容性' }).click()
  await expect(page.getByText('发现 1 个错误级别的兼容性问题。')).toBeVisible()
  await expect(page.getByText('a.jar 缺少依赖 example-lib。')).toBeVisible()
  await expect(page.getByText(/a\.jar 与 b\.jar 声明冲突/)).toBeVisible()
  await expect(page.getByText('涉及：a.jar、b.jar')).toBeVisible()
  expect(await page.evaluate(() => (window as unknown as { __testState: { inspections: number } }).__testState.inspections)).toBe(1)
})
