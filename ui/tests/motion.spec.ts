import { expect, test } from '@playwright/test'

// Motion is decoration, but two of its properties are functional contracts:
// entrances must release their transform (or hover/press stops working), and
// reduced-motion users must get the instant, static version of every overlay.
test('entrance animations release their transform so hover still lifts', async ({ page }) => {
  await page.goto('/')
  const quick = page.locator('.quick-card').first()
  await quick.scrollIntoViewIfNeeded()
  // Wait past the longest staggered entrance (≤ 230ms delay + 460ms duration).
  await page.waitForTimeout(1200)
  await expect.poll(() => quick.evaluate((el) => getComputedStyle(el).transform)).toBe('none')
  await quick.hover()
  await expect.poll(() => quick.evaluate((el) => getComputedStyle(el).transform)).toContain('-3')
})

test('theme changes reveal from the clicked control and persist', async ({ page }) => {
  await page.goto('/')
  await page.getByRole('button', { name: '设置', exact: true }).click()
  await page.getByRole('button', { name: '浅色' }).click()
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'light')
  await page.waitForTimeout(700)
  await expect(page.locator('.page-enter')).toHaveCSS('opacity', '1')
})

test('dialogs animate out before they unmount', async ({ page }) => {
  await page.goto('/')
  await page.getByRole('button', { name: '创建新实例' }).click()
  const dialog = page.getByRole('dialog', { name: '创建游戏实例' })
  await expect(dialog).toBeVisible()
  await page.getByRole('button', { name: '取消' }).click()
  await expect(page.locator('.modal-backdrop.is-leaving')).toHaveCount(1)
  await expect(page.locator('.modal-backdrop')).toHaveCount(0)
})

test('reduced motion keeps every overlay instant and static', async ({ browser }) => {
  const context = await browser.newContext({ reducedMotion: 'reduce' })
  const page = await context.newPage()
  await page.goto('/')
  const hero = page.locator('.hero-card')
  await hero.hover({ position: { x: 120, y: 120 } })
  await page.waitForTimeout(250)
  // Parallax is skipped entirely, so the layers keep their resting position.
  expect(await hero.evaluate((el) => getComputedStyle(el).getPropertyValue('--tilt-x').trim())).toBe('0px')
  expect(await page.locator('.hero-title-block h2 em').evaluate((el) => getComputedStyle(el).animationName)).toBe('none')
  await page.getByRole('button', { name: '创建新实例' }).click()
  await page.getByRole('button', { name: '取消' }).click()
  // No exit-phase: dismissal is synchronous for users who asked for no motion.
  await expect(page.locator('.modal-backdrop')).toHaveCount(0)
  await context.close()
})
