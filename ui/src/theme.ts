// Theme colours are computed, not trusted as legible against every surface.
function rgb(hex: string): number[] { return [1, 3, 5].map(i => parseInt(hex.slice(i, i + 2), 16) / 255) }
function luminance(hex: string): number {
  const values = rgb(hex).map(c => c <= .04045 ? c / 12.92 : ((c + .055) / 1.055) ** 2.4)
  return values[0] * .2126 + values[1] * .7152 + values[2] * .0722
}
export function contrast(a: string, b: string): number {
  const [low, high] = [luminance(a), luminance(b)].sort((x, y) => x - y)
  return (high + .05) / (low + .05)
}
export function accessibleAccent(input: string, light: boolean) {
  const fill = /^#[\da-f]{6}$/i.test(input) ? input : '#a6d991'
  const ink = contrast(fill, '#ffffff') > contrast(fill, '#000000') ? '#ffffff' : '#000000'
  let text = fill
  const surface = light ? '#ffffff' : '#151f19'
  const destination = light ? [0, 0, 0] : [255, 255, 255]
  const source = rgb(fill).map(c => c * 255)
  for (let i = 0; contrast(text, surface) < 4.5 && i <= 100; i++) {
    const ratio = i / 100
    text = '#' + source.map((c, j) => Math.round(c * (1 - ratio) + destination[j] * ratio).toString(16).padStart(2, '0')).join('')
  }
  return { fill, ink, text }
}
