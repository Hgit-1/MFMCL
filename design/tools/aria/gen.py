#!/usr/bin/env python3
"""Generate Aria seasonal artwork through the configured image endpoint.

## Why a script instead of doing it by hand

Four seasons times two crops is eight images, and the set has to stay visually
consistent: same character, same line weight, same palette discipline. Anything
that is repeated eight times by hand drifts. Keeping the shared half of the
prompt in one string also means a fix (a wrong hair colour, a missing prop)
applies to the whole set on the next run instead of to one image.

## Why the key comes from the environment

The key is a paid credential. Committing it would put it in every clone and every
build artifact, which is exactly the mistake HANDOFF.md warns about for the
CurseForge key. The script reads it from `MFMCL_ARIA_IMAGE_KEY` and refuses to
run without it.

## Output

`design/flowlaunch-ui/public/plugins/aria-with-you/` plus `manifest.json`
describing each file with its SHA-256 and pixel size, so the frontend can verify
what it loads and the repository records what was generated and with which model.
"""

from __future__ import annotations

import base64
import hashlib
import http.client
import io
import json
import os
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path

ENDPOINT = "https://api.gpt.ge/v1/images/generations"
MODEL = os.environ.get("MFMCL_ARIA_IMAGE_MODEL", "gpt-image-2.6-sunburst")

HERE = Path(__file__).resolve().parent
"""
素材输出目录。

## 为什么不再默认指向主仓库

立绘不再随启动器打包：它们由 [mfmcl-aria-with-you](https://github.com/Hgit-1/mfmcl-aria-with-you)
单独发布，启动器运行时从 CDN 取（见 `utils/aria.ts` 的 `ART_BASE`）。默认路径若
还指向 `public/plugins/`，下次重新生成就会往主仓库里写一份 7.6 MB，然后两份各自
漂移——这正是这次迁移要消除的。

## 为什么改成必须显式指定

尝试过「按相对路径猜同级的插件仓库」，但那依赖两个仓库怎么摆放：放在
`Documents/` 下、放进 `MFMCL/` 里、或者克隆到别的盘，猜中的概率都不高，而猜错时
的错误是「往一个新建的空目录里写 36 张图」，比报错更难发现。

所以改为：有 `MFMCL_ARIA_OUT_DIR` 就用它；没有就打印应该怎么设，然后退出。
"""


def resolve_out_dir() -> Path:
    configured = os.environ.get("MFMCL_ARIA_OUT_DIR")
    if configured:
        return Path(configured)
    print(
        "请设置 MFMCL_ARIA_OUT_DIR 指向插件仓库的 assets 目录，例如：\n"
        "  PowerShell:  $env:MFMCL_ARIA_OUT_DIR='C:\\Users\\you\\Documents\\mfmcl-aria-with-you\\assets'\n"
        "  bash:        export MFMCL_ARIA_OUT_DIR=~/Documents/mfmcl-aria-with-you/assets\n"
        "\n素材不再写进启动器仓库（public/plugins/），那里已经没有这个目录。",
        file=sys.stderr,
    )
    raise SystemExit(2)


OUT_DIR = resolve_out_dir()

# ---------------------------------------------------------------------------
# The character
#
# Described in full rather than referenced by filename: the API takes text, and
# a reference image cannot be uploaded from this environment. The description is
# deliberately concrete about the four things a model tends to drift on — hair
# colour, the green cube braid, the uniform, and the pink axolotl — because those
# are what make her recognisable across eight separate generations.
# ---------------------------------------------------------------------------

# ---------------------------------------------------------------------------
# The hair clip — described once, used verbatim everywhere
#
# ## Why this is its own block, and why it is this specific
#
# The clip is the single most identity-bearing detail on the character: it is the
# one asymmetrical, saturated-coloured thing on an otherwise cream-and-beige
# figure, so it is what a player recognises at 96 px. It was also the detail that
# drifted most. Across a review of eight generations:
#
#   - three of four heroes rendered it as 2-4 equal cubes instead of a tapering
#     braid (the description said "a chunky braid made of stacked cubes", which
#     is satisfiable by three cubes);
#   - one hero dropped it entirely and put a white daisy in its place, because the
#     seasonal scene text also asked for a flower near the head;
#   - one bust grew a tassel on the end that the reference does not have.
#
# So the description now states the four things that were missing: the wide top
# block, the count, the taper, and the absence of any dangly end. It names a side
# because the reference is asymmetric, and it forbids a second hair ornament
# because two competing head decorations is how the clip got displaced.
#
# `audit.py` checks the rendered result against this: area, side, and the vertical
# aspect of the blob. Segment counting is reported but not enforced — it proved
# unreliable across artists' shading.
# ---------------------------------------------------------------------------

# ---------------------------------------------------------------------------
# The hair clip — specified from measurement, not from adjectives
#
# ## Where these numbers come from
#
# The user supplied a close-up of the clip itself. Sampled from it:
#
#   - colour   RGB(189, 214, 153)   → #bdd699, a soft yellow-green (not mint,
#                not lime; green leads red by 25 and blue by 61)
#   - shape    h/w = 1.81           → a tall, narrow hanging shape
#   - top      one wide horizontal block, about 2.2:1 wide-to-tall
#   - below    ~6 cubes, each rotated slightly, stepping smaller downward,
#              the last one a small diamond, tip pointing down-left
#   - side     viewer's left, in front of the ear, top edge at temple height
#   - it is flat-shaded with a crisp outline; no tassel, no ribbon, no flower
#
# Earlier versions said "a chunky braid of stacked cubes", which three equal
# cubes satisfy. Those produced 2-4 cube renders, one daisy, and one tassel.
# ---------------------------------------------------------------------------

HAIR_CLIP = (
    "Hair clip on the viewer's left side of her head, in front of the ear, its "
    "top edge level with the temple, hanging down past the jaw. It is a braid of "
    "yellow-green cubes, roughly three times taller than it is wide overall: at "
    "the top a single wide horizontal rectangular slab about 2.2 times wider than "
    "tall, and beneath it six small cube links in a single vertical run, each "
    "cube rotated slightly against the last and each one smaller than the cube "
    "above, ending in a small diamond tilted so its tip points down and to the "
    "left. The colour is exactly #bdd699, a soft yellow-green, flat-shaded with "
    "crisp thin outlines. There is no tassel, no ribbon, and no flower in her "
    "hair: this cube braid is the only hair ornament she wears."
)

# ---------------------------------------------------------------------------
# Identity, pinned to measurements taken from the two supplied references
#
# ## Why the colours are now numeric
#
# "Cream-blonde" and "blue-grey" are adjectives the model is free to read as
# "pale yellow" and "sky blue", and across the first two sets it did: hair came
# back anywhere from (241,218,196) to (253,232,207), and the skirt drifted from
# blue-grey toward cyan in the summer render.
#
# The two references give the actual values:
#
#   reference          hair                eyes            skirt
#   -----------------  ------------------  --------------  ------------------
#   09dbdc1c… (side)   (223, 206, 193)     orange          (156, 165, 191)
#   4c61ff0d… (in-game)(197, 172, 155)*    (194,137, 88)   (165, 200, 243)**
#
#   * the in-game shot is lit by Minecraft's warm sun, so it reads darker than
#     the character sheet; the sheet is the neutral reference.
#   ** the in-game skirt is lit by open sky and reads bluer than the sheet.
#
# So the sheet wins where they disagree, and the numbers below are the sheet's.
# Stating them as hex removes the ambiguity that adjectives leave.
# ---------------------------------------------------------------------------

IDENTITY = (
    "Her colours are fixed and must not shift: hair a pale warm cream "
    "(#dfcec1, mid-tone #d6c3b4), irises amber-orange (#c2894f) with a darker "
    "top and a soft pink blush on the cheeks. The skirt is a muted blue-grey "
    "(#9ca5bf) with visible pleats, the socks are white (#f7f7f5), the loafers "
    "are warm dark brown (#6b4b3a), and the blazer is the same warm beige as her "
    "hair but a shade deeper (#cfbda8)."
)

CHARACTER = (
    "Aria, a petite chibi anime girl, soft pastel illustration with thin clean "
    "line art and flat cel shading. Long straight cream-blonde hair reaching "
    "mid-back with softly pointed ends, side-swept bangs, one small strand "
    "sticking up from the crown. Large amber-orange eyes with a gentle blush. "
    "Wearing a beige blazer over a white collared shirt with a ribbon tie, a "
    "blue-grey pleated skirt, white socks and brown loafers. She carries a pale "
    "blue notebook, and a small pink axolotl plush rides on top of it. "
    + HAIR_CLIP
    + " "
    + IDENTITY
)

# ---------------------------------------------------------------------------
# Rendering rules
#
# Split in two on purpose. Everything here that mentions a pose or a crop used to
# sit in one `STYLE` string appended to both crops, which made the portrait
# prompt say "head and shoulders only" and "full body, single character,
# standing" in the same breath. The model obliged both: it drew a close-up on
# the left and a small full-body copy on the right, as a character sheet. Three
# of the four portraits shipped like that.
#
# The fix is structural, not a stronger adjective: the shared half may only talk
# about things true of every crop (lighting, background, cleanliness), and the
# pose half is supplied by the crop. A prompt can then never contradict itself,
# no matter which crop is added later.
# ---------------------------------------------------------------------------

SHARED = (
    "Soft even lighting, no harsh shadows. Plain transparent background, the "
    "character isolated with clean edges, no backdrop, no ground, no text, no "
    "watermark, no border."
)

# Appended to every crop: the single most common failure of these models is
# drawing the same character several times. Naming the artefacts explicitly
# (sheet, turnaround, multiple views) works better than "one character".
ONE_SUBJECT = (
    "Exactly ONE character and ONE drawing on the canvas. No second copy, no "
    "smaller repeat of the same character elsewhere in the frame, no character "
    "sheet, no turnaround, no multiple views, no reference insets."
)

# ---------------------------------------------------------------------------
# Seasons
#
# Each season changes three things at once: the palette, the outfit accent, and
# the prop or gesture. Changing only the colour would make four images of the
# same pose, which reads as a filter rather than as a season.
# ---------------------------------------------------------------------------

SEASONS: dict[str, dict[str, str]] = {
    "spring": {
        "palette": "sakura pink, pale mint and cream",
        "scene": (
            "Spring. She wears the blazer open over a pink-tinged blouse and holds "
            "a single cherry blossom branch in one hand while waving with the "
            "other. A few loose petals drift around her. No flower in her hair: "
            "the mint-green cube clip is the only ornament she wears."
        ),
        "accent": "#e58fb2",
        "label": "Spring",
    },
    "summer": {
        "palette": "clear sky blue, white and warm sand",
        "scene": (
            "Summer. She wears a short-sleeved summer uniform with a light blue "
            "ribbon and holds a cup of shaved ice with a small wooden spoon. Bright "
            "midday light, a hint of sunflower yellow in the ribbon. "
            "She has NO hat: her head is bare so that the mint-green cube clip stays "
            "clearly visible beside her face. No flower in her hair either."
        ),
        "accent": "#2aa6d6",
        "label": "Summer",
    },
    "autumn": {
        "palette": "persimmon orange, maple red and warm brown",
        "scene": (
            "Autumn. She wears a knitted ochre cardigan over the uniform with a "
            "soft scarf, and holds a paper bag with two apples and a takeaway cup "
            "of hot tea. A couple of maple leaves are caught on the notebook and "
            "the sleeve. Nothing is caught in her hair: the mint-green cube clip "
            "stays fully visible and unobstructed."
        ),
        "accent": "#d97742",
        "label": "Autumn",
    },
    "winter": {
        "palette": "frost blue, pale silver and white",
        "scene": (
            "Winter. She wears a navy duffel coat over the uniform, a long knitted "
            "scarf and mittens, and holds a mug of hot cocoa with both hands. The "
            "coat has no hood and she wears no hat, so the mint-green cube clip "
            "stays clearly visible beside her face. A few snowflakes rest on her "
            "shoulders and the mug."
        ),
        "accent": "#6f9fd8",
        "label": "Winter",
    },
}

# Two crops. The hero is a tall portrait shown beside the launch button; the
# portrait is a square bust used in the sidebar and in the settings picker,
# where a full body would be unreadable at 96 px.
#
# `framing` is the *only* place a pose is described. It must not merely differ
# between crops — it has to be the whole statement, because nothing below can
# add a contradictory one.
CROPS: dict[str, dict[str, str]] = {
    "hero": {
        "size": "1024x1536",
        "framing": (
            "Framing: full body from head to shoes, standing, facing the viewer "
            "at a slight angle, with generous empty space above the head and a "
            "small margin at the feet. The whole figure fits inside the frame."
        ),
    },
    "portrait": {
        "size": "1024x1024",
        "framing": (
            "Framing: a single bust portrait in a square 1:1 frame. Head and "
            "shoulders only, cropped just below the collarbone, centred, the "
            "head filling most of the frame so the face stays clearly readable "
            "when scaled down to a 96 pixel avatar. Do not zoom out, do not "
            "include the torso, legs or shoes."
        ),
    },
}


def build_prompt(season: str, crop: str) -> str:
    spec = SEASONS[season]
    return (
        f"{CHARACTER}\n\n{spec['scene']}\n\n"
        f"Colour palette: {spec['palette']}.\n\n"
        f"{CROPS[crop]['framing']}\n\n{ONE_SUBJECT}\n\n{SHARED}"
    )


# ---------------------------------------------------------------------------
# Weather, per season
#
# ## Why weather is separate from the season scene
#
# The season scene already says what she wears and what she holds. What it does
# not say is what the air is doing — and that is what makes a picture read as
# "a July afternoon" rather than "a girl in a summer outfit on a blank plate".
#
# Every entry is written as *observable particles and light*, never as an
# adjective. "Gentle spring breeze" left the model free to draw nothing at all;
# "a few pale petals drifting past her shoulder, warm low sun from the left" is
# something it can actually put on the canvas.
#
# Weather is also the one thing that must not contradict the outfit: snow with a
# short-sleeved blouse, or strong sun with a fur coat, breaks the character's
# world even when every design detail is correct.
# ---------------------------------------------------------------------------

WEATHER: dict[str, str] = {
    "spring": (
        "Weather: mild and clear after a light shower. A few pale sakura petals "
        "drift diagonally past her shoulder, the light is soft and warm from the "
        "upper left, and the air feels newly washed."
    ),
    "summer": (
        "Weather: bright, hot, high summer. Strong sunlight from above, crisp "
        "short shadows, a faint heat shimmer in the air, and one or two pale "
        "cumulus clouds implied by the light. No rain."
    ),
    "autumn": (
        "Weather: cool and breezy, overcast-late-afternoon. Two or three maple "
        "leaves tumble past her, the light is low and golden from the side, and "
        "there is a suggestion of wind in how her hair and scarf move."
    ),
    "winter": (
        "Weather: cold and still, light snowfall. A few soft snowflakes fall "
        "slowly around her, the light is cool and diffuse with no hard shadow, "
        "and her breath would be visible."
    ),
}

# ---------------------------------------------------------------------------
# Poses, one per page
#
# ## The problem this solves
#
# The plugin showed the same figure on every page: one full-body hero on the
# home page and one watermark repeated behind the library, the store, downloads,
# the plugin list and instance detail. Five pages, one drawing. The repetition
# is what makes a decorative figure stop being decorative and start being
# wallpaper.
#
# ## What a pose has to carry
#
# Three things at once, and the ordering matters:
#
#   1. **A different silhouette.** Rotating the torso, raising an arm, crouching
#      — the point is that the shapes differ, not that the props differ.
#   2. **A page-appropriate read.** The launch page gets "about to set off", the
#      store gets "showing you something in the notebook", the download page gets
#      "waiting patiently", for the reason named on each entry.
#   3. **Consistency with the season scene.** The seasonal prop is already in
#      the scene text; a pose that needs a third hand breaks it.
#
# ## One template per page, applied to all four seasons
#
# The brief asked for "one pose per page" *and* poses that change with the date.
# Those two together are 6 pages x 4 seasons = 24 images, and 24 is where a set
# stops being consistent — the model drifts on the clip and the hair by the
# eighth, which is already documented above.
#
# So the rule is: **the pose belongs to the page, and its direction belongs to
# the season.** A page names a gesture that stays recognisable all year; the
# weather, the outfit and the light change underneath it. That is six drawings
# that are obviously different from each other, and four versions of each that
# are obviously the same character in different weather.
# ---------------------------------------------------------------------------

POSES: dict[str, dict[str, str]] = {
    "home": {
        # The launch page. Her reference sheet — and the supplied Minecraft
        # screenshot — both have her waving while holding the notebook, so the
        # launch pose keeps that and adds the one gesture a launcher's main page
        # needs: she is turned slightly toward the button.
        "framing": (
            "Framing: full body from head to shoes, standing, turned three "
            "quarters toward the viewer with her weight on one leg. She waves "
            "with her free hand and tucks the pale blue notebook against her "
            "chest with the other. The axolotl plush peeks over the notebook."
        ),
        "label": "Home",
    },
    "library": {
        # The library page is a shelf of instances. She holds the notebook up and
        # open, as if reading a list — that is the page's job, and it gives a
        # distinct silhouette (both hands occupied, elbows out).
        "framing": (
            "Framing: three-quarter view from the waist up, leaning slightly "
            "forward, holding the pale blue notebook open in both hands and "
            "looking down at it, as though checking a list. The axolotl plush is "
            "tucked under her arm."
        ),
        "label": "Library",
    },
    "store": {
        # The store is browsing. Offering the notebook outward, page turned to the
        # viewer, is the one pose that reads as "look at this" without a speech
        # bubble.
        "framing": (
            "Framing: three-quarter view from the waist up, turned to her right, "
            "presenting the open pale blue notebook toward the viewer with one "
            "hand and pointing at a page with the other index finger, smiling "
            "brightly. The axolotl plush sits on top of the notebook."
        ),
        "label": "Store",
    },
    "downloads": {
        # Downloads is waiting. Standing with both hands behind her back and her
        # head tilted is a distinctly patient silhouette, and it is the only pose
        # in the set with no hands in front of the body.
        "framing": (
            "Framing: three-quarter view from the waist up, standing with both "
            "hands clasped behind her back, head tilted slightly to one side, "
            "looking at the viewer with a patient, encouraging expression. The "
            "notebook is tucked under one arm and the axolotl plush rides on it."
        ),
        "label": "Downloads",
    },
    "plugins": {
        # Plugins is about extending the launcher, so she is mid-adjustment:
        # touching the hair clip. It also puts attention on the one saturated
        # detail that identifies her.
        "framing": (
            "Framing: three-quarter view from the waist up, one hand raised to her "
            "temple, fingertips just touching the mint-green cube clip, as though "
            "adjusting it, chin lifted slightly. The other arm holds the notebook "
            "against her side with the axolotl plush on it."
        ),
        "label": "Plugins",
    },
    "instance-detail": {
        # Instance detail is a management page: crouching to look closely at
        # something low reads as inspecting, and it is the only seated/crouched
        # silhouette in the set, so the page is unmistakable even at a glance.
        "framing": (
            "Framing: crouching slightly with knees bent, one hand on her knee, "
            "leaning in to look closely at the viewer from a lower angle, curious "
            "expression. The pale blue notebook rests against her knee with the "
            "axolotl plush sitting on top of it."
        ),
        "label": "Instance detail",
    },
}

# The size each page's artwork is rendered at. All pages get the tall crop: the
# watermark is a tall column on the right of the content area, and the bust crop
# would letterbox into it. One size also means one aspect to check.
PAGE_SIZE = "1024x1536"

ONE_SUBJECT_PAGE = (
    "Exactly ONE character and ONE drawing on the canvas. No second copy, no "
    "smaller repeat of the same character elsewhere in the frame, no character "
    "sheet, no turnaround, no multiple views, no reference insets."
)


def build_page_prompt(season: str, page: str) -> str:
    """Prompt for one page's seasonal artwork.

    The order is deliberate and mirrors `build_prompt`: character first (so the
    clip description is never truncated), then season scene, then weather, then
    the pose. The pose comes last because it is the most specific instruction and
    the one most likely to conflict with the season scene's props.
    """
    spec = SEASONS[season]
    return (
        f"{CHARACTER}\n\n"
        f"{spec['scene']}\n\n"
        f"Colour palette: {spec['palette']}.\n\n"
        f"{WEATHER[season]}\n\n"
        f"{POSES[page]['framing']}\n\n"
        f"{ONE_SUBJECT_PAGE}\n\n{SHARED}"
    )


def request_image(prompt: str, size: str, *, transparent: bool) -> bytes:
    payload: dict[str, object] = {
        "model": MODEL,
        "prompt": prompt,
        "n": 1,
        "size": size,
    }
    if transparent:
        # Only sent when asked for: an endpoint that does not understand the
        # parameter can reject the whole request, and an opaque image is still
        # usable (the UI can sit it on a tinted panel).
        payload["background"] = "transparent"
        payload["output_format"] = "png"
    body = json.dumps(payload).encode("utf-8")

    # The upstream gateway sends a chunked response and occasionally closes the
    # connection before the last chunk: two of three trial requests ended in
    # `IncompleteRead` after ~2 MB. A truncated JSON body cannot be salvaged, so
    # the only correct handling is to ask again. Billing is per request, which is
    # why the retry count stays small and the backoff is short.
    last: Exception | None = None
    for attempt in range(4):
        request = urllib.request.Request(
            ENDPOINT,
            data=body,
            headers={
                "Authorization": f"Bearer {os.environ['MFMCL_ARIA_IMAGE_KEY']}",
                "Content-Type": "application/json",
            },
            method="POST",
        )
        try:
            with urllib.request.urlopen(request, timeout=300) as response:
                raw = response.read()
            document = json.loads(raw.decode("utf-8"))
        except (http.client.IncompleteRead, urllib.error.URLError, TimeoutError) as error:
            last = error
            wait = 3 * (attempt + 1)
            print(f"  retry {attempt + 1}/4 after {type(error).__name__}; waiting {wait}s")
            time.sleep(wait)
            continue

        data = document.get("data") or []
        if not data:
            raise RuntimeError(f"no image in response: {document!r}"[:400])
        first = data[0]
        if first.get("b64_json"):
            return base64.b64decode(first["b64_json"])
        if first.get("url"):
            with urllib.request.urlopen(first["url"], timeout=300) as response:
                return response.read()
        raise RuntimeError("image entry has neither b64_json nor url")
    raise RuntimeError(f"giving up after 4 attempts: {last}")


def derive(bytes_in: bytes) -> tuple[bytes, int, int]:
    """Re-encode to WebP and report the size.

    WebP rather than PNG: these are pastel illustrations with smooth gradients,
    where PNG lands around 1.5-2.5 MB each and WebP with alpha lands under 300 KB
    for the same picture. Eight of them ship inside the app bundle, so the
    difference is the difference between shipping the feature and not.

    `alpha_quality=100` is not optional. With the default lossy alpha the
    strongest pixel only reached alpha 254, which leaves a faint translucent film
    over the whole character — invisible on a white page, obvious the moment the
    artwork sits on a dark theme.
    """
    try:
        from PIL import Image
    except ImportError:  # pragma: no cover - environment guard
        raise SystemExit("Pillow is required: pip install pillow")
    image = Image.open(io.BytesIO(bytes_in)).convert("RGBA")
    buffer = io.BytesIO()
    image.save(buffer, "WEBP", quality=92, method=6, alpha_quality=100)
    return buffer.getvalue(), image.width, image.height


# The sidebar figure renders at 96 CSS pixels, so a 1024 px portrait is about a
# megabyte of decode work for something the user sees as a thumbnail. A 384 px
# variant keeps 4x headroom for HiDPI and drops to roughly a tenth of the bytes.
THUMB_MAX = 384


def validate_crop(data: bytes, crop: str) -> str | None:
    """Reject a render whose framing does not match what was asked for.

    ## Why a check and not just a better prompt

    The prompt is now internally consistent, but a prompt is a request, not a
    contract. The failure this catches is not subtle: three of the first four
    portraits contained the character twice, and the only way that was noticed
    was a human looking at each file. A reviewer who is looking at eight images
    in a row will miss the second figure sooner or later, and a wrong asset
    ships and then gets scaled to 96 px where nobody can tell.

    ## The two signals

    A bust portrait *must* be cut off by the frame, so its subject has to touch
    the bottom edge. A full-body hero *must not* be, or the shoes are outside the
    picture. That single fact separates the two cases better than any heuristic
    about how much of the canvas is covered.

    The second signal catches the duplicate: a column band that is completely
    empty but has content on both sides means the frame holds two separate
    things. Nothing in a legitimate single portrait produces that, because hair
    and shoulders span the middle of the image.

    Returns `None` when the render looks right, or a one-line reason.
    """
    from PIL import Image
    import numpy as np

    image = Image.open(io.BytesIO(data)).convert("RGBA")
    solid = np.array(image.getchannel("A")) > 24
    height, width = solid.shape
    edge = max(2, round(height * 0.004))
    bottom = solid[-edge:, :].mean()

    if crop == "portrait" and bottom < 0.15:
        # A bust that does not reach the bottom edge means the model zoomed out
        # — usually because it also drew a full body to satisfy the old prompt.
        return f"portrait does not reach the bottom edge (bottom coverage {bottom:.0%})"
    if crop == "hero" and bottom > 0.15:
        return f"hero is cropped at the bottom (bottom coverage {bottom:.0%})"

    coverage = solid.mean(axis=0)
    empty = coverage < 0.01
    run = 0
    for index, blank in enumerate(empty):
        if blank:
            run += 1
            continue
        if run > width * 0.045 and index - run > width * 0.03:
            return f"two subjects: {run / width:.0%} of the width is empty inside the frame"
        run = 0
    return None


def validate_clip(data: bytes, crop: str) -> str | None:
    """Reject a render whose hair clip is not the braid from the reference sheet.

    ## Why this is checked at generation time

    The clip is the detail that drifted most across the first set — two to four
    equal cubes instead of a tapering braid, one render with a daisy instead of
    it, one with a tassel the reference does not have. Every one of those was
    found by a human comparing eight images to a reference sheet, which is
    exactly the review that gets skipped when the eighth image is the one that
    fails.

    ## What is checked

    The same measurements `audit.py` reports, via the same code, so the two
    cannot disagree:

      - **presence** (area) — catches "the clip is gone / replaced by a flower";
      - **side** — the reference is asymmetric;
      - **vertical aspect** — catches "one oversized cube" and "a row of beads".

    The full-body crops are checked; a bust frames the head so tightly that the
    clip can be partly outside the frame, which would make the area test lie.

    Returns `None` when the clip looks right, or a one-line reason.
    """
    if crop not in ("hero", "page"):
        # A bust may legitimately crop the braid's tail; only presence is
        # meaningful there, and a false rejection costs another generation plus
        # the risk of a worse render. Everything that is not a bust is a
        # full-body drawing, so it gets the full check.
        return None

    from PIL import Image
    import numpy as np

    sys.path.insert(0, str(HERE))
    import audit

    image = Image.open(io.BytesIO(data)).convert("RGBA")
    rgb = np.array(image)[:, :, :3].astype(np.int16)
    alpha = np.array(image.getchannel("A"))
    box = audit.figure_box(alpha)
    clip = audit.clip_stats(rgb, alpha, box)

    if clip["area_fraction"] < audit.CLIP_MIN_FRACTION * 2:
        # Twice the audit floor: a render at exactly the floor has a braid that
        # is barely there, and regenerating is cheaper than shipping that for a
        # whole season.
        return (
            f"hair clip missing or too small "
            f"({clip['area_fraction']:.4f} of the frame)"
        )
    if clip["side"] != "left":
        return f"hair clip on the {clip['side']}, must be on the viewer's left"
    # ## Thresholds from the reference measurement
    #
    # The supplied close-up measures h/w = 1.81 for the clip alone. With the
    # hand-drawn outline and shading around it in a full-body render, the same
    # clip measures 1.1-2.0 depending on how far the head is tilted, because the
    # rotation tips the long axis toward horizontal. A single wide block measures
    # near 0.5, which is the case this check exists to reject.
    if clip["aspect"] < 0.9:
        return (
            f"hair clip is not a vertical braid ({clip['aspect']:.2f}:1, "
            "reference is 1.81:1)"
        )
    return None


def main_pages(argv: list[str]) -> int:
    """Generate the per-page artwork: one pose template per page, four seasons.

    ## Output layout

    `public/plugins/aria-with-you/pages/aria-<page>-<season>.webp` plus a
    `-thumb.webp` beside it. A separate directory rather than more files in the
    plugin root: the root already holds the hero/portrait set that the launcher's
    own layout uses, and mixing 24 more files in makes `ls` useless.

    ## Cost control

    The pose is the part that matters most here, and it is also the part the
    model gets wrong most often — a "presenting the notebook" pose that comes
    back as a plain standing figure defeats the whole feature. So each image gets
    the same three-attempt retry as the seasonal set, and a page that fails all
    three is reported by name rather than silently skipped.
    """
    key_env = os.environ.get("MFMCL_ARIA_IMAGE_KEY")
    if not key_env:
        print("MFMCL_ARIA_IMAGE_KEY is not set", file=sys.stderr)
        return 2

    pages_dir = OUT_DIR / "pages"
    pages_dir.mkdir(parents=True, exist_ok=True)

    manifest_path = OUT_DIR / "pages.json"
    manifest: dict[str, object] = {"model": MODEL, "assets": {}, "poses": {}}
    if manifest_path.exists():
        manifest = json.loads(manifest_path.read_text("utf-8"))
        manifest.setdefault("assets", {})
        manifest.setdefault("poses", {})

    # `argv[0]` is the script path and `argv[1]` is the mode word; neither is a
    # filter. Treating them as one silently matched nothing, which looked like a
    # successful run that produced no files.
    only = {a for a in argv[2:] if not a.startswith("-")}
    failures: list[str] = []

    for page, pose in POSES.items():
        for season in SEASONS:
            page_key = f"{page}-{season}"
            if only and page_key not in only and page not in only and season not in only:
                continue
            target = pages_dir / f"aria-{page_key}.webp"
            if target.exists() and not os.environ.get("MFMCL_ARIA_FORCE"):
                print(f"skip {page_key} (exists)")
                continue
            prompt = build_page_prompt(season, page)
            print(f"generate {page_key} [{PAGE_SIZE}] ...", flush=True)
            started = time.time()
            raw = None
            reason = None
            for attempt in range(3):
                try:
                    raw = request_image(prompt, PAGE_SIZE, transparent=True)
                except urllib.error.HTTPError as error:
                    detail = error.read().decode("utf-8", "replace")[:300]
                    failures.append(f"{page_key}: HTTP {error.code} {detail}")
                    print(f"  failed: HTTP {error.code} {detail}", file=sys.stderr)
                    raw = None
                    break
                except Exception as error:
                    failures.append(f"{page_key}: {error}")
                    print(f"  failed: {error}", file=sys.stderr)
                    raw = None
                    break
                # Only the clip check applies here. `validate_crop`'s bottom-edge
                # test exists to tell a bust from a full body; every page image is
                # a full body by construction, so running it would add a failure
                # mode without adding information.
                reason = validate_clip(raw, "page")
                if reason is None:
                    break
                print(f"  rejected ({reason}), attempt {attempt + 1}/3", file=sys.stderr)
                failures.append(f"{page_key}: rejected — {reason}")
            if raw is None:
                continue
            if reason is not None:
                print(
                    f"  keeping the last attempt anyway; review {page_key} by eye",
                    file=sys.stderr,
                )

            webp, width, height = derive(raw)
            target.write_bytes(webp)
            thumb = write_thumb(target)
            manifest["assets"][page_key] = {
                "file": target.name,
                "sha256": hashlib.sha256(webp).hexdigest(),
                "bytes": len(webp),
                "width": width,
                "height": height,
                "page": page,
                "season": season,
                "thumb": thumb,
                "prompt": prompt,
            }
            print(
                f"  ok {len(webp)} bytes (+{thumb['bytes']} thumb) "
                f"in {time.time() - started:.0f}s",
                flush=True,
            )

    manifest["poses"] = {
        page: {"label": pose["label"], "framing": pose["framing"]}
        for page, pose in POSES.items()
    }
    manifest["weather"] = WEATHER
    manifest["generated_at"] = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
    manifest_path.write_text(
        json.dumps(manifest, indent=2, ensure_ascii=False) + "\n", "utf-8"
    )
    print(f"\nmanifest: {manifest_path}")
    print(f"assets:   {pages_dir}")

    if failures:
        print("\nfailures:", file=sys.stderr)
        for failure in failures:
            print(f"  {failure}", file=sys.stderr)
        return 1
    return 0


def write_thumb(source: Path) -> dict[str, object]:
    """Write `<name>-thumb.webp` next to `source` and describe it."""
    from PIL import Image

    image = Image.open(source).convert("RGBA")
    ratio = min(1.0, THUMB_MAX / max(image.width, image.height))
    size = (max(1, round(image.width * ratio)), max(1, round(image.height * ratio)))
    image = image.resize(size, Image.LANCZOS)
    buffer = io.BytesIO()
    image.save(buffer, "WEBP", quality=88, method=6, alpha_quality=100)
    data = buffer.getvalue()
    target = source.with_name(f"{source.stem}-thumb.webp")
    target.write_bytes(data)
    return {
        "file": target.name,
        "sha256": hashlib.sha256(data).hexdigest(),
        "bytes": len(data),
        "width": size[0],
        "height": size[1],
    }


def main(argv: list[str]) -> int:
    if "MFMCL_ARIA_IMAGE_KEY" not in os.environ:
        print("MFMCL_ARIA_IMAGE_KEY is not set", file=sys.stderr)
        return 2
    only = set(argv[1:]) or None
    OUT_DIR.mkdir(parents=True, exist_ok=True)

    manifest_path = OUT_DIR / "manifest.json"
    manifest: dict[str, object] = {"model": MODEL, "assets": {}}
    if manifest_path.exists():
        manifest = json.loads(manifest_path.read_text("utf-8"))
        manifest.setdefault("assets", {})

    failures: list[str] = []
    for season in SEASONS:
        for crop, crop_spec in CROPS.items():
            key = f"{season}-{crop}"
            if only and key not in only and season not in only:
                continue
            target = OUT_DIR / f"aria-{key}.webp"
            if target.exists() and not os.environ.get("MFMCL_ARIA_FORCE"):
                print(f"skip {key} (exists)")
                continue
            prompt = build_prompt(season, crop)
            print(f"generate {key} [{crop_spec['size']}] ...", flush=True)
            started = time.time()
            raw, reason = None, None
            # Retry a *wrong* render, not just a failed request: a duplicated
            # character costs one more generation but shipping it costs a visibly
            # broken sidebar for a whole season.
            for attempt in range(3):
                try:
                    raw = request_image(prompt, crop_spec["size"], transparent=True)
                except urllib.error.HTTPError as error:
                    detail = error.read().decode("utf-8", "replace")[:300]
                    failures.append(f"{key}: HTTP {error.code} {detail}")
                    print(f"  failed: HTTP {error.code} {detail}", file=sys.stderr)
                    raw = None
                    break
                except Exception as error:  # network, JSON, missing field
                    failures.append(f"{key}: {error}")
                    print(f"  failed: {error}", file=sys.stderr)
                    raw = None
                    break
                # Two independent checks, and the reason is reported separately:
                # "the frame has two characters" and "the clip is wrong" call for
                # different prompt fixes, and a bare "rejected" tells neither.
                reason = validate_crop(raw, crop)
                if reason is None:
                    reason = validate_clip(raw, crop)
                if reason is None:
                    break
                print(f"  rejected ({reason}), attempt {attempt + 1}/3", file=sys.stderr)
                failures.append(f"{key}: rejected — {reason}")
            if raw is None:
                continue
            if reason is not None:
                print(f"  keeping the last attempt anyway; review {key} by eye", file=sys.stderr)

            webp, width, height = derive(raw)
            target.write_bytes(webp)
            thumb = write_thumb(target)
            manifest["assets"][key] = {
                "file": target.name,
                "sha256": hashlib.sha256(webp).hexdigest(),
                "bytes": len(webp),
                "width": width,
                "height": height,
                "season": season,
                "crop": crop,
                "accent": SEASONS[season]["accent"],
                "thumb": thumb,
                "prompt": prompt,
            }
            print(
                f"  ok {len(webp)} bytes (+{thumb['bytes']} thumb) "
                f"in {time.time() - started:.0f}s",
                flush=True,
            )

    manifest["generated_at"] = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
    manifest["seasons"] = {
        name: {
            "label": spec["label"],
            "accent": spec["accent"],
            "palette": spec["palette"],
        }
        for name, spec in SEASONS.items()
    }
    manifest_path.write_text(json.dumps(manifest, indent=2, ensure_ascii=False) + "\n", "utf-8")

    if failures:
        print("\nfailures:", file=sys.stderr)
        for failure in failures:
            print(f"  {failure}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    # Two modes, selected by the first argument:
    #   gen.py                      the hero/portrait set used by the home page
    #                               and the sidebar
    #   gen.py pages [page|season]  one pose template per page, four seasons each
    if len(sys.argv) > 1 and sys.argv[1] == "pages":
        raise SystemExit(main_pages(sys.argv))
    raise SystemExit(main(sys.argv))
