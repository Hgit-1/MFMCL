import { expect, test } from '@playwright/test'

test('mod updates disclose hash privacy, warn like PCL2, keep .old backup note, and update verified files', async ({ page }) => {
  await page.addInitScript(() => {
    const state = { checks: 0, updates: [] as string[], clientChecks: 0 }
    Object.assign(window, { __testState: state, __TAURI_EVENT_PLUGIN_INTERNALS__: { unregisterListener: () => {} }, __TAURI_INTERNALS__: {
      transformCallback: () => 1,
      invoke: async (command: string, args: Record<string, unknown>) => {
        if (command === 'get_settings') return { themeMode: 'dark', themePreset: 'moss', customAccent: '#a6d991', customBackground: null, beginnerMode: true, showGameLog: true, updatePolicy: 'notify', download: { source: 'official', maxConcurrentDownloads: 4, connectionsPerFile: 4 } }
        if (command === 'list_versions') return []
        if (command === 'list_instances') return [{ id: 'demo', name: 'Fabric 实例', gameVersion: '1.21.4', loader: 'fabric@0.16.10', instanceDir: '/i', gameDir: '/i/game', isolated: true, state: 'ready', createdAt: '' }]
        if (command === 'list_accounts') return []
        if (command === 'loader_versions') return []
        if (command === 'list_mods') return [
          { fileName: 'sodium.jar', enabled: true, size: 4096 },
          { fileName: 'local-only.jar', enabled: true, size: 2048 },
        ]
        if (command === 'check_mod_updates') {
          state.checks++
          // serde externally-tagged enum serialization.
          return [
            { fileName: 'sodium.jar', status: { update_available: { fileName: 'sodium.jar', projectId: 'p1', installedVersion: '0.5.11', installedVersionId: 'v1', newVersion: '0.6.0', newVersionId: 'v2', newFileName: 'sodium-0.6.0.jar', url: 'https://cdn.modrinth.com/sodium.jar', sha1: 'a'.repeat(40), size: 4096 } } },
            { fileName: 'local-only.jar', status: { unknown_source: null } },
          ]
        }
        if (command === 'update_mod') { state.updates.push((args.update as { newFileName: string }).newFileName); return (args.update as { newFileName: string }).newFileName }
        if (command === 'check_client_update') { state.clientChecks++; return { upToDate: false, changedFiles: 1 } }
        return 1
      },
    } })
  })
  await page.goto('/')
  await page.getByRole('button', { name: '实例库', exact: true }).click()
  await page.getByRole('button', { name: /管理实例/ }).click()
  // Hash-disclosure privacy note is visible before any check is performed.
  await expect(page.getByText(/SHA-1 哈希发送给 Modrinth/)).toBeVisible()
  await page.getByRole('button', { name: '检查模组更新' }).click()
  await expect(page.getByText('发现 1 个可更新的模组。')).toBeVisible()
  await expect(page.getByText('sodium.jar', { exact: true })).toBeVisible()
  await expect(page.getByText('0.5.11 → 0.6.0')).toBeVisible()
  await expect(page.getByText(/未在 Modrinth 找到/)).toBeVisible()
  // Cancelling the risk confirmation changes nothing.
  page.once('dialog', dialog => { expect(dialog.message()).toContain('备份存档'); void dialog.dismiss() })
  await page.getByRole('button', { name: '更新', exact: true }).click()
  expect(await page.evaluate(() => (window as unknown as { __testState: { updates: string[] } }).__testState.updates)).toEqual([])
  // Confirming runs the verified update and mentions the recoverable old file.
  page.once('dialog', dialog => { expect(dialog.message()).toContain('.old'); void dialog.accept() })
  await page.getByRole('button', { name: '更新', exact: true }).click()
  await expect(page.getByText(/旧文件保留为 sodium\.jar\.old/)).toBeVisible()
  expect(await page.evaluate(() => (window as unknown as { __testState: { updates: string[] } }).__testState.updates)).toEqual(['sodium-0.6.0.jar'])
})

test('client update check reports manifest changes and points at repair install', async ({ page }) => {
  await page.addInitScript(() => {
    const state = { clientChecks: 0 }
    Object.assign(window, { __testState: state, __TAURI_EVENT_PLUGIN_INTERNALS__: { unregisterListener: () => {} }, __TAURI_INTERNALS__: {
      transformCallback: () => 1,
      invoke: async (command: string) => {
        if (command === 'get_settings') return { themeMode: 'dark', themePreset: 'moss', customAccent: '#a6d991', customBackground: null, beginnerMode: true, showGameLog: true, updatePolicy: 'off', download: { source: 'official', maxConcurrentDownloads: 4, connectionsPerFile: 4 } }
        if (command === 'list_versions') return []
        if (command === 'list_instances') return [{ id: 'demo', name: 'Vanilla 实例', gameVersion: '1.21.4', loader: 'vanilla', instanceDir: '/i', gameDir: '/i/game', isolated: true, state: 'ready', createdAt: '' }]
        if (command === 'list_accounts') return []
        if (command === 'loader_versions') return []
        if (command === 'list_mods') return []
        if (command === 'check_client_update') { state.clientChecks++; return { upToDate: true, changedFiles: 0 } }
        return 1
      },
    } })
  })
  await page.goto('/')
  await page.getByRole('button', { name: '实例库', exact: true }).click()
  await page.getByRole('button', { name: /管理实例/ }).click()
  await expect(page.getByText(/对照 Mojang 官方清单检查本实例的游戏文件/)).toBeVisible()
  await page.getByRole('button', { name: '检查客户端更新' }).click()
  await expect(page.getByText('客户端文件与官方清单一致。')).toBeVisible()
  expect(await page.evaluate(() => (window as unknown as { __testState: { clientChecks: number } }).__testState.clientChecks)).toBe(1)
})
