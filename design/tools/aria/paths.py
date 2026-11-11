"""Shared external-workspace boundary for the optional Aria authoring tools."""

from __future__ import annotations

import os
import sys
from pathlib import Path

LAUNCHER_ROOT = Path(__file__).resolve().parents[3]


def external_path(value: str | Path, label: str) -> Path:
    path = Path(value).expanduser()
    lexical = Path(os.path.abspath(path))
    resolved = path.resolve()
    if any(candidate == LAUNCHER_ROOT or LAUNCHER_ROOT in candidate.parents
           for candidate in (lexical, resolved)):
        raise ValueError(f"{label} must be outside the launcher repository: {value}")
    return resolved


def external_tree(value: str | Path, label: str) -> Path:
    root = external_path(value, label)
    if root.exists() and not root.is_dir():
        raise ValueError(f"{label} must be a directory: {root}")
    # A safe root alone does not prevent nested links from importing or overwriting
    # launcher files. Refuse escapes before any API request or output creation.
    for path in root.rglob("*"):
        resolved = external_path(path, label)
        if not resolved.is_relative_to(root):
            raise ValueError(f"{label} contains a link outside its directory: {path}")
    return root


def artwork_dir() -> Path:
    configured = os.environ.get("MFMCL_ARIA_OUT_DIR", "").strip()
    if not configured:
        print(
            "Set MFMCL_ARIA_OUT_DIR to an external plugin assets directory; "
            "Aria artwork and tool output must not be written into the launcher repository.",
            file=sys.stderr,
        )
        raise SystemExit(2)
    try:
        return external_tree(configured, "MFMCL_ARIA_OUT_DIR")
    except (OSError, RuntimeError, ValueError) as error:
        print(str(error), file=sys.stderr)
        raise SystemExit(2) from error
