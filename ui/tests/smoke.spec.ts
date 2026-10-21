import { expect, test } from '@playwright/test'
import { accessibleAccent, contrast } from '../src/theme'
import { validateSettings } from '../src/api'

test('custom colours remain legible in dark and light themes', () => {
  for (const colour of ['#000000', '#ffffff', '#ff0000', '#123456', '#a6d991']) {
    for (const light of [true, false]) {
      const theme = accessibleAccent(colour, light)
      expect(contrast(theme.ink, theme.fill)).toBeGreaterThanOrEqual(4.5)
      expect(contrast(theme.text, light ? '#ffffff' : '#151f19')).toBeGreaterThanOrEqual(4.5)
    }
  }
})
test('persisted preferences are treated as untrusted input', () => {
  const settings = validateSettings({ ariaConnections: -9, memoryMb: 999999, customAccent: 'red; background: url(evil)', customBackground: 'https://example.org/tracking.png' })
  expect(settings.ariaConnections).toBe(4)
  expect(settings.customBackground).toBeNull()
  expect(settings.memoryMb).toBe(2048)
  expect(settings.customAccent).toBe('#a6d991')
})
test('preview is honest about native operations and restores theme', async ({ page }) => {
  await page.goto('/')
  await expect(page.getByRole('heading', { name: '欢迎回来。' })).toBeVisible()
  await page.getByRole('button', { name: '设置', exact: true }).click()
  await page.getByRole('button', { name: '浅色' }).click()
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'light')
  await page.getByRole('button', { name: '湖水青 清爽专注' }).click()
  await page.waitForFunction(() => JSON.parse(localStorage.getItem('mfmcl-settings') ?? '{}').theme === 'ocean')
  await page.reload()
  await page.getByRole('button', { name: '设置', exact: true }).click()
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'light')
  await expect(page.getByRole('button', { name: /湖水青/ })).toContainText('已选')
  await page.getByRole('button', { name: '实例库', exact: true }).click()
  await page.getByRole('button', { name: '导入整合包', exact: true }).last().click()
  await expect(page.getByRole('status')).toContainText('桌面应用')
})
test('mobile navigation works without horizontal overflow', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 })
  await page.goto('/')
  await page.getByRole('button', { name: '打开导航' }).click()
  await page.getByRole('button', { name: '设置', exact: true }).click()
  await expect(page.getByRole('heading', { name: '按你的方式来。' })).toBeVisible()
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true)
})
