#!/usr/bin/env python3
"""LFM3-1.7B Q8_0 的独立准确率实测。

## 这个脚本在测什么

给模型 10 份**真实崩溃日志**，每份都有一个人工确认的标准答案（日志里的事实本身
就决定了答案，不需要主观判断）。让模型只根据日志给出原因，然后按标准答案逐条
判定对错。

这不是跑官方 benchmark。理由有两个：
  1. 官方那张表用的是 BF16 权重，没有任何一行是 Q8 量化后的数字；
  2. 官方 benchmark 测的是 agent / 工具调用 / 写代码，与这个启动器无关。

对启动器有意义的问题是：**它能不能把一份崩溃日志归到正确的原因上**。

## 判定方式

用关键词匹配而不是语义评分：每个 fixture 的正确原因有唯一的判别词（
`OutOfMemoryError` → 堆内存，`UnsupportedClassVersionError` → Java 版本）。关键词
只是判分的自动化手段，标准答案本身来自日志事实。

为了不把「碰巧提到关键词」算成正确，要求回答里**同时**出现原因词，且不出现
混淆词（例如把 OOM 答成模组冲突）。

## 运行

    python3 scripts/crash-ai/q8_eval.py http://127.0.0.1:8199
"""
import json
import os
import sys
import time
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
FIXTURES = ROOT / "crates/crash-ai/src/fixtures"

# 与 `crates/crash-ai/src/llm/prompt.rs` 的 SYSTEM 保持一致。
# 用同一套提示词，测的才是启动器实际会看到的输出质量。
SYSTEM = """你是 Minecraft 启动器的崩溃分析助手。你的读者是不懂技术的玩家。

规则：
1. 只根据给出的证据回答，不要编造日志里没有的内容。
2. 如果证据不足以确定原因，直接说“无法确定”，并说明还需要什么信息。
3. 不要说“可能是驱动、内存或模组”这种罗列。给出你认为最可能的那一个。
4. 建议必须是玩家能直接做的操作，不要提修改代码。
5. 用简体中文，语气平实，不要用感叹号。

输出格式（严格遵守）：
说明：一段话，2-4 句，讲清为什么崩溃。
步骤：
1. 第一个操作
2. 第二个操作
置信度：高/中/低"""

# 每个 fixture 的标准答案与判分关键词。
#
# `must` 是「答对时必须提到」的词（任一命中即可），`must_not` 是「答错时常出现
# 的混淆词」。两者配合，避免「顺口提了一句内存」就被判对。
CASES = [
    {
        "file": "out-of-memory.log",
        "answer": "堆内存不足",
        "must": ["内存", "堆", "heap", "OutOfMemory"],
        "must_not": ["模组冲突是主要原因", "显卡"],
    },
    {
        "file": "old-java.log",
        "answer": "Java 版本过低",
        "must": ["Java 版本", "Java 太旧", "升级 Java", "UnsupportedClassVersion"],
        "must_not": [],
    },
    {
        "file": "missing-mod-dependency.log",
        "answer": "缺少前置模组 architectury",
        "must": ["architectury", "前置", "依赖"],
        "must_not": [],
    },
    {
        "file": "duplicate-mod.log",
        "answer": "同一个模组装了两次",
        "must": ["重复", "两份", "Duplicate", "多次"],
        "must_not": [],
    },
    {
        "file": "heap-too-large.log",
        "answer": "分配的堆内存超过系统可给的上限",
        "must": ["内存", "堆", "heap", "过大", "超过"],
        "must_not": [],
    },
    {
        "file": "hs-err-gpu.log",
        "answer": "显卡驱动崩溃",
        "must": ["显卡", "驱动", "GPU", "nvoglv", "OpenGL"],
        "must_not": [],
    },
    {
        "file": "opengl-failure.log",
        "answer": "OpenGL / 图形环境创建失败",
        "must": ["OpenGL", "图形", "显卡", "GLFW", "驱动"],
        "must_not": [],
    },
    {
        "file": "permission-denied.log",
        "answer": "文件权限不足",
        "must": ["权限", "AccessDenied", "只读", "写入"],
        "must_not": [],
    },
    {
        "file": "watchdog-timeout.log",
        "answer": "服务端线程卡死被看门狗终止",
        "must": ["看门狗", "watchdog", "卡死", "超时", "无响应"],
        "must_not": [],
    },
    {
        # 对照组：这份日志什么都没坏。答「崩溃了」就是假阳性。
        "file": "clean-launch.log",
        "answer": "没有崩溃（日志正常）",
        "must": ["没有", "正常", "未", "无法确定", "没发现"],
        "must_not": ["崩溃了", "出现了严重错误"],
        "negative": True,
    },
]


def ask(base_url: str, log_text: str, timeout: int = 180) -> str:
    """向本地模型要一次分析。思考模式必须关掉。"""
    payload = {
        "model": "local",
        "messages": [
            {"role": "system", "content": SYSTEM},
            {"role": "user", "content": log_text[:12000]},
        ],
        "max_tokens": 420,
        "temperature": 0.2,
        "top_p": 0.9,
        "stream": False,
        # 与启动器一致：LFM3 默认先输出 <think> 块，在 CPU 上会吃光整个预算，
        # 导致 content 为空——看起来像服务坏了。
        "chat_template_kwargs": {"enable_thinking": False},
    }
    request = urllib.request.Request(
        f"{base_url}/v1/chat/completions",
        data=json.dumps(payload).encode(),
        headers={"Content-Type": "application/json"},
    )
    with urllib.request.urlopen(request, timeout=timeout) as response:
        data = json.load(response)
    return data["choices"][0]["message"]["content"]


def judge(case: dict, text: str) -> tuple[bool, str]:
    """按关键词判定。返回 (是否正确, 判定依据)。"""
    hits = [word for word in case["must"] if word.lower() in text.lower()]
    traps = [word for word in case["must_not"] if word.lower() in text.lower()]
    if traps:
        return False, f"命中了混淆词 {traps}"
    if hits:
        return True, f"命中 {hits}"
    return False, "没有命中任何正确原因词"


def main() -> int:
    base_url = sys.argv[1] if len(sys.argv) > 1 else "http://127.0.0.1:8199"
    results = []
    total_seconds = 0.0

    for index, case in enumerate(CASES, start=1):
        path = FIXTURES / case["file"]
        if not path.exists():
            print(f"[{index}/{len(CASES)}] {case['file']} 缺失，跳过", flush=True)
            continue
        log_text = path.read_text(encoding="utf-8", errors="replace")
        started = time.time()
        try:
            answer = ask(base_url, log_text)
        except Exception as error:  # noqa: BLE001 - 网络/解析错误都要能继续跑下一个
            print(f"[{index}/{len(CASES)}] {case['file']} 请求失败：{error}", flush=True)
            results.append({"file": case["file"], "ok": False, "why": f"请求失败：{error}"})
            continue
        elapsed = time.time() - started
        total_seconds += elapsed
        ok, why = judge(case, answer)
        results.append({
            "file": case["file"],
            "expected": case["answer"],
            "ok": ok,
            "why": why,
            "seconds": round(elapsed, 1),
            "answer": answer.strip(),
        })
        print(f"[{index}/{len(CASES)}] {'PASS' if ok else 'FAIL'} {case['file']} ({elapsed:.1f}s) — {why}", flush=True)

    correct = sum(1 for item in results if item["ok"])
    total = len(results)
    print()
    print(f"===== Q8_0 准确率：{correct}/{total} = {correct / total * 100:.0f}% =====")
    if results:
        print(f"平均每次 {total_seconds / len(results):.1f} 秒")
    out = ROOT / "scripts/crash-ai/q8-eval-result.json"
    out.write_text(json.dumps({
        "model": "lmstudio-community/LFM3-1.7B-GGUF LFM3-1.7B-Q8_0.gguf",
        "sha256": "15ce5334f7551ebc193dcbc21ad12acfeb68cbb17cbf08a736ae3482ffe4b098",
        "correct": correct,
        "total": total,
        "accuracy": round(correct / total * 100, 1) if total else 0,
        "averageSeconds": round(total_seconds / len(results), 1) if results else 0,
        "results": results,
    }, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(f"明细已写入 {out.relative_to(ROOT)}")
    return 0 if correct == total else 1


if __name__ == "__main__":
    sys.exit(main())
