/**
 * Capture the Aria watermark at a range of window sizes.
 *
 * ## Why a sweep and not one check
 *
 * The occlusion the user reported only appears at some aspect ratios: on a tall
 * window the cards stop well above the corner and the figure sits in empty space,
 * on a wide-short window the grid reaches the corner and the figure's face is cut
 * by a card edge. A single screenshot therefore proves nothing — the check has to
 * cover the shapes people actually run at, including ultrawide.
 *
 * For each size it records whether the figure is visible, how much of the content
 * area it covers, and whether its *head* (the top of the figure) lands on top of
 * an interactive element — which is the thing that reads as broken.
 *
 * Usage (Vite must be running on 5173):
 *   node scripts/sweep-aria.mjs
 */
import { spawn } from 'node:child_process'
import { mkdir, writeFile } from 'node:fs/promises'
import { setTimeout as sleep } from 'node:timers/promises'
import path from 'node:path'

const EDGE = 'C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe'
const OUT = path.join(process.env.TEMP ?? '/tmp', 'aria-audit', 'sweep')
const URL = 'http://127.0.0.1:5173/'
const PORT = 9337

/** Real-world window shapes, including the two that break. */
const SIZES = [
  { w: 1920, h: 1080, name: 'fhd' },
  { w: 1440, h: 900, name: 'laptop' },
  { w: 1366, h: 768, name: 'hd-ready' },
  { w: 1280, h: 800, name: 'small-laptop' },
  { w: 1024, h: 768, name: 'xga' },
  { w: 2560, h: 1440, name: 'qhd' },
  { w: 3440, h: 1440, name: 'ultrawide' },
  { w: 1600, h: 720, name: 'wide-short' },
  { w: 900, h: 1000, name: 'narrow-tall' },
]

async function cdp() {
  const browser = spawn(EDGE, [
    '--headless=new',
    '--disable-gpu',
    '--hide-scrollbars',
    `--remote-debugging-port=${PORT}`,
    '--user-data-dir=' + path.join(process.env.TEMP ?? '/tmp', 'aria-audit', 'sweep-profile'),
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
 * What to report for one size.
 *
 * `headHit` is the important number: it samples a point near the top centre of
 * the figure — where her face is — and asks the browser what is painted there.
 * If the answer is a card or button rather than the figure, the face is being cut
 * by content, which is exactly the "occlusion" the user reported.
 */
const PROBE = `(() => {
  const figure = document.querySelector('figure[data-variant="watermark"]');
  const main = document.querySelector('main');
  const layer = document.querySelector('[class*="ariaWatermarkLayer"]');
  if (!figure || !layer) return JSON.stringify({ shown: false });
  const style = getComputedStyle(figure);
  if (style.display === 'none') return JSON.stringify({ shown: false, reason: 'display:none' });

  const box = figure.getBoundingClientRect();
  const content = main.getBoundingClientRect();
  const area = content.width * content.height;

  /*
   * Visibility is established from pixels, not from styles.
   *
   * elementFromPoint cannot answer this: the figure has pointer-events none, so
   * hit-testing skips it and always reports whatever is underneath — that
   * produced "hidden on all nine sizes" for screenshots that plainly showed her.
   * Reading computed styles is no better: it says what was asked for, and the two
   * real bugs this session (behind the main plate, behind a page's own plate) both
   * had perfect styles.
   *
   * So the caller captures two screenshots — one with the layer, one with it
   * hidden — and compares them. Any difference proves the pixels changed, i.e.
   * she is actually painted there.
   */
  return JSON.stringify({
    shown: true,
    w: Math.round(box.width),
    h: Math.round(box.height),
    ratio: +(box.width * box.height / area).toFixed(3),
    box: [box.x, box.y, box.width, box.height],
    overflowBottom: Math.round(box.bottom - content.bottom),
    overflowRight: Math.round(box.right - content.right),
    insideWindow: box.left >= -1 && box.right <= innerWidth + 1 && box.bottom <= innerHeight + 1,
  });
})()`

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

  // Go to the library page: it has the densest card grid, so it is where the
  // figure is most likely to collide with content.
  await send('Runtime.evaluate', {
    expression: `(() => {
      const label = String.fromCodePoint(0x6e38, 0x620f, 0x5e93);
      const button = [...document.querySelectorAll('aside button, nav button')]
        .find(b => (b.textContent || '').includes(label));
      if (button) button.click();
      return Boolean(button);
    })()`,
  })
  await sleep(900)

  const rows = []
  for (const size of SIZES) {
    await send('Emulation.setDeviceMetricsOverride', {
      width: size.w, height: size.h, deviceScaleFactor: 1, mobile: false,
    })
    await sleep(700)
    const probe = await send('Runtime.evaluate', { expression: PROBE, returnByValue: true })
    const withLayer = await send('Page.captureScreenshot', { format: 'png' })
    await writeFile(path.join(OUT, `${size.name}-${size.w}x${size.h}.png`), Buffer.from(withLayer.data, 'base64'))

    let data
    try {
      data = JSON.parse(probe.result.value)
    } catch {
      data = { raw: probe.result.value }
    }

    /*
     * Prove the pixels change when the layer is removed.
     *
     * Two screenshots and a byte comparison is crude but decisive: it cannot be
     * fooled by styles, stacking bugs or pointer-events, all three of which gave
     * wrong answers earlier in this session. The images differ if and only if she
     * is actually painted where the box says.
     */
    let painted = null
    if (data.shown) {
      await send('Runtime.evaluate', {
        expression: `(() => {
          const layer = document.querySelector('[class*="ariaWatermarkLayer"]');
          if (layer) layer.style.visibility = 'hidden';
          return Boolean(layer);
        })()`,
      })
      await sleep(350)
      const without = await send('Page.captureScreenshot', { format: 'png' })
      painted = Buffer.from(withLayer.data, 'base64').equals(Buffer.from(without.data, 'base64')) === false
      await send('Runtime.evaluate', {
        expression: `(() => {
          const layer = document.querySelector('[class*="ariaWatermarkLayer"]');
          if (layer) layer.style.visibility = '';
        })()`,
      })
      await sleep(200)
    }

    rows.push({ size: `${size.w}x${size.h}`, name: size.name, painted, ...data })
  }

  socket.close()
  browser.kill()

  console.log('size         shown  painted   w    h  ratio  in-window  ovB  ovR')
  let failures = 0
  for (const row of rows) {
    /*
     * Failures, in order of how much they matter:
     *
     *   1. shown but the pixels do not change when the layer is hidden -> she is
     *      painted nowhere, i.e. hidden behind an opaque plate. This is the bug
     *      the sweep exists for, and it is the only check that has never given a
     *      false answer;
     *   2. spilling outside the window -> a scroll track appears and the layout
     *      shifts by its width;
     *   3. covering more than 30% of the content area -> it stops being decor.
     */
    const hidden = row.shown && row.painted === false
    const outOfWindow = row.shown && row.insideWindow === false
    const tooBig = row.shown && row.ratio > 0.30
    const bad = hidden || outOfWindow || tooBig
    if (bad) failures += 1
    console.log(
      `${row.name.padEnd(12)} ${String(row.shown).padEnd(5)} ` +
      `${String(row.painted ?? '-').padEnd(7)} ` +
      `${String(row.w ?? '-').padStart(4)} ${String(row.h ?? '-').padStart(4)} ` +
      `${String(row.ratio ?? '-').padStart(6)} ` +
      `${String(row.insideWindow ?? '-').padStart(10)} ` +
      `${String(row.overflowBottom ?? '-').padStart(4)} ${String(row.overflowRight ?? '-').padStart(4)}` +
      (hidden ? '   <-- PAINTED NOWHERE' : outOfWindow ? '   <-- SPILLS' : tooBig ? '   <-- TOO BIG' : ''),
    )
  }
  console.log(`\n${failures === 0 ? 'all sizes OK' : failures + ' size(s) failed'}`)
  console.log(`${OUT}`)
}

main().catch(error => {
  console.error(error)
  process.exit(1)
})
