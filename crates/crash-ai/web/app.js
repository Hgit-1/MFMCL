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
async function checkHealth() {
  const status = $('status');
  const text = $('status-text');
  status.dataset.state = 'checking';
  text.textContent = '正在检查模型…';
  try {
    const response = await fetch('/api/health');
    const data = await response.json();
    if (data.ok) {
      status.dataset.state = 'ok';
      text.textContent = `模型就绪 · ${data.model}`;
      $('foot-model').textContent = `${data.model} @ ${data.endpoint}`;
    } else {
      status.dataset.state = 'down';
      text.textContent = '模型未连接（仍可只用规则分析）';
      $('foot-model').textContent = data.endpoint || '';
    }
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
    const response = await fetch('/api/analyse', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ log, environment, useModel }),
    });
    const data = await response.json();
    if (!response.ok) {
      renderError(data.error || `HTTP ${response.status}`);
      return;
    }
    lastAnalysis = data;
    render(data, Math.round(performance.now() - started));
  } catch (error) {
    renderError(String(error));
  } finally {
    button.disabled = false;
    button.textContent = '开始分析';
    hint.textContent = '规则分析是即时的；开启模型后需要几秒到一分钟';
  }
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

function render(data, elapsedMs) {
  const result = $('result');
  result.className = 'result';

  const parts = [];

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

  if (findings.length === 0) {
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
  wireDrop();

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
