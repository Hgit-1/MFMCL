# 崩溃 AI 分析

用本地 **LFM3-1.7B** 解释 Minecraft 崩溃日志。

- 页面：`http://127.0.0.1:8790/`（独立服务，供设计评审）
- 后端：`crates/crash-ai`（库 + 独立服务 + CLI），同一份代码也接进了桌面版

## 设计原则（评审时请先读这一节）

一个 1.7B 的模型不能独立判断崩溃原因。实测中，它在内存溢出的日志上给出的建议是
**「降低堆内存到 512 MB」**——正好是把崩溃变成必然的方向。所以职责这样切分：

| 做什么 | 谁负责 | 为什么 |
| --- | --- | --- |
| 判断**是什么**问题 | 确定性规则 | 可测试、可回溯，命中就是命中 |
| 解释**为什么**、读这份日志的具体细节 | LFM3 | 这是语言模型真正擅长的事 |
| 给出**怎么做** | 规则（命中时）| 建议是人工整理并测试过的 |
| 规则没命中时的推测与建议 | LFM3，但明确标注 | 没有更好的来源，但要让人知道这是猜的 |

界面上必须始终能看出结论的来源。规则命中和模型推测的视觉、措辞、步骤来源说明
都不一样，这是这套设计的核心，改版时不要把它们合并成一种样式。

## 三种结果的呈现

### 1. 规则命中（最常见）

```
很可能的原因是：内存不足            [很可能] 内存不足   runtime.outofmemory
                                    游戏用尽了分配给它的堆内存……
                                    1. 提高内存分配（按模组数量可加 1–2 GB）
                                    2. 减少模组，或降低视距
                                    ▸ 依据的日志行

[模型解释] 为什么                   lfm3-1.7b
                                    崩溃日志显示在 Arrays.copyOf 时抛出 OutOfMemoryError…
                                    步骤来自匹配到的规则，不是模型生成的。
```

要点：
- 左侧色条按置信度区分（红/橙/灰）
- 规则 id（`runtime.outofmemory`）用等宽小字，供愿意查文档的人
- 模型的步骤一律来自规则，并明写「步骤来自规则」

### 2. 规则未命中，模型推测

虚线框 + 「模型推测」标签 + 「步骤由模型给出，未经规则验证」。这是唯一使用模型
自己建议的场景。

### 3. 模型不可用

顶部横幅说明原因，规则结论照常显示。**不能**把这种情况显示成「分析失败」——
分析是成功的，只是少了一段解释。

## 规则清单

19 条，按失败层分组。每条都有 id、置信度、解释、建议，以及一个 fixture 测试。

| 分组 | 规则 |
| --- | --- |
| JVM 无法启动 | `heap.invalid` `heap.reserve` `jvm.option.invalid` `jvm.option.space` |
| Java 版本不匹配 | `java.class.version` `java.class.version.mismatch` `java.too.new` `java.openj9` |
| 游戏文件 | `files.missing.assets` `files.corrupt.jar` `files.class.notfound` `files.no.main.class` `files.access.denied` |
| 图形 | `gpu.opengl` `gpu.gl.function` `gpu.driver` |
| 模组加载 | `mods.mixin` `mods.duplicate` `mods.missing.dependency` `mods.incompatible` `mods.forge.dependency` |
| 被外部终止 | `runtime.killed` `runtime.headless` |
| 内存 | `runtime.outofmemory` |

### 一个真实的坑

`mods.mixin` 最初只匹配 `mixin` 一个词。每个 Fabric/Forge 的正常启动日志里都有
`SpongePowered MIXIN Subsystem Version=...`，于是**一份完全正常的日志被判成了
Mixin 失败**。现在要求同时出现失败词，并排除子系统横幅。这个 bug 是
`clean-launch` fixture 抓到的——所以 fixture 里必须有一份「什么都没坏」的日志。

## LFM3 的两个坑

**思考模式必须关掉。** LFM3 默认先输出 `<think>` 块。在 CPU 上，1.7B 模型的思考块
会吃掉全部 token 预算，API 返回空的 `content`——看起来像服务坏了。请求里固定带
`chat_template_kwargs: { enable_thinking: false }`，响应里再兜底剥离 `<think>`。

实测对比：思考开 → `''`；思考关 → `'PONG'`。

**置信度用全角冒号。** 模型实际输出 `置信度：高`（全角）。早期只匹配半角，导致
所有分析都静默落到「低」。现在先归一化冒号再匹配。

## 运行

```bash
# 1. 模型服务（本机 LM Studio 的运行时，需支持 lfm3 架构）
cd /tmp/llama.cpp/build/bin
./llama-server -m ~/.lmstudio/models/LiquidAI/LFM3-1.7B-GGUF/LFM3-1.7B-Q4_K_M.gguf \
  --host 127.0.0.1 --port 8099 -c 8192 -t 12

# 2. 分析服务
cargo run -p crash-ai --bin crash-ai-server
```

环境变量：

| 变量 | 默认值 | 说明 |
| --- | --- | --- |
| `MFMCL_LLM_BASE_URL` | `http://127.0.0.1:8099` | llama.cpp 服务地址 |
| `MFMCL_LLM_MODEL` | `lfm3-1.7b` | 报告给模型服务的名字 |
| `CRASH_AI_PORT` | `8790` | 分析服务端口 |
| `CRASH_AI_HOST` | `127.0.0.1` | 监听地址 |

CLI：

```bash
cargo run -p crash-ai --bin crash-ai-cli -- path/to/crash-report.txt
cargo run -p crash-ai --bin crash-ai-cli -- --self-test   # 全部 fixture
```

## 性能实测

本机（CPU，12 线程，Q4_K_M）：

| 场景 | 耗时 |
| --- | --- |
| 规则 only | 4–14 ms |
| 规则 + 模型（典型日志） | 6–13 s |

模型部分慢是必然的：1.7B 在 CPU 上约 26 tok/s。所以界面把规则结果先渲染出来，
模型解释后到再补上——用户不会盯着空白等十几秒。

## 已知限制

- **模型只在 CPU 上跑。** 本机 CUDA 驱动版本与运行时不匹配（`Failed to initialize
  NVML`），llama.cpp 的 CUDA 后端加载失败。CPU 推理可用但慢。换机器或修复驱动后
  把 `-ngl` 调高即可。
- **上下文 8k。** 送进模型的日志片段上限 6000 字符，约 1500 token，留足回答空间。
- **只分析文本。** 不读 JVM 的 hs_err_pid 二进制段，也不读 mods 目录做交叉验证。
- **中文输出固定。** 提示词中文写死，模型只保证简体中文，不做多语切换。

## 代码结构

```
crates/crash-ai/
├── src/lib.rs          分析入口：抽取事实 + 合并结果
├── src/rules.rs        19 条签名规则 + 测试
├── src/llm/mod.rs      LFM3 客户端、输出解析、思考块剥离
├── src/llm/prompt.rs   提示词（短、有据、格式固定）
├── src/bin/server.rs   独立 HTTP 服务 + 静态页面
├── src/bin/cli.rs      CLI 与 fixture 自检
├── src/fixtures/*.log  6 份真实形态的日志，用于回归
└── web/                独立页面（index.html / app.js / style.css）

src-tauri/src/crash_ai.rs  桌面版的同名命令（同一份库）
```

## 接进桌面版

三个命令，与独立服务行为一致：

- `crash_ai_status` —— 模型是否可用
- `crash_ai_analyse` —— 分析日志内容（不读文件）
- `crash_ai_instance_report` —— 取某实例最新的 crash-report

`crash_ai_analyse` 刻意不接收文件路径：日志内容由调用方读好再传。崩溃报告可能来自
实例目录之外（用户从论坛复制的），让后端按路径读取会把「能否读任意路径」变成额外
的攻击面。取实例报告的命令则只在自己管理的 `crash-reports` 目录里找。

## 给评审的具体问题

1. **三种结果样式够不够区分？** 尤其是「模型推测」与「规则命中」在快速扫读时是否
   会被混淆。这是整套设计成立的前提。
2. **规则 id 要不要给用户看？** 现在只有悬停/小字，但它能指向文档。也许该做成可点。
3. **模型解释放在规则下面是否合适？** 另一种排法是模型解释优先（读起来更自然），
   规则作为「已确认的事实」收在下面。
4. **「步骤来自规则」这句说明会不会太技术化？** 它的存在是为了诚实，但普通玩家
   可能不理解「规则」指什么。
5. **缺少日志时的向导。** 现在要求用户自己找到日志文件。是否需要一个「帮我找到
   日志」的入口（桌面版能直接定位实例目录）。
