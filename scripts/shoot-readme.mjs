/* Produce the screenshots embedded in README.md.
 *
 * ## Why a script instead of dragging files out of a window
 *
 * The README is the first thing anyone sees, and screenshots that disagree with
 * the build are worse than none: a stale picture promises a feature that moved or
 * vanished. Regenerating them with one command means "update the pictures" is a
 * step in a release checklist rather than a manual chore someone skips.
 *
 * ## Same CDP approach as `shoot-aria.mjs`, and for the same reason
 *
 * Puppeteer would add a large dependency tree to a workspace that deliberately
 * keeps the frontend small. The three calls this needs — navigate, evaluate,
 * capture — are a few dozen lines over a raw WebSocket.
 *
 * ## What it deliberately does not do
 *
 * It does not fake data. The screenshots show whatever the app shows in a fresh
 * browser profile, which means the preview dataset the frontend already ships
 * (`PREVIEW_INSTANCES` and friends). Nothing here injects a fake "installed"
 * state that a real user would never see, because a README screenshot is a claim.
 *
 * Usage (Vite must be running on 5173):
 *   node scripts/shoot-readme.mjs
 */
import { spawn } from 'node:child_process'
import { mkdir, writeFile } from 'node:fs/promises'
import { setTimeout as sleep } from 'node:timers/promises'
import path from 'node:path'

const EDGE = 'C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe'
const OUT = path.join(process.cwd(), 'docs', 'screenshots')
/*
 * Named `PAGE_URL`, not `URL`: a top-level `const URL` shadows the global `URL`
 * constructor, and the artwork interceptor needs `new URL(...)` to read a
 * request's file name. The failure was silent — every interception threw and the
 * script kept going, producing exactly the figure-less screenshots this helper
 * exists to prevent.
 */
const PAGE_URL = 'http://127.0.0.1:5173/'
const PORT = 9344

/*
 * Screenshots to capture. Labels are code points because a Chinese literal in
 * this file survives a PowerShell -> node round trip as mojibake; the page
 * rebuilds the same string and the DOM match then works.
 */
const SHOTS = [
  {
    name: 'home',
    nav: null, // already the default page
    caption: 'The home page. One instance, one button, and Aria showing the season.',
  },
  {
    name: 'library',
    nav: [0x6e38, 0x620f, 0x5e93], // 游戏库
    caption: 'The library: every instance as a card with its state, size and last launch.',
  },
  {
    name: 'store',
    nav: [0x5185, 0x5bb9, 0x5546, 0x5e97], // 内容商店
    /*
     * The store nav item only *expands* a submenu; the page itself is a child
     * entry. Clicking just the parent captured the library with an open drawer,
     * which is not a screenshot of the store at all.
     */
    then: [0x4d, 0x6f, 0x64], // Mod — the category that shows both sources
    caption: 'The content store, searching Modrinth and CurseForge together.',
  },
  {
    name: 'plugins',
    nav: [0x542f, 0x52a8, 0x5668, 0x63d2, 0x4ef6], // 启动器插件
    caption: 'The plugin page: install from a package, from a URL, or from a source index.',
  },
  {
    name: 'downloads',
    nav: [0x4e0b, 0x8f7d, 0x4efb, 0x52a1], // 下载任务
    caption: 'Download tasks, each showing whole-task progress rather than one file at a time.',
  },
]

/** Pin the light theme and a fixed season so the set looks consistent. */
const SEED = `
  localStorage.setItem('mfmcl-eula-version', '1');
  localStorage.setItem('mfmcl-onboarding', JSON.stringify({ completedAt: Date.now() }));
  localStorage.setItem('mfmcl-aria', JSON.stringify({
    enabled: true, mode: 'fixed', fixedSeason: 'spring',
    southernHemisphere: false, showSidebar: true, showHero: true,
    showWatermark: true, tintSurfaces: true
  }));
  localStorage.setItem('flowlaunch-plugins', JSON.stringify({
    custom: [], enabled: { 'aria-with-you': true, 'launch-logger': true, 'download-mirror': true },
    options: {}, acknowledgedUpdates: {}
  }));
`

async function cdp() {
  const browser = spawn(EDGE, [
    '--headless=new',
    '--disable-gpu',
    '--hide-scrollbars',
    `--remote-debugging-port=${PORT}`,
    '--user-data-dir=' + path.join(process.env.TEMP ?? '/tmp', 'readme-shots', 'profile'),
    '--no-first-run',
    '--window-size=1440,900',
    'about:blank',
  ], { stdio: 'ignore' })

  let target = null
  for (let attempt = 0; attempt < 60 && !target; attempt += 1) {
    await sleep(250)
    try {
      const response = await fetch(`http://127.0.0.1:${PORT}/json/list`)
      const list = await response.json()
      target = list.find(item => item.type === 'page') ?? null
    } catch {
      // devtools not up yet
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
  const listeners = new Set()
  socket.onmessage = event => {
    const message = JSON.parse(event.data)
    if (message.id && pending.has(message.id)) {
      const { resolve, reject } = pending.get(message.id)
      pending.delete(message.id)
      if (message.error) reject(new Error(JSON.stringify(message.error)))
      else resolve(message.result)
      return
    }
    // CDP events have no id; hand them to whoever subscribed. Errors inside a
    // listener must not escape: an unhandled rejection here kills the process
    // mid-capture, which reads as "the script is broken" rather than "one
    // request was not fulfilled".
    if (message.method) {
      for (const listener of listeners) {
        try {
          const outcome = listener(message)
          if (outcome && typeof outcome.catch === 'function') {
            outcome.catch(error => console.warn(`  listener: ${error.message}`))
          }
        } catch (error) {
          console.warn(`  listener: ${error.message}`)
        }
      }
    }
  }
  const send = (method, params = {}) => new Promise((resolve, reject) => {
    const id = nextId++
    pending.set(id, { resolve, reject })
    socket.send(JSON.stringify({ id, method, params }))
  })
  const on = listener => listeners.add(listener)

  return { browser, socket, send, on }
}

/**
 * Serve the remote artwork from disk for the duration of the capture.
 *
 * ## Why this is necessary, and why it is still honest
 *
 * The artwork lives in a separate repository and is fetched from jsDelivr at
 * runtime (`utils/aria.ts::ART_BASE`). That is the right design — it decouples
 * season changes from launcher releases — but it makes a screenshot depend on a
 * CDN round trip, and this machine's network resets connections to both
 * `cdn.jsdelivr.net` and `raw.githubusercontent.com`.
 *
 * Fulfilling those requests from the plugin repository's working tree shows the
 * interface exactly as a user with a working connection sees it. Nothing is
 * invented: every byte served is the file at the pinned tag.
 *
 * The interception is narrow on purpose. Only requests under the artwork CDN
 * prefix are touched, so a bug in this helper cannot mask a real problem in the
 * app's own assets.
 */
async function serveArtworkLocally(send, on, repositoryRoot) {
  const { readFile } = await import('node:fs/promises')
  const base = path.join(repositoryRoot, 'assets')

  on(async message => {
    if (message.method !== 'Fetch.requestPaused') return
    const { requestId, request } = message.params
    const name = path.basename(new URL(request.url).pathname)
    try {
      const bytes = await readFile(path.join(base, name))
      await send('Fetch.fulfillRequest', {
        requestId,
        responseCode: 200,
        responseHeaders: [
          { name: 'Content-Type', value: 'image/webp' },
          { name: 'Access-Control-Allow-Origin', value: '*' },
        ],
        body: bytes.toString('base64'),
      })
    } catch {
      // Not one of ours, or absent: let it fail exactly as it would in
      // production rather than substituting a placeholder.
      await send('Fetch.continueRequest', { requestId }).catch(() => {})
    }
  })

  await send('Fetch.enable', {
    patterns: [{ urlPattern: '*cdn.jsdelivr.net/gh/Hgit-1/mfmcl-aria-with-you*' }],
  })
}

/**
 * Click a nav item by its label code points.
 *
 * Matched by **prefix**, not equality: two of the nav buttons carry a badge
 * (`下载任务，1 个进行中`, `游戏库 6`), so their `textContent` is the label plus a
 * count. An exact match silently skipped exactly the two pages that have live
 * data — which is the opposite of what a README wants to show.
 *
 * The prefix test is safe here because the labels are distinct at their first
 * characters; `游戏库` and `游戏` would not be, but that pairing does not exist.
 */
async function clickNav(send, codepoints) {
  const result = await send('Runtime.evaluate', {
    expression: `(() => {
      const label = String.fromCodePoint(${codepoints.join(',')});
      const button = [...document.querySelectorAll('button')]
        .find(item => item.textContent.trim().startsWith(label));
      if (!button) return 'missing';
      button.click();
      return 'clicked';
    })()`,
    returnByValue: true,
  })
  return result.result.value === 'clicked'
}

async function main() {
  await mkdir(OUT, { recursive: true })
  const { browser, socket, send, on } = await cdp()

  /*
   * Where the artwork repository is checked out.
   *
   * Required rather than guessed: a relative guess depends on how the two
   * repositories happen to sit next to each other, and guessing wrong would
   * silently serve nothing and produce screenshots with no figures — which looks
   * like a broken feature instead of a broken script.
   */
  const artworkRoot = process.env.MFMCL_ARIA_REPO
  if (!artworkRoot) {
    console.error('Set MFMCL_ARIA_REPO to the checkout of Hgit-1/mfmcl-aria-with-you')
    process.exitCode = 2
    return
  }

  await send('Page.enable')
  await send('Runtime.enable')
  await send('Emulation.setDeviceMetricsOverride', {
    width: 1440, height: 900, deviceScaleFactor: 2, mobile: false,
  })
  await serveArtworkLocally(send, on, artworkRoot)

  // Boot once, seed the returning-user state, then reload so the app reads it.
  // Those keys are read in `useState` initialisers, so writing them after the
  // first render has no effect until the next load.
  await send('Page.navigate', { url: PAGE_URL })
  await sleep(1800)
  await send('Runtime.evaluate', { expression: SEED })
  await send('Page.reload', { ignoreCache: false })
  await sleep(2800)

  // Wait for the shell rather than sleeping a fixed time: a capture taken over
  // the onboarding wizard looks exactly like a broken app.
  let mounted = false
  for (let attempt = 0; attempt < 40 && !mounted; attempt += 1) {
    const probe = await send('Runtime.evaluate', {
      expression: `document.querySelectorAll('aside button, nav button').length`,
      returnByValue: true,
    })
    mounted = (probe.result.value ?? 0) > 0
    if (!mounted) await sleep(250)
  }
  if (!mounted) throw new Error('app never mounted; check the dev server')

  const written = []
  for (const shot of SHOTS) {
    if (shot.nav) {
      const clicked = await clickNav(send, shot.nav)
      if (!clicked) {
        console.warn(`  ${shot.name}: nav item not found, skipping`)
        continue
      }
      // A drawer has to open before its child entry exists in the DOM.
      if (shot.then) {
        await sleep(500)
        const entered = await clickNav(send, shot.then)
        if (!entered) {
          console.warn(`  ${shot.name}: sub-item not found, capturing the parent`)
        }
      }
      // Let the page transition and any lazy image finish decoding. 900 ms is
      // enough for the 260 ms page-in animation plus an async image decode.
      await sleep(900)
    } else {
      await sleep(400)
    }

    const capture = await send('Page.captureScreenshot', {
      format: 'png',
      captureBeyondViewport: false,
    })
    const file = path.join(OUT, `${shot.name}.png`)
    await writeFile(file, Buffer.from(capture.data, 'base64'))
    written.push({ ...shot, file })
    console.log(`  ${shot.name}: ${file}`)
  }

  console.log(`\n${written.length}/${SHOTS.length} captured into ${OUT}`)
  socket.close()
  browser.kill()
}

main().catch(error => {
  console.error(error)
  process.exitCode = 1
})
