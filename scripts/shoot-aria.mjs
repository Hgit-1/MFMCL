/* Headless verification of the per-page Aria artwork.
 *
 * ## Why this exists as a script rather than a manual check
 *
 * The watermark bug this session found was invisible in the DOM: the element was
 * present, had the right `src`, and the browser had fully decoded the image —
 * but a negative `z-index` painted it behind `.main`'s opaque background. No
 * assertion on markup would have caught it; only a screenshot would.
 *
 * So this drives a real browser and saves one frame per page, with the state
 * that a returning user would have: EULA accepted, the plugin enabled, and a
 * fixed season so the four seasons can be compared without waiting for months.
 *
 * It talks to Chrome DevTools Protocol over a raw WebSocket, because adding
 * Puppeteer to the project for one screenshot script would put a large
 * dependency tree in a workspace that deliberately keeps the frontend small.
 *
 * Usage (Vite must already be running on 5173):
 *   node scripts/shoot-aria.mjs
 */
import { spawn } from 'node:child_process'
import { mkdir, writeFile } from 'node:fs/promises'
import { setTimeout as sleep } from 'node:timers/promises'
import path from 'node:path'

const EDGE = 'C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe'
const OUT = path.join(process.env.TEMP ?? '/tmp', 'aria-audit', 'pages')
const URL = 'http://127.0.0.1:5173/'
const PORT = 9333

/** Pages to capture, in navigation order, with the season pinned for each run. */
const PAGES = [
  // Labels as Unicode code points: 游戏库 / 内容商店 / 下载任务 / 启动器插件.
  // Written numerically so the string cannot be mangled in transit.
  { name: 'library', codepoints: [0x6e38, 0x620f, 0x5e93] },
  { name: 'store', codepoints: [0x5185, 0x5bb9, 0x5546, 0x5e97] },
  { name: 'downloads', codepoints: [0x4e0b, 0x8f7d, 0x4efb, 0x52a1] },
  { name: 'plugins', codepoints: [0x542f, 0x52a8, 0x5668, 0x63d2, 0x4ef6] },
]
const SEASONS = ['spring', 'summer', 'autumn', 'winter']

async function cdp(url) {
  const browser = spawn(EDGE, [
    '--headless=new',
    '--disable-gpu',
    '--hide-scrollbars',
    `--remote-debugging-port=${PORT}`,
    '--user-data-dir=' + path.join(process.env.TEMP ?? '/tmp', 'aria-audit', 'profile'),
    '--no-first-run',
    '--window-size=1440,900',
    'about:blank',
  ], { stdio: 'ignore' })

  // Wait for the DevTools endpoint to answer.
  let target = null
  for (let attempt = 0; attempt < 60 && !target; attempt += 1) {
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

async function main() {
  await mkdir(OUT, { recursive: true })
  const { browser, socket, send } = await cdp(URL)

  await send('Page.enable')
  await send('Runtime.enable')
  await send('Emulation.setDeviceMetricsOverride', {
    width: 1440, height: 900, deviceScaleFactor: 1, mobile: false,
  })

  // Seed the state a returning user has, then get past the first-run screens.
  //
  // Three separate gates stand between a fresh profile and the main window, and
  // each one cost a round of debugging:
  //
  //   1. the EULA dialog (accepted, so it does not cover the page);
  //   2. the five-step onboarding wizard (skipped, by writing its "completed"
  //      record — clicking through it would depend on its Chinese labels);
  //   3. the Aria setting itself.
  //
  // Leaving out (2) made every capture a screenshot of the wizard, with the
  // nav clicks landing on its own buttons.
  const seed = `
    localStorage.setItem('mfmcl-eula-version', '1');
    localStorage.setItem('mfmcl-onboarding', JSON.stringify({ completedAt: Date.now() }));
    localStorage.setItem('mfmcl-aria', JSON.stringify({
      enabled: true, mode: 'fixed', fixedSeason: SEASON,
      southernHemisphere: false, showSidebar: true, showHero: true,
      showWatermark: true, tintSurfaces: true
    }));
    /*
     * The plugin store as well, not just the Aria setting.
     *
     * \`App.tsx\` has an effect that treats the plugin page's toggle as the source
     * of truth: it reads \`isPluginEnabled(plugin, pluginStore)\` and writes the
     * result into \`ariaSetting.enabled\`. Because the plugin ships with
     * \`enabledByDefault: false\`, seeding only \`mfmcl-aria\` had its flag
     * overwritten to \`false\` on the very next render — which is why every page
     * came back as "no figure" even though the setting on disk said enabled.
     */
    localStorage.setItem('flowlaunch-plugins', JSON.stringify({
      custom: [], enabled: { 'aria-with-you': true },
      options: {}, acknowledgedUpdates: {}
    }));
  `

  const shots = []
  for (const season of SEASONS) {
    // Set the season, then accept the agreement by clicking the button the way a
    // user does. Seeding localStorage and reloading proved unreliable: the write
    // raced the navigation, and the app came back up on the consent dialog.
    // Clicking is also what the first-run flow actually does, so the resulting
    // state is the one users end up in.
    await send('Page.navigate', { url: URL })
    await sleep(1800)
    await send('Runtime.evaluate', {
      expression: seed.replace('SEASON', `'${season}'`),
    })
    /*
     * Reload so the app boots from the seeded storage.
     *
     * `isOnboardingPending()` and `hasAcceptedEula()` are both read once, in a
     * `useState` initialiser during the first render. Writing the keys after that
     * render has no effect until the app reloads, which is why the earlier
     * version kept coming back to the wizard even with the keys present.
     */
    await send('Page.reload', { ignoreCache: false })
    await sleep(2600)

    // Confirm the app is on the main window before navigating: a capture taken
    // over the wizard or the dialog is worse than no capture, because it looks
    // like a working screenshot.
    /*
     * Wait for the sidebar to exist before probing or navigating.
     *
     * `nav: 0` here means the app had not finished mounting, not that the layout
     * changed — the same build renders eight nav items a moment later. A fixed
     * sleep was flaky under parallel load, so this polls until the sidebar is
     * actually there and only then proceeds.
     */
    let ready = null
    for (let attempt = 0; attempt < 40; attempt += 1) {
      const probe = await send('Runtime.evaluate', {
        expression: `JSON.stringify({
          nav: document.querySelectorAll('aside button, nav button').length,
          wizard: document.body.innerText.includes('跳过引导')
        })`,
        returnByValue: true,
      })
      ready = JSON.parse(probe.result.value)
      if (ready.nav > 0) break
      await sleep(250)
    }
    console.log(`  ${season}: ${JSON.stringify(ready)}`)

    for (const page of PAGES) {
      /*
       * Locate the nav button by its Chinese label, passed as escaped code
       * points.
       *
       * The label cannot be written literally in this file: the string makes a
       * PowerShell -> node -> file round trip and arrives mangled, which is why
       * text matching failed earlier while the DOM plainly had the button. Code
       * points survive any encoding, and the page reconstructs the same string
       * the DOM holds.
       */
      const clicked = await send('Runtime.evaluate', {
        expression: `(() => {
          const label = String.fromCodePoint(${page.codepoints.join(',')});
          const buttons = [...document.querySelectorAll('aside button, nav button, button')];
          const target = buttons.find(b => (b.textContent || '').includes(label));
          if (!target) return 'not-found';
          target.click();
          return 'clicked';
        })()`,
        returnByValue: true,
      })
      if (clicked.result.value !== 'clicked') {
        console.error(`  ${season}/${page.name}: ${clicked.result.value}`)
        continue
      }
      /*
       * The store's nav button only expands a sub-menu; the page itself opens
       * when one of its categories is picked. Clicking the expanding button and
       * capturing immediately photographed the *previous* page, which is why
       * `store` kept reporting `library`.
       *
       * The category label is 版本 here (the first entry). Its index inside the
       * sub-menu is stable and it is picked by position rather than text, since
       * every sub-menu entry is a Chinese word of similar shape.
       */
      if (page.name === 'store') {
        await sleep(500)
        await send('Runtime.evaluate', {
          expression: `(() => {
            const label = String.fromCodePoint(0x5185, 0x5bb9, 0x5546, 0x5e97);
            const root = [...document.querySelectorAll('aside button')]
              .find(b => (b.textContent || '').includes(label));
            if (!root) return 'no-root';
            const container = root.parentElement;
            const items = [...container.querySelectorAll('button')].filter(b => b !== root);
            if (!items.length) return 'no-items';
            items[0].click();
            return 'picked:' + items.length;
          })()`,
        })
        await sleep(1400)
      }
      await sleep(900)
      /*
       * Wait for the click to actually land before probing or capturing.
       *
       * React commits the page swap asynchronously, so a fixed sleep produced
       * captures that were one page behind: the log showed "spring-store" while
       * the figure still belonged to `library`. Polling the DOM for the figure's
       * `data-page` removes the guesswork, and costs nothing when the swap is
       * fast.
       */
      for (let attempt = 0; attempt < 20; attempt += 1) {
        const current = await send('Runtime.evaluate', {
          expression: `(() => {
            const figure = document.querySelector('figure[data-variant="watermark"]');
            return figure ? figure.dataset.page : 'none';
          })()`,
          returnByValue: true,
        })
        if (current.result.value === page.name) break
        await sleep(150)
      }

      // Report what the watermark actually resolved to, so a silent fallback to
      // the generic season art is visible in the log instead of being mistaken
      // for success.
      const probe = await send('Runtime.evaluate', {
        expression: `(() => {
          const figure = document.querySelector('figure[data-variant="watermark"]');
          if (!figure) return 'no-figure';
          const img = figure.querySelector('img');
          const rect = figure.getBoundingClientRect();
          const cx = rect.x + rect.width * 0.7;
          const cy = rect.y + rect.height * 0.55;
          const top = document.elementFromPoint(cx, cy);
          return JSON.stringify({
            page: figure.dataset.page,
            file: img.src.split('/').pop(),
            w: Math.round(rect.width),
            h: Math.round(rect.height),
            covered: Boolean(top && !figure.contains(top) && top !== figure),
            topTag: top ? top.tagName : null
          });
        })()`,
        returnByValue: true,
      })

      const shot = await send('Page.captureScreenshot', { format: 'png' })
      const file = path.join(OUT, `${season}-${page.name}.png`)
      await writeFile(file, Buffer.from(shot.data, 'base64'))
      shots.push(`${season}-${page.name}: ${probe.result.value}`)
    }
  }

  socket.close()
  browser.kill()
  console.log(shots.join('\n'))
  console.log(`\n${OUT}`)
}

main().catch(error => {
  console.error(error)
  process.exit(1)
})
