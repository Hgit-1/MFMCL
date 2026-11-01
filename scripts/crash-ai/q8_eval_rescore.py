#!/usr/bin/env python3
"""对 `q8_eval.py` 保存的回答做严格重判。

## 为什么需要这一步

第一版判分用「回答里出现正确原因词就算对」，结果 10/10。逐条读输出后发现这个
判据太松，两个典型反例：

  - `clean-launch.log`（正常日志）的回答里出现了「没有明确提到崩溃」，于是匹配到
    「没有」被判对；但模型实际把这个日志当成了崩溃，并建议卸载所有模组。
  - `hs-err-gpu.log` 的回答里出现了 `nvoglv64.dll`，于是被判对；但模型把它解释
    成「Java 与操作系统的内存访问冲突」，方向完全错了。

关键词匹配无法区分「提到了正确线索」和「理解对了」。所以这里改用逐条人工判定的
结论，并把判定理由写下来，可复核。

## 三个判定维度

  1. **诊断**：给出的原因是否与日志事实一致（这是主要指标）
  2. **建议**：修复步骤是否安全、是否指向真正该改的地方
  3. **误报**：对正常日志是否报了崩溃

「诊断对但建议错」单独标注：这在崩溃分析里是最危险的一类，因为用户会照着做。

## 运行

    python3 scripts/crash-ai/q8_eval_rescore.py
"""
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SOURCE = ROOT / "scripts/crash-ai/q8-eval-result.json"
OUTPUT = ROOT / "scripts/crash-ai/q8-eval-rescored.json"

# 严格判定。
#
# `diagnosis`：correct / partial / wrong
# `advice`：    safe / risky / harmful / n-a
# `note`：判定理由，写给复核的人看。
VERDICTS = {
    "out-of-memory.log": {
        "diagnosis": "correct",
        "advice": "risky",
        "note": "正确识别 Java heap space 耗尽。但建议「关闭所有模组」过于粗暴；"
                "日志已明确是堆内存，正确做法是提高分配或排查单个高占用模组。",
    },
    "old-java.log": {
        "diagnosis": "correct",
        "advice": "harmful",
        "note": "正确识别 class 版本 61.0 与 Java 8 不匹配。但建议「删除 mods 文件夹」"
                "会丢光模组，而真正该做的是安装 Java 21。诊断对、动作有害。",
    },
    "missing-mod-dependency.log": {
        "diagnosis": "correct",
        "advice": "safe",
        "note": "正确指出缺少 architectury，并建议安装它。这一条全对。",
    },
    "duplicate-mod.log": {
        "diagnosis": "correct",
        "advice": "safe",
        "note": "正确指出同一模组存在两份，建议删掉多余的那份。",
    },
    "heap-too-large.log": {
        "diagnosis": "wrong",
        "advice": "harmful",
        "note": "日志是「Could not reserve enough space for 16777216KB」——堆**设得太大**，"
                "系统给不出 16 GB。模型读成「内存不足」，并建议删除 fabric-api/sodium 等"
                "模组。方向反了：这是设置问题，删模组毫无作用。",
    },
    "hs-err-gpu.log": {
        "diagnosis": "wrong",
        "advice": "risky",
        "note": "nvoglv64.dll 是 NVIDIA 显示驱动，模型却解释成「Java 与操作系统的内存"
                "访问冲突」，并建议「完全断开与电脑连接、重启」。线索念对了，理解错了。",
    },
    "opengl-failure.log": {
        "diagnosis": "correct",
        "advice": "safe",
        "note": "正确识别 GLFW 无法创建 GLX/OpenGL 上下文，建议检查显卡驱动。",
    },
    "permission-denied.log": {
        "diagnosis": "correct",
        "advice": "risky",
        "note": "正确识别 AccessDeniedException 是权限问题。但建议是 Windows 的"
                "「属性 → 安全」流程，而日志里是 Linux 路径 `/home/u/`。",
    },
    "watchdog-timeout.log": {
        "diagnosis": "partial",
        "advice": "wrong",
        "note": "把 watchdog 说成「守护进程超时」并建议「减少在线玩家」——那是服务端"
                "运维动作，这是一份客户端日志。真正含义是服务端线程卡死被强制终止。",
    },
    "clean-launch.log": {
        "diagnosis": "wrong",
        "advice": "harmful",
        "note": "**误报**。这份日志没有任何错误，模型开头写「崩溃日志显示…」，"
                "结尾建议卸载所有模组。对正常日志报崩溃是最严重的一类失败。",
    },
}


def main() -> int:
    if not SOURCE.exists():
        print(f"缺少 {SOURCE}，先运行 q8_eval.py")
        return 1
    data = json.loads(SOURCE.read_text(encoding="utf-8"))

    rows = []
    for item in data["results"]:
        verdict = VERDICTS.get(item["file"])
        if verdict is None:
            continue
        rows.append({
            "file": item["file"],
            "expected": item.get("expected", ""),
            **verdict,
            "seconds": item.get("seconds"),
            "answer": item.get("answer", ""),
        })

    total = len(rows)
    correct = sum(1 for row in rows if row["diagnosis"] == "correct")
    partial = sum(1 for row in rows if row["diagnosis"] == "partial")
    wrong = sum(1 for row in rows if row["diagnosis"] == "wrong")
    harmful = sum(1 for row in rows if row["advice"] == "harmful")

    print(f"{'file':<32} {'诊断':<8} {'建议':<8}")
    print("-" * 52)
    for row in rows:
        print(f"{row['file']:<32} {row['diagnosis']:<8} {row['advice']:<8}")
    print("-" * 52)
    print(f"诊断正确 {correct}/{total} = {correct / total * 100:.0f}%"
          f"（部分正确 {partial}，错误 {wrong}）")
    print(f"建议有害 {harmful}/{total}")
    # 一个更严格的合取指标：诊断对**且**建议不有害。
    safe_correct = sum(1 for row in rows if row["diagnosis"] == "correct" and row["advice"] in ("safe", "risky"))
    print(f"诊断正确且建议可用 {safe_correct}/{total} = {safe_correct / total * 100:.0f}%")

    OUTPUT.write_text(json.dumps({
        "model": data["model"],
        "sha256": data["sha256"],
        "scoring": "strict manual review (diagnosis direction + advice safety + false positives)",
        "averageSeconds": data["averageSeconds"],
        "diagnosis": {"correct": correct, "partial": partial, "wrong": wrong, "accuracy": round(correct / total * 100, 1)},
        "advice": {"harmful": harmful},
        "diagnosisCorrectAndAdviceUsable": {"count": safe_correct, "accuracy": round(safe_correct / total * 100, 1)},
        "cases": rows,
    }, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(f"明细已写入 {OUTPUT.relative_to(ROOT)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
