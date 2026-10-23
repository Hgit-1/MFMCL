import { expect, test, type Page } from '@playwright/test'

type Phase = { accounts: number; java: boolean; instance?: 'none' | 'created' | 'ready' }

async function boot(page: Page, phase: Phase) {
  await page.addInitScript(({ phase }: { phase: Phase }) => {
    const accounts = Array.from({ length: phase.accounts }, (_, index) => ({ id: `a${index}`, displayName: `玩家${index}`, kind: 'microsoft', selected: index === 0, createdAt: '' }))
    const instances = phase.instance === 'none' || !phase.instance ? [] : phase.instance === 'created'
      ? [{ id: 'i1', name: '首个实例', gameVersion: '1.21.4', loader: 'vanilla', instanceDir: '/i', gameDir: '/i/game', isolated: true, state: 'not_installed', createdAt: '' }]
      : [{ id: 'i1', name: '首个实例', gameVersion: '1.21.4', loader: 'vanilla', instanceDir: '/i', gameDir: '/i/game', isolated: true, state: 'ready', createdAt: '' }]
    Object.assign(window, { __TAURI_EVENT_PLUGIN_INTERNALS__: { unregisterListener: () => {} }, __TAURI_INTERNALS__: {
      transformCallback: () => 1,
      invoke: async (command: string) => {
        if (command === 'get_settings') return { themeMode: 'dark', themePreset: 'moss', customAccent: '#a6d991', customBackground: null, beginnerMode: true, showGameLog: true, updatePolicy: 'off', download: { source: 'official', maxConcurrentDownloads: 4, connectionsPerFile: 4 } }
        if (command === 'list_versions') return []
        if (command === 'list_accounts') return accounts
        if (command === 'list_instances') return instances
        if (command === 'detect_java') { if (phase.java) return { path: '/java', majorVersion: 21, versionText: '21' }; throw new Error('no java') }
        if (command === 'check_launcher_update') return { current: '0.1.0', latest: null, notes: '', available: false, url: null, sha256: null, size: null }
        return 1
      },
    } })
  }, { phase })
}

test('empty first run shows all four steps with routing shortcuts', async ({ page }) => {
  await boot(page, { accounts: 0, java: false, instance: 'none' })
  await page.goto('/')
  const guide = page.getByRole('region', { name: '新手引导' })
  await expect(guide.getByText('已完成 0/4 步')).toBeVisible()
  await expect(guide.getByRole('button', { name: '添加账户' })).toBeVisible()
  await expect(guide.getByRole('button', { name: '打开设置' })).toBeVisible()
  await expect(guide.getByRole('button', { name: '创建实例' })).toBeVisible()
  await expect(guide.getByText('安装并启动游戏')).toBeVisible()
  await expect(guide.getByText(/离线档案不验证正版授权/)).toBeVisible()
})

test('an account completes step one and clears its shortcut', async ({ page }) => {
  await boot(page, { accounts: 1, java: false, instance: 'none' })
  await page.goto('/')
  const guide = page.getByRole('region', { name: '新手引导' })
  await expect(guide.getByText('已完成 1/4 步')).toBeVisible()
  await expect(guide.getByText('账户已就绪，可在左下角切换。')).toBeVisible()
  await expect(guide.getByRole('button', { name: '添加账户' })).toHaveCount(0)
})

test('java and a created instance leave only the launch step', async ({ page }) => {
  await boot(page, { accounts: 1, java: true, instance: 'created' })
  await page.goto('/')
  const guide = page.getByRole('region', { name: '新手引导' })
  await expect(guide.getByText('已完成 3/4 步')).toBeVisible()
  await expect(guide.getByText('已检测到可用的 Java。')).toBeVisible()
  await expect(guide.getByRole('button', { name: '创建实例' })).toHaveCount(0)
})

test('a ready instance completes the guide', async ({ page }) => {
  await boot(page, { accounts: 1, java: true, instance: 'ready' })
  await page.goto('/')
  await expect(page.getByRole('region', { name: '新手引导' }).getByText('所有步骤都已完成，祝你玩得开心。')).toBeVisible()
})

test('guide buttons route to the real flows', async ({ page }) => {
  await boot(page, { accounts: 0, java: false, instance: 'none' })
  await page.goto('/')
  await page.getByRole('region', { name: '新手引导' }).getByRole('button', { name: '添加账户' }).click()
  await expect(page.getByRole('dialog', { name: '添加账户' })).toBeVisible()
})
