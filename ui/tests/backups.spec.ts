import { expect, test } from '@playwright/test'

test('backup requires privacy confirmation and restore creates a new instance from the checked preview', async ({ page }) => {
  await page.addInitScript(() => {
    const state = { calls: [] as { command: string; args?: Record<string, unknown> }[], instances: [{ id:'source', name:'原世界', gameVersion:'1.21.4', loader:'vanilla', state:'ready' }] }
    Object.assign(window, { __backupTest: state, __TAURI_EVENT_PLUGIN_INTERNALS__: { unregisterListener: () => {} }, __TAURI_INTERNALS__: {
      transformCallback: () => 1,
      invoke: async (command: string, args?: Record<string, unknown>) => {
        state.calls.push({ command, args })
        if (command === 'get_settings') return { themeMode:'dark', themePreset:'moss', customAccent:'#a6d991', beginnerMode:true, showGameLog:true, download: { source:'official', maxConcurrentDownloads:4, connectionsPerFile:4 } }
        if (command === 'list_instances') return state.instances
        if (command === 'list_accounts' || command === 'list_versions') return []
        if (command === 'plugin:dialog|open') return (args?.options as { directory?: boolean })?.directory ? '/user/backups' : '/user/backups/world.mfmclbackup'
        if (command === 'create_backup') return '/user/backups/new.mfmclbackup'
        if (command === 'preview_backup') return { name:'保存的世界', minecraft:'1.21.4', loader:'fabric@0.16.10', fileCount:3, totalBytes:1024, archiveSha256:'a'.repeat(64), createdAt:'' }
        if (command === 'restore_backup') { const target = { id:'new', name:args?.name as string, gameVersion:'1.21.4', loader:'vanilla', state:'not_installed' }; state.instances = [target, ...state.instances]; return target }
        return null
      },
    } })
  })
  await page.goto('/')
  await page.getByRole('button', { name:'实例库', exact:true }).click()
  const section = page.getByRole('region', { name:'私人备份与恢复' })
  page.once('dialog', dialog => dialog.dismiss())
  await section.getByRole('button', { name:'创建本地备份' }).click()
  expect(await page.evaluate(() => (window as unknown as { __backupTest:{ calls:{command:string}[] } }).__backupTest.calls.filter(c=>c.command==='create_backup').length)).toBe(0)
  page.once('dialog', dialog => { expect(dialog.message()).toContain('私人信息'); return dialog.accept() })
  await section.getByRole('button', { name:'创建本地备份' }).click()
  await expect(page.getByRole('status')).toContainText('本地备份已保存')
  await section.getByRole('button', { name:'从备份恢复' }).click()
  const dialog = page.getByRole('dialog', { name:'恢复备份为新实例' })
  await expect(dialog).toContainText('原实例保持不变')
  await dialog.getByLabel('新实例名称').fill('恢复测试')
  await dialog.getByRole('button', { name:'信任并恢复为新实例' }).click()
  await expect.poll(() => page.evaluate(() => (window as unknown as { __backupTest:{ calls:{command:string,args:Record<string,unknown>}[] } }).__backupTest.calls.find(c=>c.command==='restore_backup')?.args)).toEqual({ path:'/user/backups/world.mfmclbackup', archiveSha256:'a'.repeat(64), name:'恢复测试', confirmed:true })
  await expect(page.getByRole('status')).toContainText('原实例未修改')
})
