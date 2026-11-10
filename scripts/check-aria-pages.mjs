/**
 * Check the Aria watermark on every page of the launcher, at several sizes.
 *
 * ## Why the earlier sweep was not enough
 *
 * That one navigated to the library page only. A user then reported occlusion on
 * the store's *version list* page, where a single results card covers nearly the
 * whole content area — a shape the library's card grid never produces. One page
 * per sweep is one page of coverage.
 *
 * This visits each page the watermark is supposed to appear on, and for each page
 * and size it answers two questions with pixels rather than styles:
 *
 *   1. Does hiding the layer change the capture? If not, she is painted nowhere.
 *   2. How many of her pixels survive? A page whose card covers 95% of the
 *      content area is not a bug in the stacking, but it does make the feature
 *      pointless, and the report should say so instead of claiming success.
 *
 * Usage (Vite must be running on 5173):
 *   node scripts/check-aria-pages.mjs
 */
import { spawn } from 'node:child_process'
import { mkdir, writeFile } from 'node:fs/promises'
import { setTimeout as sleep } from 'node:timers/promises'
import path from 'node:path'
import { createHash } from 'node:crypto'

const EDGE = 'C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe'
const OUT = path.join(process.env.TEMP ?? '/tmp', 'aria-audit', 'pages-check')
const URL = 'http://127.0.0.1:5173/'
const PORT = 9338

/** Pages with per-page artwork, and how to reach each one. */
const PAGES = [
  {
    name: 'library',
    // 游戏库
    codepoints: [0x6e38, 0x620f, 0x5e93],
  },
  {
    name: 'store-versions',
    // 内容商店 then the first category 版本
    codepoints: [0x5185, 0x5bb9, 0x5546, 0x5e97],
    categoryIndex: 0,
  },
  {
    name: 'downloads',
    codepoints: [0x4e0b, 0x8f7d, 0x4efb, 0x52a1],
  },
  {
    name: 'plugins',
    codepoints: [0x542f, 0x52a8, 0x5668, 0x63d2, 0x4ef6],
  },
]

const SIZES = [
  { w: 1440, h: 900, name: 'laptop' },
  { w: 1366, h: 768, name: 'hd-ready' },
  { w: 1024, h: 768, name: 'xga' },
  { w: 2560, h: 1440, name: 'qhd' },
]

async function cdp() {
  const browser = spawn(EDGE, [
    '--headless=new',
    '--disable-gpu',
    '--hide-scrollbars',
    `--remote-debugging-port=${PORT}`,
    '--user-data-dir=' + path.join(process.env.TEMP ?? '/tmp', 'aria-audit', 'pages-profile'),
    '--no-first-run',
    'about:blank',
  ], { stdio: 'ignore' })

  let target = null
  for (let attempt = 0; attempt < 80 && !target; attempt += 1) {
    await sleep(250)
    try {
      const response = await fetch(`http://127.0.0.1:${PORT}/json/list`)
      const list = await response.json()
      target = list.find(item => item.type === 'page') ?? null
    } catch {
      // not up yet
    }
  }
  if (!target) throw new Error('devtools endpoint never came up')

  const socket = new WebSocket(target.webSocketDebuggerUrl)
  await new Promise((resolve, reject) => {
    socket.onopen = resolve
    socket.onerror = () => reject(new Error('websocket failed'))
  })

  let nextId = 1
  const pending = new Map()
  socket.onmessage = event => {
    const message = JSON.parse(event.data)
    if (message.id && pending.has(message.id)) {
      const { resolve, reject } = pending.get(message.id)
      pending.delete(message.id)
      if (message.error) reject(new Error(JSON.stringify(message.error)))
      else resolve(message.result)
    }
  }
  const send = (method, params = {}) => new Promise((resolve, reject) => {
    const id = nextId++
    pending.set(id, { resolve, reject })
    socket.send(JSON.stringify({ id, method, params }))
  })

  return { browser, socket, send }
}

const SEED = `
  localStorage.setItem('mfmcl-eula-version', '1');
  localStorage.setItem('mfmcl-onboarding', JSON.stringify({ completedAt: Date.now() }));
  localStorage.setItem('mfmcl-aria', JSON.stringify({
    enabled: true, mode: 'fixed', fixedSeason: 'autumn',
    southernHemisphere: false, showSidebar: true, showHero: true,
    showWatermark: true, tintSurfaces: false
  }));
  localStorage.setItem('flowlaunch-plugins', JSON.stringify({
    custom: [], enabled: { 'aria-with-you': true },
    options: {}, acknowledgedUpdates: {}
  }));
`

/**
 * Locate the figure so the caller can crop the screenshots to it.
 *
 * Coverage is deliberately *not* measured here. `elementFromPoint` cannot see a
 * `pointer-events: none` element, so every sample came back with the card
 * underneath and reported "0% clear" even on the library page whose screenshot
 * plainly showed her. Anything that has to answer "is she visible" has to look
 * at pixels.
 */
const PROBE = `(() => {
  const figure = document.querySelector('figure[data-variant="watermark"]');
  const layer = document.querySelector('[class*="ariaWatermarkLayer"]');
  const main = document.querySelector('main');
  if (!figure || !layer || !main) return JSON.stringify({ shown: false });
  if (getComputedStyle(figure).display === 'none') return JSON.stringify({ shown: false, reason: 'display:none' });

  const box = figure.getBoundingClientRect();
  const content = main.getBoundingClientRect();
  return JSON.stringify({
    shown: true,
    w: Math.round(box.width),
    h: Math.round(box.height),
    ratio: +(box.width * box.height / (content.width * content.height)).toFixed(3),
    box: [Math.round(box.x), Math.round(box.y), Math.round(box.width), Math.round(box.height)],
    insideWindow: box.left >= -1 && box.right <= innerWidth + 1 && box.bottom <= innerHeight + 1
  });
})()`

const hash = buffer => createHash('sha1').update(buffer).digest('hex')

/**
 * Count how many pixels the artwork actually contributes, from two screenshots.
 *
 * ## Why this is the only measurement that works
 *
 * Everything else lied at some point in this work:
 *
 *   - DOM presence: the element existed while invisible behind `.main`'s plate;
 *   - computed styles: both real bugs had perfectly correct styles;
 *   - `elementFromPoint`: `pointer-events: none` makes hit-testing skip the
 *     figure, so it always answers "the card underneath" and reported 0% visible
 *     on pages whose screenshots showed her clearly.
 *
 * Two captures — one with the layer, one with it hidden — are compared pixel by
 * pixel inside the figure's box. The number of differing pixels *is* the artwork's
 * painted area. It cannot be fooled, because it is the thing users see.
 *
 * PNG decoding: Chrome's capture is RGBA PNG. Rather than pull a decoder
 * dependency, this shells out to Python, which already has Pillow in this
 * environment for the asset tooling. If Pillow is missing the check degrades to
 * "did the files differ at all", which still catches total occlusion but cannot
 * report a fraction.
 */
async function visibleFraction(pngWith, pngWithout, box) {
  const tmp = path.join(OUT, '_diff')
  await mkdir(tmp, { recursive: true })
  const withPath = path.join(tmp, 'with.png')
  const withoutPath = path.join(tmp, 'without.png')
  await writeFile(withPath, pngWith)
  await writeFile(withoutPath, pngWithout)

  const script = `
import sys, json
from PIL import Image
import numpy as np
x0, y0, w, h = ${JSON.stringify(box)}
a = Image.open(sys.argv[1]).convert('RGB')
b = Image.open(sys.argv[2]).convert('RGB')
if a.size != b.size:
    print(json.dumps({'ok': False, 'reason': 'size mismatch'}))
    raise SystemExit
box = (max(0, x0), max(0, y0), min(a.width, x0 + w), min(a.height, y0 + h))
pa = np.asarray(a.crop(box), dtype=np.int16)
pb = np.asarray(b.crop(box), dtype=np.int16)
diff = np.abs(pa - pb).max(axis=2)
changed = int((diff > 2).sum())
total = int(diff.size)
print(json.dumps({'ok': True, 'changed': changed, 'total': total,
                  'fraction': round(changed / total, 4) if total else 0}))
`
  const { execFile } = await import('node:child_process')
  return await new Promise(resolve => {
    execFile('python', ['-c', script, withPath, withoutPath], { timeout: 60000 }, (error, stdout) => {
      if (error) return resolve({ ok: false, reason: String(error).slice(0, 120) })
      try {
        resolve(JSON.parse(stdout.trim()))
      } catch {
        resolve({ ok: false, reason: 'parse: ' + stdout.slice(0, 120) })
      }
    })
  })
}

async function main() {
  await mkdir(OUT, { recursive: true })
  const { browser, socket, send } = await cdp()
  await send('Page.enable')
  await send('Runtime.enable')

  await send('Emulation.setDeviceMetricsOverride', { width: 1440, height: 900, deviceScaleFactor: 1, mobile: false })
  await send('Page.navigate', { url: URL })
  await sleep(2200)
  await send('Runtime.evaluate', { expression: SEED })
  await send('Page.reload', { ignoreCache: false })
  await sleep(2600)

  const rows = []
  for (const size of SIZES) {
    await send('Emulation.setDeviceMetricsOverride', { width: size.w, height: size.h, deviceScaleFactor: 1, mobile: false })
    await sleep(600)

    for (const page of PAGES) {
      // Navigate by clicking, because the app has no routes.
      const clicked = await send('Runtime.evaluate', {
        expression: `(() => {
          const label = String.fromCodePoint(${page.codepoints.join(',')});
          const button = [...document.querySelectorAll('aside button, nav button')]
            .find(b => (b.textContent || '').includes(label));
          if (!button) return 'not-found';
          button.click();
          return 'clicked';
        })()`,
        returnByValue: true,
      })
      if (clicked.result.value !== 'clicked') {
        rows.push({ size: size.name, page: page.name, error: 'nav not found' })
        continue
      }
      await sleep(500)

      // The store's nav button only expands a sub-menu.
      if (page.categoryIndex !== undefined) {
        await send('Runtime.evaluate', {
          expression: `(() => {
            const label = String.fromCodePoint(${page.codepoints.join(',')});
            const root = [...document.querySelectorAll('aside button')]
              .find(b => (b.textContent || '').includes(label));
            if (!root) return 'no-root';
            const items = [...root.parentElement.querySelectorAll('button')].filter(b => b !== root);
            if (items[${page.categoryIndex}]) items[${page.categoryIndex}].click();
            return 'ok';
          })()`,
        })
      }
      await sleep(1200)

      const probe = await send('Runtime.evaluate', { expression: PROBE, returnByValue: true })
      let data
      try {
        data = JSON.parse(probe.result.value)
      } catch {
        data = { shown: false, parse: probe.result.value }
      }

      const shot = await send('Page.captureScreenshot', { format: 'png' })
      const withLayer = Buffer.from(shot.data, 'base64')
      await writeFile(path.join(OUT, `${size.name}-${page.name}.png`), withLayer)

      // Prove the artwork is painted, and measure how much of it survives the
      // page's own opaque surfaces.
      let painted = null
      let visibility = null
      if (data.shown) {
        await send('Runtime.evaluate', {
          expression: `(() => {
            const layer = document.querySelector('[class*="ariaWatermarkLayer"]');
            if (layer) layer.style.visibility = 'hidden';
            return true;
          })()`,
        })
        await sleep(320)
        const hiddenShot = await send('Page.captureScreenshot', { format: 'png' })
        const withoutLayer = Buffer.from(hiddenShot.data, 'base64')
        painted = hash(withLayer) !== hash(withoutLayer)
        visibility = await visibleFraction(withLayer, withoutLayer, data.box)
        await send('Runtime.evaluate', {
          expression: `(() => {
            const layer = document.querySelector('[class*="ariaWatermarkLayer"]');
            if (layer) layer.style.visibility = '';
            return true;
          })()`,
        })
        await sleep(180)
      }

      rows.push({ size: size.name, page: page.name, painted, visibility, ...data })
    }
  }

  socket.close()
  browser.kill()

  console.log('size       page             shown  painted  w    h  ratio  visible  in-window')
  let failures = 0
  for (const row of rows) {
    if (row.error) {
      failures += 1
      console.log(`${row.size.padEnd(10)} ${row.page.padEnd(16)} ${row.error}`)
      continue
    }
    const visible = row.visibility?.fraction
    /*
     * A page fails when the artwork exists but is painted nowhere, spills outside
     * the window, or is almost entirely hidden behind an opaque card. The last
     * case is a failure and not a note: a watermark that users never see is not a
     * working feature, it is a hidden file taking up 7 MB.
     */
    const hidden = row.shown && row.painted === false
    const spills = row.shown && row.insideWindow === false
    const buried = row.shown && typeof visible === 'number' && visible < 0.25
    const bad = hidden || spills || buried
    if (bad) failures += 1
    console.log(
      `${row.size.padEnd(10)} ${row.page.padEnd(16)} ${String(row.shown).padEnd(5)} ` +
      `${String(row.painted ?? '-').padEnd(8)} ${String(row.w ?? '-').padStart(4)} ${String(row.h ?? '-').padStart(4)} ` +
      `${String(row.ratio ?? '-').padStart(6)} ${String(visible ?? '-').padStart(7)} ` +
      `${String(row.insideWindow ?? '-').padStart(9)}` +
      (hidden ? '  <-- NOT PAINTED' : spills ? '  <-- SPILLS' : buried ? '  <-- BURIED' : ''),
    )
  }
  console.log(`\n${failures === 0 ? 'every page OK at every size' : failures + ' combination(s) failed'}`)
  console.log(OUT)
}

main().catch(error => {
  console.error(error)
  process.exit(1)
})
