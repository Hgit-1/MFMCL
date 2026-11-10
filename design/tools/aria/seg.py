#!/usr/bin/env python3
"""Cut the seasonal Aria artwork out of its background with the segmentation API.

## Why a script and not a manual step

Eight images, each one needing the same request shape, the same "download the
result before the signed URL expires" dance, and the same comparison against the
alpha that is already in the file. Done by hand that is eight chances to save the
wrong file.

## Why it does not overwrite by default

The artwork that ships already has an alpha channel from the generation endpoint,
and it is *good*: `audit.py` measures a negative fringe (the soft edge is darker
than the interior, not lighter) on every asset, i.e. no white halo on the dark
theme. A second cutout can still be an improvement — the model draws hair strands
and petal edges finer than the natively returned alpha sometimes keeps — but it
can also be a regression: a person/object model may clip a thin strand or the
wisp of hair over the hat brim.

So the default is **compare and report**. `--apply` is the opt-in, and it keeps a
copy of the previous alpha in `<name>.alpha-prev.png` beside the asset so a
regression is one rename away from being undone.

## Auth

`MFMCL_ARIA_IMAGE_KEY` — the same variable `gen.py` uses. Nothing about the key
is written to disk.
"""

from __future__ import annotations

import io
import json
import os
import sys
import time
import urllib.error
import urllib.request
import uuid
from pathlib import Path

try:
    from PIL import Image
    import numpy as np
except ImportError:  # pragma: no cover - environment guard
    raise SystemExit("Pillow and numpy are required: pip install pillow numpy")

HERE = Path(__file__).resolve().parent
ASSET_DIR = HERE.parent.parent / "flowlaunch-ui" / "public" / "plugins" / "aria-with-you"
OUT_DIR = HERE / "out" / "seg"

ENDPOINT = "https://api.gpt.ge/task/pic/segmentation"
SEASONS = ("spring", "summer", "autumn", "winter")
CROPS = ("hero", "portrait")


def _multipart(fields: dict[str, str], file_field: str, filename: str, data: bytes):
    boundary = f"----aria{uuid.uuid4().hex}"
    chunks: list[bytes] = []
    for name, value in fields.items():
        chunks.append(
            (
                f"--{boundary}\r\n"
                f'Content-Disposition: form-data; name="{name}"\r\n\r\n'
                f"{value}\r\n"
            ).encode()
        )
    chunks.append(
        (
            f"--{boundary}\r\n"
            f'Content-Disposition: form-data; name="{file_field}"; filename="{filename}"\r\n'
            "Content-Type: image/png\r\n\r\n"
        ).encode()
    )
    chunks.append(data)
    chunks.append(f"\r\n--{boundary}--\r\n".encode())
    return b"".join(chunks), f"multipart/form-data; boundary={boundary}"


def segment(png: bytes, *, key: str, sync: bool = True, mode: str = "person") -> bytes:
    """Send one image and return the cutout PNG bytes.

    `sync=1` blocks server-side until the task is done (~200 ms in testing). The
    async path exists for batching, which this script does not need: eight images
    one after another is simpler than polling, and a failed poll is one more
    failure mode to write.

    `type=person` rather than auto-detect: these are character illustrations
    against a plain background, and the auto mode occasionally decides the props
    (cherry blossom branch, paper bag) are the subject instead.
    """
    fields = {
        "sync": "1" if sync else "0",
        "type": mode,
        # output_type=1 returns the cutout *and* the mask. The mask is what makes
        # the comparison honest: it says which pixels the model decided were
        # foreground, independent of how it coloured them.
        "output_type": "1",
        "crop": "0",
        "format": "png",
    }
    body, content_type = _multipart(fields, "image_file", "input.png", png)
    request = urllib.request.Request(
        ENDPOINT,
        data=body,
        headers={"Authorization": f"Bearer {key}", "Content-Type": content_type},
        method="POST",
    )

    last: Exception | None = None
    for attempt in range(3):
        try:
            with urllib.request.urlopen(request, timeout=180) as response:
                document = json.loads(response.read().decode("utf-8"))
            break
        except (urllib.error.URLError, TimeoutError, json.JSONDecodeError) as error:
            last = error
            time.sleep(3 * (attempt + 1))
    else:
        raise RuntimeError(f"segmentation request failed: {last}")

    if document.get("status") != 200:
        raise RuntimeError(f"segmentation rejected: {document.get('message')!r}")

    data = document.get("data") or {}
    if data.get("state_detail") not in (None, "Complete"):
        raise RuntimeError(f"segmentation did not complete: {data.get('state_detail')}")
    image_url = data.get("image")
    if not image_url:
        raise RuntimeError(f"no image in response: {data.get('err_info')!r}")

    # The signed URL expires in an hour; download straight away rather than
    # handing it to the caller who might cache it.
    with urllib.request.urlopen(image_url, timeout=180) as response:
        return response.read()


def alpha_only(result: Image.Image) -> np.ndarray:
    """The cutout's alpha as a uint8 array."""
    return np.array(result.convert("RGBA").getchannel("A"))


def compare(current: Path, candidate: np.ndarray) -> dict[str, object]:
    """Describe how the candidate alpha differs from the one in the file.

    The numbers that matter:

      - **IoU** of the solid (>128) masks. Below ~0.95 the two disagree about
        where the character is, not just about how soft the edge is.
      - **Gained / lost opaque pixels** — the direction matters. Gaining opaque
        pixels around the silhouette means the old alpha was too tight and
        clipped hair. Losing them means the model ate a strand or a prop.
      - **Soft edge width**, as the fraction of semi-transparent pixels. A lower
        number is a crisper edge; that is what a cutout model is for. A much
        higher number means a blurrier edge, which will look like a halo.
    """
    existing = Image.open(current).convert("RGBA")
    old = np.array(existing.getchannel("A"))
    if candidate.shape != old.shape:
        candidate = np.array(
            Image.fromarray(candidate).resize(
                (existing.width, existing.height), Image.LANCZOS
            )
        )
    old_solid = old > 128
    new_solid = candidate > 128
    intersection = np.logical_and(old_solid, new_solid).sum()
    union = np.logical_or(old_solid, new_solid).sum()
    iou = float(intersection / union) if union else 1.0
    return {
        "iou": round(iou, 4),
        "gained_px": int(np.logical_and(new_solid, ~old_solid).sum()),
        "lost_px": int(np.logical_and(old_solid, ~new_solid).sum()),
        "old_soft": round(float(((old > 8) & (old < 247)).mean()), 4),
        "new_soft": round(float(((candidate > 8) & (candidate < 247)).mean()), 4),
        "shape_match": candidate.shape == old.shape,
    }


def main(argv: list[str]) -> int:
    key = os.environ.get("MFMCL_ARIA_IMAGE_KEY")
    if not key:
        print("MFMCL_ARIA_IMAGE_KEY is not set", file=sys.stderr)
        return 2

    apply = "--apply" in argv
    force = "--force" in argv
    only = {a for a in argv[1:] if not a.startswith("--")}

    OUT_DIR.mkdir(parents=True, exist_ok=True)
    summary: dict[str, object] = {}
    failures: list[str] = []

    for season in SEASONS:
        for crop in CROPS:
            key_name = f"{season}-{crop}"
            if only and key_name not in only and season not in only:
                continue
            path = ASSET_DIR / f"aria-{key_name}.webp"
            if not path.exists():
                failures.append(f"{key_name}: asset missing")
                continue

            # The API takes a flattened input: it segments an opaque picture. The
            # existing file is transparent, so it is composited onto white first.
            # White rather than mid-grey because the model's training expects a
            # plain light background, and a grey plate bleeds into soft hair.
            current = Image.open(path).convert("RGBA")
            plate = Image.new("RGBA", current.size, (255, 255, 255, 255))
            flat = Image.alpha_composite(plate, current).convert("RGB")

            buffer = io.BytesIO()
            flat.save(buffer, "PNG")
            print(f"segment {key_name} ...", flush=True)
            started = time.time()
            try:
                result_bytes = segment(buffer.getvalue(), key=key)
            except Exception as error:  # network, HTTP, missing field
                failures.append(f"{key_name}: {error}")
                print(f"  failed: {error}", file=sys.stderr)
                continue

            result = Image.open(io.BytesIO(result_bytes)).convert("RGBA")
            (OUT_DIR / f"aria-{key_name}-cutout.png").write_bytes(result_bytes)

            report = compare(path, alpha_only(result))
            summary[key_name] = report
            print(
                f"  iou {report['iou']:.3f}  gained {report['gained_px']}  "
                f"lost {report['lost_px']}  soft {report['old_soft']:.4f} -> "
                f"{report['new_soft']:.4f}  ({time.time() - started:.1f}s)"
            )

            if not apply:
                continue
            if report["iou"] < 0.97:
                # Not a soft-edge difference: the two disagree about the subject.
                # Overwriting here is how a hand disappears.
                print(
                    f"  skipped apply: IoU {report['iou']:.3f} below 0.97",
                    file=sys.stderr,
                )
                failures.append(f"{key_name}: IoU too low to apply safely")
                continue
            if not force and report["lost_px"] > 4000:
                print(
                    f"  skipped apply: {report['lost_px']} opaque pixels would be "
                    "lost (use --force to accept)",
                    file=sys.stderr,
                )
                continue

            # Keep the current alpha for a one-rename rollback. Only the alpha:
            # the colour channels are the artwork and the cutout keeps them.
            previous = np.array(current.getchannel("A"))
            Image.fromarray(previous).save(OUT_DIR / f"aria-{key_name}-alpha-prev.png")

            merged = current.copy()
            merged.putalpha(Image.fromarray(alpha_only(result)))
            buffer = io.BytesIO()
            merged.save(buffer, "WEBP", quality=92, method=6, alpha_quality=100)
            path.write_bytes(buffer.getvalue())
            print(f"  applied ({len(buffer.getvalue())} bytes)")

    report_path = OUT_DIR / "segmentation.json"
    report_path.write_text(
        json.dumps({"applied": apply, "assets": summary}, indent=2, ensure_ascii=False)
        + "\n",
        "utf-8",
    )
    print()
    if failures:
        print("problems:", file=sys.stderr)
        for failure in failures:
            print(f"  {failure}", file=sys.stderr)
    print(f"report: {report_path}")
    print(f"cutouts: {OUT_DIR}")
    return 1 if failures else 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
