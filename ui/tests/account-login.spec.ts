import { expect, test } from '@playwright/test'

const microsoftAccount = {
  id: 'microsoft-account', displayName: 'Mina', kind: 'microsoft', selected: true, createdAt: '2026-10-21T00:00:00Z',
}

async function installDesktopMock(page: import('@playwright/test').Page) {
  await page.addInitScript(() => {
    const state: { calls: { command: string; args?: Record<string, unknown> }[]; accounts: Record<string, unknown>[]; resolveSkin?: (value: unknown) => void; resolveMicrosoft?: (value: unknown) => void; skinError?: boolean; microsoftError?: boolean } = { calls: [], accounts: [] }
    Object.assign(window, {
      __authTestState: state,
      __TAURI_EVENT_PLUGIN_INTERNALS__: { unregisterListener: () => {} },
      __TAURI_INTERNALS__: {
        transformCallback: () => 1,
        invoke: async (command: string, args?: Record<string, unknown>) => {
          state.calls.push({ command, args })
          if (command === 'get_settings') return { themeMode: 'dark', themePreset: 'moss', customAccent: '#a6d991', customBackground: null, beginnerMode: true, showGameLog: true, download: { source: 'official', maxConcurrentDownloads: 4, connectionsPerFile: 4 } }
          if (command === 'save_settings') return
          if (command === 'list_versions' || command === 'list_instances') return []
          if (command === 'list_accounts') return state.accounts
          if (command === 'detect_java') throw new Error('No Java in test')
          if (command === 'skin_login') { if (state.skinError) throw '认证未完成：用户名或密码不正确。'; return new Promise((resolve) => { state.resolveSkin = resolve }) }
          if (command === 'skin_select_profile') { const account = { id: 'skin-account', displayName: 'Alex', kind: 'skin_server', authServer: 'https://auth.example.com/', credentialRef: 'opaque-vault-reference', selected: true, createdAt: '2026-10-21T00:00:00Z' }; state.accounts = [account]; return account }
          if (command === 'microsoft_login') { if (state.microsoftError) throw '此开发构建尚未配置 Microsoft 应用 ID。'; return new Promise((resolve) => { state.resolveMicrosoft = resolve }) }
          return null
        },
      },
    })
  })
}

test('skin login requires HTTPS trust, clears password, selects a profile, and refreshes accounts', async ({ page }) => {
  await installDesktopMock(page)
  await page.goto('/')
  await page.getByRole('button', { name: '设置', exact: true }).click()
  await page.getByRole('button', { name: '配置服务' }).click()

  const dialog = page.getByRole('dialog', { name: '连接皮肤站' })
  await dialog.getByLabel(/HTTPS 认证根地址/).fill('https://auth.example.com')
  await dialog.getByLabel('皮肤站用户名').fill('Alex')
  const password = dialog.getByLabel('密码')
  await password.fill('private-password')
  await expect(dialog.getByText('https://auth.example.com', { exact: true })).toBeVisible()
  const signIn = dialog.getByRole('button', { name: '登录并选择角色' })
  await expect(signIn).toBeDisabled()
  expect(await page.evaluate(() => (window as unknown as { __authTestState: { calls: { command: string }[] } }).__authTestState.calls.some((call) => call.command === 'skin_login'))).toBe(false)

  await dialog.getByLabel(/我已核对上述 HTTPS 地址/).check()
  await signIn.click()
  await expect(password).toHaveValue('')
  await expect(dialog.getByText('正在安全连接…')).toBeVisible()
  await expect.poll(() => page.evaluate(() => (window as unknown as { __authTestState: { calls: { command: string; args?: Record<string, unknown> }[] } }).__authTestState.calls.find((call) => call.command === 'skin_login')?.args)).toMatchObject({ server: 'https://auth.example.com/', username: 'Alex', password: 'private-password', trusted: true })
  await page.evaluate(() => (window as unknown as { __authTestState: { resolveSkin?: (value: unknown) => void } }).__authTestState.resolveSkin?.({ pendingId: 'pending-profile-choice', profiles: [{ id: 'profile-a', name: 'Alex' }, { id: 'profile-b', name: 'Builder' }] }))

  await expect(page.getByRole('dialog', { name: '选择游戏角色' })).toBeVisible()
  await page.getByRole('combobox', { name: /游戏角色/ }).selectOption('profile-b')
  await page.getByRole('button', { name: '确认角色' }).click()
  await expect(page.getByRole('status')).toContainText('皮肤站 账户“Alex”已连接。')
  await expect.poll(() => page.evaluate(() => (window as unknown as { __authTestState: { calls: { command: string; args?: Record<string, unknown> }[] } }).__authTestState.calls.find((call) => call.command === 'skin_select_profile')?.args)).toEqual({ pendingId: 'pending-profile-choice', profileId: 'profile-b' })

  await page.getByRole('button', { name: '账户选项' }).click()
  await expect(page.getByText('皮肤站账户 · https://auth.example.com/')).toBeVisible()
  await expect(page.getByText(/private-password|access_token|refresh_token/i)).toHaveCount(0)
})

test('Microsoft login submits optional public client ID and refreshes account list', async ({ page }) => {
  await installDesktopMock(page)
  await page.goto('/')
  await page.getByRole('button', { name: '设置', exact: true }).click()
  await page.getByRole('button', { name: '连接账户' }).click()

  const dialog = page.getByRole('dialog', { name: '连接 Microsoft 账户' })
  await dialog.getByLabel(/公共客户端 ID/).fill('approved-public-client-id')
  await dialog.getByRole('button', { name: '在浏览器中登录' }).click()
  await expect(dialog.getByText('正在打开系统浏览器并等待授权回调…')).toBeVisible()
  await expect.poll(() => page.evaluate(() => (window as unknown as { __authTestState: { calls: { command: string; args?: Record<string, unknown> }[] } }).__authTestState.calls.find((call) => call.command === 'microsoft_login')?.args)).toEqual({ clientId: 'approved-public-client-id' })

  await page.evaluate((account) => {
    const state = (window as unknown as { __authTestState: { resolveMicrosoft?: (value: unknown) => void; accounts: Record<string, unknown>[] } }).__authTestState
    state.accounts = [account]
    state.resolveMicrosoft?.(account)
  }, microsoftAccount)
  await expect(page.getByRole('status')).toContainText('Microsoft 账户“Mina”已连接。')
  await page.getByRole('button', { name: '账户选项' }).click()
  await expect(page.getByRole('dialog', { name: '选择账户' }).getByRole('button', { name: /Mina Microsoft 账户/ })).toBeVisible()
  await expect(page.locator('.profile-meta')).toContainText('MinaMicrosoft 账户')
  await expect.poll(() => page.evaluate(() => (window as unknown as { __authTestState: { calls: { command: string }[] } }).__authTestState.calls.filter((call) => call.command === 'list_accounts').length)).toBeGreaterThan(1)
  await expect(page.getByText(/access_token|refresh_token|credentialRef/)).toHaveCount(0)
})

test('both login dialogs keep backend authentication errors visible', async ({ page }) => {
  await installDesktopMock(page)
  await page.goto('/')
  await page.evaluate(() => {
    const state = (window as unknown as { __authTestState: { microsoftError?: boolean; skinError?: boolean } }).__authTestState
    state.microsoftError = true
    state.skinError = true
  })
  await page.getByRole('button', { name: '设置', exact: true }).click()
  await page.getByRole('button', { name: '连接账户' }).click()
  const microsoft = page.getByRole('dialog', { name: '连接 Microsoft 账户' })
  await microsoft.getByRole('button', { name: '在浏览器中登录' }).click()
  await expect(microsoft.getByRole('alert')).toContainText('此开发构建尚未配置 Microsoft 应用 ID。')
  await microsoft.getByRole('button', { name: '取消' }).click()

  await page.getByRole('button', { name: '配置服务' }).click()
  const skin = page.getByRole('dialog', { name: '连接皮肤站' })
  await skin.getByLabel(/HTTPS 认证根地址/).fill('https://auth.example.com/')
  await skin.getByLabel('皮肤站用户名').fill('Alex')
  const password = skin.getByLabel('密码')
  await password.fill('bad-password')
  await skin.getByLabel(/我已核对上述 HTTPS 地址/).check()
  await skin.getByRole('button', { name: '登录并选择角色' }).click()
  await expect(password).toHaveValue('')
  await expect(skin.getByRole('alert')).toContainText('用户名或密码不正确。')
})

test('skin server validation rejects insecure or credential-bearing roots', async () => {
  const { validateSkinServer } = await import('../src/api')
  expect(() => validateSkinServer('http://auth.example.com/')).toThrow('HTTPS')
  expect(() => validateSkinServer('https://user:secret@auth.example.com/')).toThrow('HTTPS')
  expect(() => validateSkinServer('https://auth.example.com/?redirect=elsewhere')).toThrow('HTTPS')
  expect(validateSkinServer('https://auth.example.com/auth')).toBe('https://auth.example.com/auth/')
})
