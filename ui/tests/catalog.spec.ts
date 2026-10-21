import { expect, test } from '@playwright/test'

test('Modrinth catalogue search discloses data sharing and installs verified versions', async ({ page }) => {
  await page.addInitScript(() => {
    const state = { searches: [] as string[], installs: [] as string[], deletions: [] as boolean[], listed: 0 }
    Object.assign(window, { __testState: state, __TAURI_EVENT_PLUGIN_INTERNALS__: { unregisterListener: () => {} }, __TAURI_INTERNALS__: {
      transformCallback: () => 1,
      invoke: async (command: string, args: Record<string, unknown>) => {
        if (command === 'get_settings') return { themeMode: 'dark', themePreset: 'moss', customAccent: '#a6d991', customBackground: null, beginnerMode: true, showGameLog: true, updatePolicy: 'off', download: { source: 'official', maxConcurrentDownloads: 4, connectionsPerFile: 4 } }
        if (command === 'list_versions') return []
        if (command === 'list_accounts') return []
        if (command === 'list_instances') return state.deletions.length ? [] : [{ id: 'demo', name: 'Fabric 实例', gameVersion: '1.21.4', loader: 'fabric@0.16.10', instanceDir: '/data/instances/demo', gameDir: '/data/instances/demo/game', isolated: true, state: 'ready', createdAt: '' }]
        if (command === 'loader_versions') return []
        if (command === 'list_mods') { state.listed++; return state.installs.length ? [{ fileName: 'sodium-0.6.0.jar', enabled: true, size: 4096 }] : [] }
        if (command === 'search_mods') { state.searches.push(args.query as string); return [
          { projectId: 'p1', title: 'Sodium', description: '渲染优化', author: '作者', downloads: 123456, follows: 789, iconUrl: null },
        ] }
        if (command === 'list_mod_versions') return [
          { versionId: 'v2', versionNumber: '0.6.0', dateCreated: '2026-06-01T00:00:00Z', fileName: 'sodium-0.6.0.jar', url: 'https://cdn.modrinth.com/sodium.jar', sha1: 'a'.repeat(40), size: 4194304 },
        ]
        if (command === 'install_mod_version') { state.installs.push((args.version as { fileName: string }).fileName); return (args.version as { fileName: string }).fileName }
        if (command === 'delete_instance') { state.deletions.push(args.deleteFiles as boolean); return }
        return 1
      },
    } })
  })
  await page.goto('/')
  await page.getByRole('button', { name: '实例库', exact: true }).click()
  await page.getByRole('button', { name: /管理实例/ }).click()
  // Disclosure precedes any query.
  await expect(page.getByText(/搜索词与版本条件会发送给 Modrinth/)).toBeVisible()
  await page.getByLabel('搜索 Modrinth 模组').fill('sodium')
  await page.getByRole('button', { name: '搜索', exact: true }).click()
  await expect(page.getByText('Sodium')).toBeVisible()
  await expect(page.getByText(/123,456 次下载/)).toBeVisible()
  expect(await page.evaluate(() => (window as unknown as { __testState: { searches: string[] } }).__testState.searches)).toEqual(['sodium'])
  await page.getByRole('button', { name: '查看版本' }).click()
  await expect(page.getByText('选择版本 · Sodium')).toBeVisible()
  await expect(page.getByText(/不会覆盖已有文件/)).toBeVisible()
  // Cancel the trust confirmation: nothing is installed.
  page.once('dialog', dialog => dialog.dismiss())
  await page.getByRole('button', { name: '安装', exact: true }).click()
  expect(await page.evaluate(() => (window as unknown as { __testState: { installs: string[] } }).__testState.installs)).toEqual([])
  // Confirm: verified install lands and the mod list refreshes.
  page.once('dialog', dialog => { expect(dialog.message()).toContain('用户权限'); void dialog.accept() })
  await page.getByRole('button', { name: '安装', exact: true }).click()
  await expect(page.getByText('已安装并校验 sodium-0.6.0.jar。')).toBeVisible()
  expect(await page.evaluate(() => (window as unknown as { __testState: { installs: string[] } }).__testState.installs)).toEqual(['sodium-0.6.0.jar'])
})

test('instance deletion requires explicit confirmation and reports what was removed', async ({ page }) => {
  await page.addInitScript(() => {
    const state = { deletions: [] as boolean[] }
    Object.assign(window, { __testState: state, __TAURI_EVENT_PLUGIN_INTERNALS__: { unregisterListener: () => {} }, __TAURI_INTERNALS__: {
      transformCallback: () => 1,
      invoke: async (command: string, args: Record<string, unknown>) => {
        if (command === 'get_settings') return { themeMode: 'dark', themePreset: 'moss', customAccent: '#a6d991', customBackground: null, beginnerMode: true, showGameLog: true, updatePolicy: 'off', download: { source: 'official', maxConcurrentDownloads: 4, connectionsPerFile: 4 } }
        if (command === 'list_versions') return []
        if (command === 'list_accounts') return []
        if (command === 'list_instances') return state.deletions.length ? [] : [{ id: 'demo', name: '我的实例', gameVersion: '1.21.4', loader: 'vanilla', instanceDir: '/data/instances/demo', gameDir: '/data/instances/demo/game', isolated: true, state: 'ready', createdAt: '' }]
        if (command === 'loader_versions') return []
        if (command === 'list_mods') return []
        if (command === 'delete_instance') { state.deletions.push(args.deleteFiles as boolean); return }
        return 1
      },
    } })
  })
  await page.goto('/')
  await page.getByRole('button', { name: '实例库', exact: true }).click()
  await page.getByRole('button', { name: /管理实例/ }).click()
  // Cancelling keeps the instance.
  page.once('dialog', dialog => dialog.dismiss())
  await page.getByRole('button', { name: '删除实例…' }).click()
  expect(await page.evaluate(() => (window as unknown as { __testState: { deletions: boolean[] } }).__testState.deletions)).toEqual([])
  // Confirming mentions the directory and irreversibility, then deletes files.
  let message = ''
  page.once('dialog', dialog => { message = dialog.message(); void dialog.accept() })
  await page.getByRole('button', { name: '删除实例…' }).click()
  expect(message).toContain('/data/instances/demo')
  expect(message).toContain('无法恢复')
  await expect(page.getByText('实例与本地文件已删除。')).toBeVisible()
  expect(await page.evaluate(() => (window as unknown as { __testState: { deletions: boolean[] } }).__testState.deletions)).toEqual([true])
})
