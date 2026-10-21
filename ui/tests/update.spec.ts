import { expect, test } from '@playwright/test'

type UpdateStatus = { current: string; latest: string | null; notes: string; available: boolean; url: string | null; sha256: string | null }

async function boot(page: import('@playwright/test').Page, update: { status?: UpdateStatus; error?: string }) {
  await page.addInitScript(({ update }) => {
    const state = { removals: [] as string[], checks: 0, downloads: 0 }
    const accounts = [
      { id: 'ms', displayName: 'MsPlayer', kind: 'microsoft', selected: true, createdAt: '' },
      { id: 'offline', displayName: 'Alex', kind: 'offline', selected: false, createdAt: '' },
    ]
    Object.assign(window, { __testState: state, __TAURI_EVENT_PLUGIN_INTERNALS__: { unregisterListener: () => {} }, __TAURI_INTERNALS__: {
      transformCallback: () => 1,
      invoke: async (command: string, args: Record<string, unknown>) => {
        if (command === 'get_settings') return { themeMode: 'dark', themePreset: 'moss', customAccent: '#a6d991', customBackground: null, beginnerMode: true, showGameLog: true, updatePolicy: 'notify', download: { source: 'official', maxConcurrentDownloads: 4, connectionsPerFile: 4 } }
        if (command === 'list_accounts') return accounts.filter(account => !state.removals.includes(account.id))
        if (command === 'remove_account') { state.removals.push(args.accountId as string); return }
        if (command === 'get_system_info') return { operatingSystem: 'linux', architecture: 'x86_64', appDataDir: '/tmp', appVersion: '0.1.0' }
        if (command === 'check_launcher_update') {
          state.checks++
          if (update.error) throw new Error(update.error)
          return update.status
        }
        if (command === 'download_launcher_update') { state.downloads++; return '/downloads/MFMCL-0.2.0.AppImage' }
        if (command === 'plugin:dialog|open') return '/chosen-update-folder'
        if (command === 'list_versions') return []
        if (command === 'list_instances') return []
        return 1
      },
    } })
  }, { update })
}

const available: UpdateStatus = { current: '0.1.0', latest: '0.2.0', notes: '修复下载与登录问题。', available: true, url: 'https://example.com/MFMCL-0.2.0.AppImage', sha256: 'a'.repeat(64) }
const current: UpdateStatus = { current: '0.1.0', latest: '0.1.0', notes: '', available: false, url: null, sha256: null }

test('cached accounts can be removed after confirmation, keyring note shown for online accounts', async ({ page }) => {
  await boot(page, { status: current })
  await page.goto('/')
  await page.getByRole('button', { name: '账户选项' }).click()
  await expect(page.getByRole('button', { name: '移除账户 MsPlayer' })).toBeVisible()
  // Cancelling keeps the account.
  page.once('dialog', dialog => dialog.dismiss())
  await page.getByRole('button', { name: '移除账户 MsPlayer' }).click()
  await expect(page.getByRole('button', { name: '移除账户 MsPlayer' })).toBeVisible()
  // Confirming an online account mentions credential removal.
  let message = ''
  page.once('dialog', dialog => { message = dialog.message(); void dialog.accept() })
  await page.getByRole('button', { name: '移除账户 MsPlayer' }).click()
  await expect.poll(() => page.evaluate(() => (window as unknown as { __testState: { removals: string[] } }).__testState.removals)).toEqual(['ms'])
  expect(message).toContain('系统凭据库')
  await expect(page.getByRole('button', { name: '移除账户 MsPlayer' })).toHaveCount(0)
  await expect(page.getByRole('button', { name: '移除账户 Alex' })).toBeVisible()
  // Offline removal uses a plain confirmation without the keyring sentence.
  message = ''
  page.once('dialog', dialog => { message = dialog.message(); void dialog.accept() })
  await page.getByRole('button', { name: '移除账户 Alex' }).click()
  await expect.poll(() => page.evaluate(() => (window as unknown as { __testState: { removals: string[] } }).__testState.removals)).toEqual(['ms', 'offline'])
  expect(message).not.toContain('系统凭据库')
  await expect(page.getByText('账户已移除。')).toBeVisible()
})

test('startup check runs quietly and an available update offers a verified download', async ({ page }) => {
  await boot(page, { status: available })
  await page.goto('/')
  // The app-level notify policy triggers a quiet background check and reminder toast.
  await expect.poll(() => page.evaluate(() => (window as unknown as { __testState: { checks: number } }).__testState.checks)).toBeGreaterThanOrEqual(1)
  await expect(page.getByText(/发现新版本 0\.2\.0/)).toBeVisible()
  await page.getByRole('button', { name: '设置', exact: true }).click()
  const card = page.locator('.update-card')
  await expect(card.getByText('新版本 0.2.0 可用')).toBeVisible()
  await expect(card.getByText('修复下载与登录问题。')).toBeVisible()
  await card.getByRole('button', { name: '下载安装包' }).click()
  await expect(card.getByText(/安装包已保存；运行安装包并重启 MFMCL 后生效/)).toBeVisible()
  expect(await page.evaluate(() => (window as unknown as { __testState: { downloads: number } }).__testState.downloads)).toBe(1)
})

test('manual check surfaces backend errors and unconfigured builds stay honest', async ({ page }) => {
  await boot(page, { error: '此构建未配置更新清单地址；请在构建时设置 MFMCL_UPDATE_MANIFEST 后重新分发。' })
  await page.goto('/')
  await page.getByRole('button', { name: '设置', exact: true }).click()
  const card = page.locator('.update-card')
  // Quiet startup failure shows nothing; the manual check reveals the reason.
  await expect(card.getByText(/未配置更新清单地址/)).toHaveCount(0)
  await card.getByRole('button', { name: '立即检查更新' }).click()
  await expect(card.getByText(/未配置更新清单地址/)).toBeVisible()
  // Policy switching persists through the shared settings mapping.
  await card.getByRole('button', { name: /^关闭/ }).click()
  await expect(card.getByRole('button', { name: /^关闭/ })).toHaveClass(/selected/)
})
