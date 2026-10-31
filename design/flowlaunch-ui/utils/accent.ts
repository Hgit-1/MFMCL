// Accent colour helpers.
//
// The launcher stores one accent colour but the UI needs two families from it:
//
//   Tints (500 / 600 / 700) - the stored accent itself, used for borders,
//                         focus rings, tonal washes, progress bars and any
//                         decoration that has no text sitting on it.
//   Tonal (tonal / tonal-text)
//                       - the "selected row" treatment: the accent mixed a
//                         little way into the elevated surface, with the accent
//                         as text on top. Used by both navigation rails so a
//                         selected item reads as a tint, not as a painted block.
//   Solid (solid / solid-hover / solid-active)
//                       - the accent at full strength, used as the fill behind
//                         text on primary buttons. The presets (deep-sea blue,
//                         tundra green, ember orange, ...) are light and airy by
//                         design, so the fill keeps the accent's exact value and
//                         the *foreground* carries the contrast instead: near
//                         black ink reads at 5.9-9.2:1 on every preset. Only a
//                         user's own very dark accent has to be lifted, and then
//                         only in OKLab lightness with hue and chroma pinned.
//   --color-on-primary  - near black ink, the foreground on a solid accent fill.
//   --color-primary-text-light/dark - the accent as *text/icon* colour,
//                         nudged in the theme's direction until it is readable.

export const DEFAULT_ACCENT = '#53b4f6';

/**
 * Near black used for text/icons sitting on a solid accent fill. Kept a shade
 * off pure black (#0A0E14, the dark theme page colour) so buttons feel like part
 * of the same palette rather than a stamped rectangle.
 */
export const ON_ACCENT = '#0a0e14';
const WHITE = '#ffffff';
/** Page background the accent text sits on, per theme. */
const LIGHT_BG = '#f9fafb';
const DARK_BG = '#0a0e14';
/** Elevated surface (cards, rails) the tonal wash is mixed into, per theme. */
const LIGHT_SURFACE = '#ffffff';
const DARK_SURFACE = '#1f2937';
/** WCAG AA for normal-size text. */
const MIN_CONTRAST = 4.5;
/**
 * Solid fills and tonal rows aim slightly above AA. Contrast is computed on
 * rounded hex values, and antialiased text on a saturated fill reads thinner
 * than the ratio suggests, so keep a small margin instead of landing exactly
 * on 4.5.
 */
const SOLID_CONTRAST = 4.6;
const TONAL_CONTRAST = 4.6;
/**
 * How far the accent is mixed into the elevated surface for a selected row.
 * Low enough to stay a wash rather than a block, high enough that the row is
 * obviously selected next to its neighbours.
 */
const TONAL_MIX = 0.12;

const HEX_PATTERN = /^#?([0-9a-f]{3}|[0-9a-f]{6})$/i;

/** Normalise `#ABC` / `45a7e8` style input to lowercase `#rrggbb`, or null. */
export function normalizeHex(value: string): string | null {
  const match = HEX_PATTERN.exec(value.trim());
  if (!match) return null;
  let body = match[1].toLowerCase();
  if (body.length === 3) body = body.split('').map(char => char + char).join('');
  return `#${body}`;
}

function toRgb(color: string): [number, number, number] | null {
  const normalized = normalizeHex(color);
  if (!normalized) return null;
  return [1, 3, 5].map(offset => parseInt(normalized.slice(offset, offset + 2), 16)) as [number, number, number];
}

function relativeLuminance([r, g, b]: [number, number, number]): number {
  const channel = (value: number) => {
    const scaled = value / 255;
    return scaled <= 0.03928 ? scaled / 12.92 : Math.pow((scaled + 0.055) / 1.055, 2.4);
  };
  return 0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b);
}

/** WCAG contrast ratio between two hex colours. */
export function contrastRatio(a: string, b: string): number {
  const first = toRgb(a);
  const second = toRgb(b);
  if (!first || !second) return 1;
  const l1 = relativeLuminance(first);
  const l2 = relativeLuminance(second);
  return (Math.max(l1, l2) + 0.05) / (Math.min(l1, l2) + 0.05);
}

function shift(color: string, towards: [number, number, number], amount: number): string {
  const rgb = toRgb(color);
  if (!rgb) return color;
  const ratio = Math.min(1, Math.max(0, amount));
  return `#${rgb
    .map((channel, index) => Math.round(channel + (towards[index] - channel) * ratio))
    .map(channel => channel.toString(16).padStart(2, '0'))
    .join('')}`;
}

/** Mix the colour towards black; `amount` is how much black is added (0-1). */
export function darkenHex(color: string, amount: number): string {
  return shift(color, [0, 0, 0], amount);
}

/**
 * Linear RGB blend of two hex colours; `weight` is the share of `foreground`
 * (0 = all `background`, 1 = all `foreground`). Used for tonal washes, where
 * the accent should contribute a little colour to an existing surface rather
 * than replace it.
 */
export function mixHex(foreground: string, background: string, weight: number): string {
  const front = toRgb(foreground);
  const back = toRgb(background);
  if (!front || !back) return background;
  const ratio = Math.min(1, Math.max(0, weight));
  return `#${front
    .map((channel, index) => Math.round(back[index] + (channel - back[index]) * ratio))
    .map(channel => channel.toString(16).padStart(2, '0'))
    .join('')}`;
}

/**
 * Step the colour towards `ink` or `white` until it reaches `min` contrast
 * against `background`. Returns the input unchanged when it already passes.
 */
function ensureContrast(color: string, background: string, lighter: boolean, min = MIN_CONTRAST): string {
  if (contrastRatio(color, background) >= min) return color;
  const target: [number, number, number] = lighter ? [255, 255, 255] : [0, 0, 0];
  for (let step = 1; step <= 20; step += 1) {
    const candidate = shift(color, target, step * 0.05);
    if (contrastRatio(candidate, background) >= min) return candidate;
  }
  return lighter ? WHITE : '#000000';
}

// ---------------------------------------------------------------------------
// OKLab
//
// The solid family is derived in OKLab rather than HSL. HSL is not perceptually
// uniform: it counts "lightness" as (max + min) / 2, so dropping lightness on
// 樱花粉 or 余烬橙 saturates the result relative to how it started and the hue
// visibly tilts (pink turns coral, orange turns rust). OKLab separates
// perceptual lightness (L) from hue and chroma, so lowering only L leaves the
// colour recognisably the one the user picked.
//
// Chroma is pinned, not boosted: at a lower L a saturated colour leaves the
// sRGB cube, so the fill is pulled back to the gamut hull at that L and hue —
// the most vivid version of the same colour that can carry ivory text.
// ---------------------------------------------------------------------------

type Oklab = [number, number, number];

function srgbToLinear(value: number): number {
  const scaled = value / 255;
  return scaled <= 0.04045 ? scaled / 12.92 : Math.pow((scaled + 0.055) / 1.055, 2.4);
}

function linearToSrgb(value: number): number {
  const scaled = value <= 0.0031308 ? value * 12.92 : 1.055 * Math.pow(value, 1 / 2.4) - 0.055;
  return Math.min(1, Math.max(0, scaled)) * 255;
}

function rgbToOklab(rgb: [number, number, number]): Oklab {
  const [r, g, b] = rgb.map(srgbToLinear) as [number, number, number];
  const l = Math.cbrt(0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b);
  const m = Math.cbrt(0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b);
  const s = Math.cbrt(0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b);
  return [
    0.2104542553 * l + 0.793617785 * m - 0.0040720468 * s,
    1.9779984951 * l - 2.428592205 * m + 0.4505937099 * s,
    0.0259040371 * l + 0.7827717662 * m - 0.808675766 * s,
  ];
}

/** OKLab to linear sRGB. Components may fall outside 0-1; callers gamut-map. */
function oklabToLinear([lightness, a, b]: Oklab): [number, number, number] {
  const l = (lightness + 0.3963377774 * a + 0.2158037573 * b) ** 3;
  const m = (lightness - 0.1055613458 * a - 0.0638541728 * b) ** 3;
  const s = (lightness - 0.0894841775 * a - 1.291485548 * b) ** 3;
  return [
    4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s,
    -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s,
    -0.0041960863 * l - 0.7034186147 * m + 1.707614701 * s,
  ];
}

/** Slack for float error at the gamut boundary before a colour counts as clipped. */
const GAMUT_EPSILON = 1e-4;

function inGamut(rgb: [number, number, number]): boolean {
  return rgb.every(channel => channel >= -GAMUT_EPSILON && channel <= 1 + GAMUT_EPSILON);
}

/**
 * Convert OKLab to hex, keeping hue and L and clipping only chroma when the
 * colour sits outside sRGB. Binary search finds the largest in-gamut chroma,
 * clamped at the origin so a neutral always has a legal answer.
 */
function oklabToHex([lightness, a, b]: Oklab): string {
  let linear = oklabToLinear([lightness, a, b]);
  if (!inGamut(linear)) {
    let low = 0;
    let high = 1;
    for (let step = 0; step < 24; step += 1) {
      const mid = (low + high) / 2;
      if (inGamut(oklabToLinear([lightness, a * mid, b * mid]))) low = mid;
      else high = mid;
    }
    linear = oklabToLinear([lightness, a * low, b * low]);
  }
  return `#${linear.map(channel => Math.round(linearToSrgb(channel)).toString(16).padStart(2, '0')).join('')}`;
}

/**
 * Move perceptual lightness by `amount`, holding hue and chroma. Used for the
 * solid hover/active shades so the whole button family stays one colour.
 * Positive steps lighten, negative steps darken.
 */
function stepLightness(color: string, amount: number): string {
  const rgb = toRgb(color);
  if (!rgb) return color;
  const [lightness, a, b] = rgbToOklab(rgb);
  return oklabToHex([Math.min(1, Math.max(0, lightness + amount)), a, b]);
}

/**
 * Perceptual lightness deltas for the hover/active solid shades. Small and
 * evenly spaced: the fill now carries dark text at a generous ratio, so a
 * heavier step would read as a colour change rather than a press.
 */
const SOLID_HOVER_STEP = -0.03;
const SOLID_ACTIVE_STEP = -0.06;

/**
 * The accent as a button fill that can carry `ON_ACCENT` text at AA.
 *
 * The presets are light by design, so ink lands at 5.9-9.2:1 on the accent
 * exactly as picked — the button finally looks like the colour the user chose
 * instead of a darkened cousin of it. Only an accent too dark for ink (a user's
 * own near-navy, say) is adjusted, and then only upward in OKLab lightness with
 * hue and chroma pinned, which is why a preset accent keeps its exact value.
 */
export function solidFill(color: string): string {
  const accent = normalizeHex(color) ?? DEFAULT_ACCENT;
  if (contrastRatio(accent, ON_ACCENT) >= SOLID_CONTRAST) return accent;
  const [lightness, a, b] = rgbToOklab(toRgb(accent)!);
  for (let step = lightness; step < 1; step += 0.002) {
    const candidate = oklabToHex([step, a, b]);
    if (contrastRatio(candidate, ON_ACCENT) >= SOLID_CONTRAST) return candidate;
  }
  return WHITE;
}

/**
 * The accent as a text/icon colour for each theme: darkened for light
 * backgrounds, lightened for dark ones, so it always clears 4.5:1.
 */
export function accentTextColors(color: string): { light: string; dark: string } {
  const accent = normalizeHex(color) ?? DEFAULT_ACCENT;
  return {
    light: ensureContrast(accent, LIGHT_BG, false),
    dark: ensureContrast(accent, DARK_BG, true),
  };
}

/**
 * The "selected row" pair: a wash of the accent mixed into the elevated
 * surface, plus the accent as text on that wash.
 *
 * The tint is deliberately light so a selected nav row reads as a highlight
 * rather than a painted block — the error the old solid fill made at full
 * strength. Mixing darkens the wash on light themes and lightens it on dark
 * ones, so the same 12% mix works in both; the text colour is then nudged
 * until it clears AA on whatever wash came out.
 */
export function accentTonalColors(color: string): {
  light: { background: string; text: string };
  dark: { background: string; text: string };
} {
  const accent = normalizeHex(color) ?? DEFAULT_ACCENT;
  const lightBackground = mixHex(accent, LIGHT_SURFACE, TONAL_MIX);
  const darkBackground = mixHex(accent, DARK_SURFACE, TONAL_MIX);
  return {
    light: { background: lightBackground, text: ensureContrast(accent, lightBackground, false, TONAL_CONTRAST) },
    dark: { background: darkBackground, text: ensureContrast(accent, darkBackground, true, TONAL_CONTRAST) },
  };
}

// ---------------------------------------------------------------------------
// Deriving an accent from an image
//
// The "free background" feature can pick a colour out of the user's own picture.
// Doing that in RGB would fail the same way HSL fails for the solid shades: an
// average in gamma space drifts towards the middle of the cube, so a photo of a
// forest averages to mud. The average is therefore taken in OKLab, weighting
// each pixel by its chroma — the sky and the leaves decide the hue, the grey
// wall behind them does not.
// ---------------------------------------------------------------------------

export interface ImageAnalysis {
  /** Accent derived from the image, or null when the image has no usable pixels. */
  accent: string | null;
  /** Perceptual lightness of the image, 0-1. Used to suggest a light/dark theme. */
  lightness: number;
}

/**
 * How strongly chroma counts relative to a flat pixel.
 *
 * Without the constant term a picture that is 90% grey would be decided by a
 * handful of stray pixels; with only the constant term a single bright accent
 * dot would be lost among the grey. 0.2 + 6 × chroma means vivid pixels outvote
 * dull ones roughly ten to one at typical chroma (≈0.1).
 */
const CHROMA_FLOOR = 0.2;
const CHROMA_WEIGHT = 6;

/**
 * Accent lightness is clamped into this band. Below it the colour becomes a
 * near-black accent (borders and progress bars disappear into the page); above
 * it the colour is so pale that a solid button fill loses its edge. The band
 * covers the shipped presets (深海蓝 sits at ≈0.73).
 */
const ACCENT_L_MIN = 0.45;
const ACCENT_L_MAX = 0.8;

/**
 * Analyse RGBA pixels (as returned by `CanvasRenderingContext2D.getImageData`)
 * and return a representative accent plus the image's lightness.
 *
 * Transparent pixels are skipped, so analysing a PNG with an alpha channel does
 * not get pulled towards black.
 */
export function analyzeImageData(pixels: Uint8ClampedArray): ImageAnalysis {
  let weightSum = 0;
  let aSum = 0;
  let bSum = 0;
  let lightnessSum = 0;
  let lightnessWeight = 0;

  for (let index = 0; index + 3 < pixels.length; index += 4) {
    const alpha = pixels[index + 3];
    // Fully transparent pixels carry no colour; partially transparent ones are
    // weighted by their coverage so a soft edge does not count as much as a solid fill.
    if (alpha < 8) continue;
    const coverage = alpha / 255;
    const [lightness, a, b] = rgbToOklab([pixels[index], pixels[index + 1], pixels[index + 2]]);
    const chroma = Math.hypot(a, b);
    const weight = (CHROMA_FLOOR + CHROMA_WEIGHT * chroma) * coverage;

    aSum += a * weight;
    bSum += b * weight;
    weightSum += weight;

    // Lightness gets a flat average: contrast is about overall brightness, not
    // about which part of the picture is the most colourful.
    lightnessSum += lightness * coverage;
    lightnessWeight += coverage;
  }

  if (weightSum <= 0 || lightnessWeight <= 0) return { accent: null, lightness: 0.5 };

  const lightness = lightnessSum / lightnessWeight;
  const meanA = aSum / weightSum;
  const meanB = bSum / weightSum;
  const accentLightness = Math.min(ACCENT_L_MAX, Math.max(ACCENT_L_MIN, lightness));
  return {
    accent: oklabToHex([accentLightness, meanA, meanB]),
    lightness,
  };
}

/**
 * The accent the launcher should actually use, given a user-chosen colour and
 * whether the background has taken over.
 */
export function pickAccent(userAccent: string, auto: string | null): string {
  if (auto) return normalizeHex(auto) ?? DEFAULT_ACCENT;
  return normalizeHex(userAccent) ?? DEFAULT_ACCENT;
}

/**
 * Write the accent family to the document root. Called before the first paint
 * and whenever the accent changes, so the whole app follows one colour.
 */
export function applyAccent(color: string, target: HTMLElement = document.documentElement): void {
  const accent = normalizeHex(color) ?? DEFAULT_ACCENT;
  const text = accentTextColors(accent);
  const tonal = accentTonalColors(accent);
  const solid = solidFill(accent);
  target.style.setProperty('--color-primary-500', accent);
  target.style.setProperty('--color-primary-600', darkenHex(accent, 0.18));
  target.style.setProperty('--color-primary-700', darkenHex(accent, 0.3));
  target.style.setProperty('--color-primary-solid', solid);
  // Hover/active step in OKLab, so the pressed state stays the same hue instead
  // of drifting as an RGB mix toward white or black would.
  target.style.setProperty('--color-primary-solid-hover', stepLightness(solid, SOLID_HOVER_STEP));
  target.style.setProperty('--color-primary-solid-active', stepLightness(solid, SOLID_ACTIVE_STEP));
  target.style.setProperty('--color-on-primary', ON_ACCENT);
  target.style.setProperty('--color-primary-text-light', text.light);
  target.style.setProperty('--color-primary-text-dark', text.dark);
  target.style.setProperty('--color-primary-tonal-light', tonal.light.background);
  target.style.setProperty('--color-primary-tonal-dark', tonal.dark.background);
  target.style.setProperty('--color-primary-tonal-text-light', tonal.light.text);
  target.style.setProperty('--color-primary-tonal-text-dark', tonal.dark.text);
}
