// 崩溃分析页的前端逻辑。
//
// ## 一个原则：结果的来源必须可见
//
// 「规则匹配到内存不足」和「1.7B 模型猜可能是内存不足」是完全不同的两件事，
// 但它们在界面上很容易长得一样。所以每个结论都带来源标记，模型单独给出的推测
// 用虚线框和「推测」标签区分，永远不写法同规则命中的样式。
//
// 同理，模型挂了不能说「分析失败」——规则结论仍然有效，只是少了一段解释。

const $ = id => document.getElementById(id);

let lastAnalysis = null;

// --- 状态条 ---------------------------------------------------------------

/**
 * 检查模型是否在线。
 *
 * 打开页面就问，而不是等到第一次分析才发现模型没起。用户看到一个「模型未连接」
 * 的标记，就知道该启动 llama.cpp，而不是对着一句「分析失败」猜。
 */
/**
 * 列出本机崩溃报告。
 *
 * 服务端只扫描它被允许的目录（`MFMCL_MINECRAFT_DIR` 与 `~/.minecraft`）。读不到
 * 就把整块藏起来——显示一个空下拉框会让用户以为功能坏了。
 */
async function loadReports() {
  try {
    const response = await fetch('/api/sources');
    const data = await response.json();
    const reports = data.reports || [];
    if (reports.length === 0) return;
    const select = $('report-select');
    for (const report of reports) {
      const option = document.createElement('option');
      option.value = report.path;
      const size = formatBytes(report.bytes);
      option.textContent = `${report.instance} · ${report.name} (${size})`;
      select.appendChild(option);
    }
    $('reports').hidden = false;
  } catch {
    // 服务端不支持时静默：用户还可以粘贴日志。
  }
}

async function loadReport(path) {
  if (!path) return;
  try {
    const response = await fetch('/api/read', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ path }),
    });
    const data = await response.json();
    if (!response.ok) {
      $('log').value = `无法读取：${data.error || response.status}`;
      updateLogMeta();
      return;
    }
    $('log').value = data.content;
    updateLogMeta();
  } catch (error) {
    $('log').value = `无法读取：${error}`;
    updateLogMeta();
  }
}

async function checkHealth() {
  const status = $('status');
  const text = $('status-text');
  status.dataset.state = 'checking';
  text.textContent = '正在检查模型…';
  try {
    const response = await fetch('/api/health');
    const data = await response.json();
    const feasibility = data.feasibility || {};
    const modelName = data.model || '';

    /*
     * 三种状态必须分开说，因为修复方式完全不同：
     *   - 机器跑不动 → 不用找模型服务了，这台电脑就是带不动
     *   - 熔断     → 之前试过失败，已自动停用
     *   - 服务没起 → 去启动 llama.cpp
     */
    if (feasibility.feasibility === 'notViable') {
      status.dataset.state = 'down';
      text.textContent = '这台电脑跑不动本地模型';
    } else if (data.disabledReason) {
      status.dataset.state = 'down';
      text.textContent = '本地模型已自动停用';
    } else if (data.ok) {
      status.dataset.state = 'ok';
      text.textContent = feasibility.feasibility === 'slow'
        ? `模型就绪 · ${modelName}（CPU 推理，较慢）`
        : `模型就绪 · ${modelName}`;
      $('foot-model').textContent = `${modelName} @ ${data.endpoint}`;
    } else {
      status.dataset.state = 'down';
      text.textContent = '模型未连接（仍可只用规则分析）';
      $('foot-model').textContent = data.endpoint || '';
    }

    renderModelStatus(feasibility, data.disabledReason);
  } catch {
    status.dataset.state = 'down';
    text.textContent = '无法连接分析服务';
  }
}

// --- 输入 -----------------------------------------------------------------

function updateLogMeta() {
  const value = $('log').value;
  const lines = value ? value.split('\n').length : 0;
  $('log-meta').textContent = value
    ? `${lines.toLocaleString()} 行 · ${formatBytes(new Blob([value]).size)}`
    : '尚未载入日志';
}

function formatBytes(bytes) {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}

async function readFile(file) {
  if (!file) return;
  const text = await file.text();
  $('log').value = text;
  updateLogMeta();
}

/**
 * 拖放。
 *
 * 阻止默认行为是必须的：不拦 dragover 的话浏览器会用文件内容替换整个页面，
 * 用户会以为把界面弄坏了。
 */
function wireDrop() {
  const drop = $('drop');
  const hint = $('drop-hint');
  ['dragenter', 'dragover'].forEach(type => {
    drop.addEventListener(type, event => {
      event.preventDefault();
      hint.hidden = false;
    });
  });
  ['dragleave', 'drop'].forEach(type => {
    drop.addEventListener(type, event => {
      event.preventDefault();
      hint.hidden = true;
    });
  });
  drop.addEventListener('drop', event => {
    const file = event.dataTransfer?.files?.[0];
    if (file) void readFile(file);
  });
}

// --- 分析 -----------------------------------------------------------------

async function analyse() {
  const log = $('log').value;
  if (!log.trim()) {
    $('log').focus();
    return;
  }

  const button = $('analyse');
  const hint = $('analyse-hint');
  const useModel = $('use-model').checked;

  button.disabled = true;
  button.textContent = useModel ? '分析中…' : '分析中…';
  hint.textContent = useModel ? '模型正在阅读日志，通常需要几秒' : '正在匹配规则';
  showSkeleton();

  const environment = {};
  const game = $('env-game').value.trim();
  const java = $('env-java').value.trim();
  const loader = $('env-loader').value.trim();
  const memory = Number($('env-memory').value);
  if (game) environment.gameVersion = game;
  if (java) environment.javaVersion = java;
  if (loader) environment.loader = loader;
  if (memory > 0) environment.memoryMb = memory;

  const started = performance.now();
  try {
    await streamAnalyse({ log, environment, useModel }, started);
  } catch (error) {
    renderError(String(error));
  } finally {
    button.disabled = false;
    button.textContent = '开始分析';
    hint.textContent = '规则分析是即时的；开启模型后需要几秒到一分钟';
  }
}

/**
 * 流式分析。
 *
 * ## 为什么不用 EventSource
 *
 * `EventSource` 只能发 GET，而日志内容有几 MB —— 放 URL 里不可能。所以用
 * `fetch` + 手动读 `response.body` 的流。代价是要自己解析 SSE 帧。
 *
 * ## 分两段渲染是核心
 *
 * 规则结论（`rules` 事件）在毫秒级到达，先画出来；模型增量（`chunk`）随后逐段
 * 追加到「为什么」区域。这样用户在第一秒就看到「是什么问题」，之后是在读内容，
 * 而不是盯着骨架屏猜是否卡死。
 */
async function streamAnalyse(payload, started) {
  /*
   * 第一段：只跑本地部分。
   *
   * ## 为什么值得多发一次请求
   *
   * 服务端把整条 SSE 一次写回，意味着模型生成完毕之前，浏览器连响应头都收不到
   * ——用户仍然要等 6 秒才看到规则结论。实测确认了这一点（6106 ms 后一次性出现）。
   *
   * 规则分析只要 4–14 毫秒，所以先单独要一份：用户在第一秒就看到「是什么问题」，
   * 之后模型逐字补上解释。这次请求的代价与收益完全不成比例。
   */
  try {
    const quick = await fetch('/api/analyse', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ ...payload, useModel: false }),
    });
    if (quick.ok) {
      const rules = await quick.json();
      renderStreaming(rules, '', null, null, Math.round(performance.now() - started));
    }
  } catch {
    // 快路径失败不影响后面的完整分析。
  }

  const response = await fetch('/api/analyse/stream', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(payload),
  });
  if (!response.ok) {
    let message = `HTTP ${response.status}`;
    try { message = (await response.json()).error || message; } catch { /* 忽略 */ }
    renderError(message);
    return;
  }

  const reader = response.body.getReader();
  const decoder = new TextDecoder();
  let buffer = '';
  let rules = null;
  let text = '';
  let done = null;
  let error = null;

  const paint = () => {
    if (!rules) return;
    renderStreaming(rules, text, done, error, Math.round(performance.now() - started));
  };

  while (true) {
    const { value, done: finished } = await reader.read();
    if (finished) break;
    buffer += decoder.decode(value, { stream: true });

    // SSE 以空行分隔事件。
    let index;
    while ((index = buffer.indexOf('\n\n')) >= 0) {
      const block = buffer.slice(0, index);
      buffer = buffer.slice(index + 2);

      let name = 'message';
      const dataLines = [];
      for (const line of block.split('\n')) {
        if (line.startsWith('event: ')) name = line.slice(7).trim();
        else if (line.startsWith('data: ')) dataLines.push(line.slice(6));
      }
      if (dataLines.length === 0) continue;

      let payloadData;
      try {
        payloadData = JSON.parse(dataLines.join('\n'));
      } catch {
        continue;
      }

      if (name === 'rules') { rules = payloadData; paint(); }
      else if (name === 'chunk') { text += payloadData.text || ''; paint(); }
      else if (name === 'done') { done = payloadData; paint(); }
      else if (name === 'error') { error = payloadData; paint(); }
    }
  }

  // 流结束但没收到 done（连接中断）：仍然显示已经拿到的部分，并说明不完整。
  if (!done && !error) {
    error = { error: '连接中断，模型输出可能不完整', modelUsed: false };
  }
  paint();
  if (rules) lastAnalysis = { ...rules, ...(done || {}) };
}

/**
 * 渲染流式结果。
 *
 * 与 `render()` 的区别只有一个：模型解释是**持续增长**的文本，所以它在 DOM 里
 * 必须有稳定的位置，不能每次重画整块——否则用户每收到一个字就被滚动位置打断。
 */
function renderStreaming(rules, text, done, error, elapsedMs) {
  const result = $('result');
  result.className = 'result';

  const analysis = {
    findings: rules.findings || [],
    cause: rules.cause,
    capability: rules.capability,
    facts: rules.facts,
    modelStatus: rules.modelStatus,
    warnings: rules.warnings || [],
  };

  const hardware = $('hardware');
  const hardwareHtml = renderHardware(analysis.capability);
  if (hardwareHtml) {
    hardware.hidden = false;
    hardware.outerHTML = hardwareHtml.replace('class="hardware"', 'class="hardware" id="hardware"');
  }

  const parts = [];
  $('result-meta').textContent = `${(analysis.facts?.totalLines || 0).toLocaleString()} 行 · ${elapsedMs} ms`;
  parts.push(renderCause(analysis.cause));

  if (analysis.findings.length === 0 && (!analysis.cause || analysis.cause.kind === 'unknown')) {
    parts.push(renderNoneBlock());
  } else {
    if (analysis.findings.length) {
      const strongest = CONFIDENCE_LABEL[analysis.findings[0].confidence] || '';
      parts.push(`<p class="lead">${strongest}的原因是：<strong>${escapeHtml(analysis.findings[0].title)}</strong></p>`);
    }
    parts.push(analysis.findings.map(renderFinding).join(''));
  }

  /*
   * 模型区域。
   *
   * 三种状态各有明确的视觉：
   *   - 正在生成：光标闪烁 + 已到达的文字
   *   - 完成：换成解析后的结构（步骤来自规则）
   *   - 失败：说明失败原因，规则结论仍然在
   */
  if (done?.narrative) {
    parts.push(renderNarrative(done.narrative, analysis.findings.length === 0));
  } else if (error) {
    parts.push(`<div class="banner">${escapeHtml(error.error || '模型分析失败')}</div>`);
  } else if (text) {
    parts.push(`<article class="narrative streaming">
      <header>
        <span class="badge model">模型解释</span>
        <h3>为什么</h3>
        <span class="stream-cursor" aria-hidden="true"></span>
      </header>
      <p class="stream-text">${escapeHtml(text)}</p>
    </article>`);
  } else if (rules.modelStatus && rules.modelStatus.disableModel) {
    // 模型本来就不该跑：不显示「正在思考」，那会误导。
  } else {
    parts.push(`<article class="narrative streaming">
      <header>
        <span class="badge model">模型解释</span>
        <h3>为什么</h3>
        <span class="stream-cursor" aria-hidden="true"></span>
      </header>
      <p class="stream-pending">正在阅读日志…</p>
    </article>`);
  }

  result.innerHTML = parts.join('');
}

function renderNoneBlock() {
  return `<div class="none">
      <strong>没有匹配到已知的崩溃特征</strong>
      <p>这不一定说明没问题：可能是较少见的失败方式，或者日志里缺少关键部分。
      把 <code>crash-reports</code> 里的完整报告贴进来通常更准。</p>
    </div>`;
}

function showSkeleton() {
  $('result').className = 'result';
  $('result').innerHTML = '<div class="skeleton"></div><div class="skeleton short"></div>';
  $('result-meta').textContent = '';
}

function renderError(message) {
  $('result').className = 'result';
  $('result-meta').textContent = '';
  $('result').innerHTML = `<p class="error">${escapeHtml(message)}</p>`;
}

// --- 渲染 -----------------------------------------------------------------

const CONFIDENCE_LABEL = {
  high: '很可能',
  medium: '可能',
  low: '不确定',
  guess: '推测',
};

const VERDICT_LABEL = {
  hopeless: '这台电脑无法运行这个整合包',
  tight: '这台电脑比较吃力',
  adequate: '这台电脑够用',
  ample: '这台电脑绰绰有余',
};

/**
 * 硬件结论。
 *
 * 「彻底带不动」是这里唯一会阻止用户继续调设置的信息，所以它必须显眼，
 * 并且给出的是「为什么不可能」而不是「可能有点慢」。
 */
function renderHardware(capability) {
  if (!capability || !capability.detected) return '';
  const verdict = capability.verdict;
  const rows = capability.assessments
    .map(item => `<div class="hardware-row">
        <b>${escapeHtml(resourceLabel(item.resource))}</b>
        <span class="measured">${escapeHtml(item.measured)}</span>
        <span>／ 需要 ${escapeHtml(item.required)}</span>
      </div>`)
    .join('');
  const advice = (capability.advice || []).length
    ? `<ul class="hardware-advice">${capability.advice.map(a => `<li>${escapeHtml(a)}</li>`).join('')}</ul>`
    : '';
  return `<div class="hardware" data-verdict="${verdict}">
      <div class="hardware-head">${escapeHtml(VERDICT_LABEL[verdict] || verdict)}</div>
      ${rows}
      ${advice}
    </div>`;
}

/**
 * 模型本身的状态。
 *
 * 这一块回答的是「这台电脑能不能跑模型」，与上面的硬件块（能不能跑游戏）是
 * 两件事。只有在本机跑不动、或已熔断时才显示——正常情况下它不该占用版面。
 */
function renderModelStatus(feasibility, disabledReason) {
  const box = $('model-status');
  if (!box) return;
  const notViable = feasibility && feasibility.feasibility === 'notViable';
  if (!notViable && !disabledReason) {
    box.hidden = true;
    return;
  }
  box.hidden = false;
  box.dataset.state = notViable ? 'blocked' : 'tripped';
  const title = notViable ? '本地模型已停用（这台电脑跑不动）' : '本地模型已自动停用';
  const detail = notViable ? feasibility.summary : (disabledReason || '');
  const reason = notViable ? feasibility.reason : '';
  box.innerHTML = `
    <div class="model-status-head">${escapeHtml(title)}</div>
    <p>${escapeHtml(detail)}</p>
    ${reason ? `<p class="model-status-reason">${escapeHtml(reason)}</p>` : ''}
    <p class="model-status-note">规则分析不受影响，仍然可用。</p>`;
}

function resourceLabel(resource) {
  return {
    memory: '物理内存',
    heap: '分配的堆内存',
    cpu: 'CPU',
    gpu: '显卡',
    disk: '磁盘',
    display: '显示环境',
  }[resource] || resource;
}

/** 因果结论：从堆栈里读出来的「是什么问题」。 */
function renderCause(cause) {
  if (!cause || cause.kind === 'unknown') return '';
  const mods = (cause.modFailures || []).slice(0, 8);
  const modList = mods.length
    ? `<div class="mod-failures">${mods
        .map(m => `<div><code>${escapeHtml(m.modId)}</code> <span>${escapeHtml(m.message)}</span></div>`)
        .join('')}</div>`
    : '';
  return `<div class="cause-bar">
      <div>
        <strong>${escapeHtml(reasonHeadline(cause))}</strong>
        <p>${escapeHtml(cause.reason)}</p>
        ${modList}
      </div>
    </div>`;
}

function reasonHeadline(cause) {
  return {
    systemLimit: '问题在操作系统，不在游戏',
    outOfMemory: '游戏内存不足',
    jvmArguments: 'JVM 启动参数无效',
    javaVersion: 'Java 版本不匹配',
    gameFiles: '游戏文件缺失或损坏',
    graphics: '图形栈/显卡问题',
    mods: '模组加载失败',
    nativeLibrary: '缺少系统依赖库',
    fileAccess: '文件访问被拒绝',
    externalKill: '进程被外部终止',
    noDisplay: '没有可用的图形会话',
  }[cause.kind] || '未知原因';
}

function render(data, elapsedMs) {
  const result = $('result');
  result.className = 'result';

  // 硬件状态放在最上面：如果机器根本带不动，其他内容都不重要。
  const hardware = $('hardware');
  const hardwareHtml = renderHardware(data.capability);
  if (hardwareHtml) {
    hardware.hidden = false;
    hardware.outerHTML = hardwareHtml.replace('class="hardware"', 'class="hardware" id="hardware"');
  }

  const parts = [];

  // 因果结论先于规则清单：它是权威结论，规则是它的展开。
  parts.push(renderCause(data.cause));

  // 概要：一句话说清有多少条结论、几行日志、用了多少时间。
  const findings = data.findings || [];
  const strongest = findings.length ? CONFIDENCE_LABEL[findings[0].confidence] || '' : '';
  $('result-meta').textContent =
    `${data.facts.totalLines.toLocaleString()} 行 · ${elapsedMs} ms`;

  // 模型缺失的说明放在最前面：用户需要先知道「这是一份不完整的分析」。
  if (!data.modelUsed) {
    parts.push(`<div class="banner">${escapeHtml(
      (data.warnings && data.warnings[0]) || '模型没有参与本次分析，以下只是规则匹配的结果。',
    )}</div>`);
  }

  if (findings.length === 0 && (!data.cause || data.cause.kind === 'unknown')) {
    parts.push(`
      <div class="none">
        <strong>没有匹配到已知的崩溃特征</strong>
        <p>这不一定说明没问题：可能是较少见的失败方式，或者日志里缺少关键部分。
        把 <code>crash-reports</code> 里的完整报告贴进来通常更准。</p>
      </div>`);
  } else {
    parts.push(`<p class="lead">${strongest}的原因是：<strong>${escapeHtml(findings[0].title)}</strong></p>`);
    parts.push(findings.map(renderFinding).join(''));
  }

  if (data.narrative) {
    parts.push(renderNarrative(data.narrative, findings.length === 0));
  }

  if (data.facts.exception) {
    parts.push(`
      <details class="evidence-block">
        <summary>识别到的异常</summary>
        <pre>${escapeHtml(data.facts.exception)}</pre>
      </details>`);
  }

  if (data.facts.involvedMods && data.facts.involvedMods.length) {
    parts.push(`
      <details class="evidence-block">
        <summary>涉及的模组</summary>
        <div class="chips">${data.facts.involvedMods
          .map(name => `<span class="chip">${escapeHtml(name)}</span>`)
          .join('')}</div>
      </details>`);
  }

  result.innerHTML = parts.join('');
}

function renderFinding(finding) {
  const label = CONFIDENCE_LABEL[finding.confidence] || finding.confidence;
  const suggestions = (finding.suggestions || [])
    .map(step => `<li>${escapeHtml(step)}</li>`)
    .join('');
  const evidence = (finding.evidence || [])
    .map(line => `<pre>${escapeHtml(line)}</pre>`)
    .join('');

  return `
    <article class="finding conf-${finding.confidence}">
      <header>
        <span class="badge">${escapeHtml(label)}</span>
        <h3>${escapeHtml(finding.title)}</h3>
        <code class="rule-code">${escapeHtml(finding.code)}</code>
      </header>
      <p>${escapeHtml(finding.explanation)}</p>
      ${suggestions ? `<ol class="steps">${suggestions}</ol>` : ''}
      ${evidence ? `<details class="inline-evidence"><summary>依据的日志行</summary>${evidence}</details>` : ''}
    </article>`;
}

/**
 * 模型的解释。
 *
 * 当规则什么都没找到时，用虚线框和「推测」标签明确区分：这时模型说的是猜测，
 * 用户不该按规则命中的严肃程度对待它。
 *
 * 步骤来源同样要标出来。实测这个 1.7B 模型在内存溢出日志上给出的步骤是
 * 「降低堆内存到 512 MB」——正好是把崩溃变成必然的方向。所以规则命中时步骤一律
 * 来自规则（人工整理过、有测试覆盖），模型只负责解释「为什么」；只有在规则完全
 * 没匹配时才用模型的建议，并把整块标为推测。
 */
function renderNarrative(narrative, speculative) {
  const steps = (narrative.steps || [])
    .map(step => `<li>${escapeHtml(step)}</li>`)
    .join('');
  const sourceNote = narrative.source === 'rules'
    ? '<p class="source-note">步骤来自匹配到的规则，不是模型生成的。</p>'
    : '<p class="source-note">步骤由模型给出，未经规则验证，请自行判断。</p>';
  return `
    <article class="narrative${speculative ? ' speculative' : ''}">
      <header>
        <span class="badge model">${speculative ? '模型推测' : '模型解释'}</span>
        <h3>为什么</h3>
        <code class="rule-code">${escapeHtml(narrative.model)}</code>
      </header>
      <p>${escapeHtml(narrative.explanation)}</p>
      ${steps ? `<ol class="steps">${steps}</ol>${sourceNote}` : ''}
    </article>`;
}

function escapeHtml(value) {
  return String(value ?? '').replace(/[&<>"']/g, character => ({
    '&': '&amp;',
    '<': '&lt;',
    '>': '&gt;',
    '"': '&quot;',
    "'": '&#39;',
  }[character]));
}

// --- 示例日志 -------------------------------------------------------------

const SAMPLE = `[19:45:30] [main/INFO]: Loading Minecraft 1.20.1 with Forge 47.2.20
[19:45:31] [main/INFO]: Loading 186 mods
[19:50:12] [Render thread/INFO]: Preparing spawn area: 87%
[19:51:03] [Render thread/ERROR]: Exception caught in render loop
java.lang.OutOfMemoryError: Java heap space
\tat java.util.Arrays.copyOf(Arrays.java:3236)
\tat net.minecraft.client.renderer.chunk.RenderChunk.rebuild(RenderChunk.java:184)
\tat net.minecraft.client.renderer.ChunkRenderDispatcher.uploadChunk(ChunkRenderDispatcher.java:212)
\tat net.minecraft.client.renderer.LevelRenderer.setupRender(LevelRenderer.java:810)
[19:51:04] [Render thread/FATAL]: Unreported exception thrown!
[19:51:06] [Forge Version Check/INFO]: Mod loaded successfully`;

// --- 启动 -----------------------------------------------------------------

function init() {
  void checkHealth();
  void loadReports();
  wireDrop();

  $('report-select').addEventListener('change', event => {
    void loadReport(event.target.value);
  });

  $('log').addEventListener('input', updateLogMeta);
  $('file').addEventListener('change', event => void readFile(event.target.files?.[0]));
  $('analyse').addEventListener('click', () => void analyse());
  $('clear').addEventListener('click', () => {
    $('log').value = '';
    updateLogMeta();
    $('result').className = 'result empty';
    $('result').innerHTML = '<p class="placeholder">还没有分析结果。</p>';
    $('result-meta').textContent = '';
  });
  $('load-sample').addEventListener('click', () => {
    $('log').value = SAMPLE;
    $('env-game').value = '1.20.1';
    $('env-loader').value = 'Forge';
    $('env-memory').value = '4096';
    updateLogMeta();
  });

  // Ctrl/Cmd+Enter 直接分析：日志框很大，用户粘完通常不想再去够按钮。
  $('log').addEventListener('keydown', event => {
    if ((event.ctrlKey || event.metaKey) && event.key === 'Enter') {
      event.preventDefault();
      void analyse();
    }
  });
}

init();
