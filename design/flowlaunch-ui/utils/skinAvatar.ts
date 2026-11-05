// 皮肤头像渲染。
//
// ## 为什么在本地画，而不是请求第三方头像服务
//
// crafatar、minotar 这类服务接受 UUID 并返回渲染好的头像，用起来只要一个 URL。
// 但那个 URL 会把「这个玩家是谁」发给一个与启动器无关的第三方——而启动器本来
// 只在登录时与 Microsoft、Mojang 通信。为了一张 32 像素的图引入新的接收方，代价
// 与收益不成比例。
//
// 本地渲染不需要网络：皮肤 PNG 是官方接口本来就会给的（或用户在设置里选的），
// 从它裁出脸即可，全部在 canvas 里完成。
//
// ## 皮肤格式
//
// 现代格式是 64×64，脸在 `(8,8)` 的 8×8 区域，帽子层（发型/眼镜/头盔）在
// `(40,8)`，需要叠在脸上一起画——只画脸会丢掉大多数皮肤的特征。
//
// 旧格式是 64×32，帽子层同样在 `(40,8)`，但脸的下半部分不可用。这里按图片实际
// 高度判断，不做版本猜测。

/** 头像的渲染尺寸（CSS 像素）。像素画放大用整数倍最清晰。 */
const FACE_PX = 8;
/** 帽子层在图集中的偏移。两种格式相同。 */
const HAT_X = 40;
const HAT_Y = 8;
const FACE_X = 8;
const FACE_Y = 8;

export interface SkinAvatarInput {
  /** 皮肤图片的 data URL 或 blob URL。读不到时返回 null。 */
  source: string;
}

/**
 * 从皮肤图里裁出「脸 + 帽子层」，返回 data URL。
 *
 * 失败时返回 `null`：调用方据此回落到默认头像，而不是显示破图。失败原因可能是
 * 图片损坏、跨域（远程皮肤没有 CORS 头）或 canvas 被禁用。
 */
export async function renderSkinAvatar({ source }: SkinAvatarInput): Promise<string | null> {
  if (!source) return null;
  if (typeof document === 'undefined') return null;

  const image = await loadImage(source).catch(() => null);
  if (!image) return null;

  // 至少要有 64×32 才能取到脸与帽子层；更大的图按比例缩放没有意义（皮肤不是矢量）。
  if (image.naturalWidth < 64 || image.naturalHeight < 32) return null;

  const canvas = document.createElement('canvas');
  // 8×8 的源区域放大到 128，保证在 2x/3x 屏上也清晰。
  const scale = 16;
  canvas.width = FACE_PX * scale;
  canvas.height = FACE_PX * scale;
  const context = canvas.getContext('2d');
  if (!context) return null;

  // 像素画必须关掉平滑，否则放大后变成一团糊。
  context.imageSmoothingEnabled = false;

  // 先画脸。
  context.drawImage(
    image,
    FACE_X, FACE_Y, FACE_PX, FACE_PX,
    0, 0, canvas.width, canvas.height,
  );

  /*
   * 再叠帽子层。
   *
   * 用 `source-over` 而不是「有像素就替换」：帽子层是带 alpha 的贴图，透明区域
   * 应当让底下的脸透出来。直接覆盖会让透明部分变成黑色。
   */
  context.drawImage(
    image,
    HAT_X, HAT_Y, FACE_PX, FACE_PX,
    0, 0, canvas.width, canvas.height,
  );

  try {
    return canvas.toDataURL('image/png');
  } catch {
    // 跨域图片会污染 canvas，`toDataURL` 抛安全错误。
    return null;
  }
}

function loadImage(source: string): Promise<HTMLImageElement> {
  return new Promise((resolve, reject) => {
    const image = new Image();
    // 远程皮肤需要 CORS 才能读像素；data/blob URL 不受影响。
    image.crossOrigin = 'anonymous';
    image.onload = () => resolve(image);
    image.onerror = () => reject(new Error('skin load failed'));
    image.src = source;
  });
}

/**
 * 皮肤设置的默认值。与 `SkinSection` 里的 `DEFAULT_SKIN_SETTING` 对应，
 * 但这里只关心「有没有一张可用的图」。
 */
export function skinSourceOf(setting: { type: string; customDataUrl?: string }): string | null {
  if (setting.type === 'custom' && setting.customDataUrl) {
    return setting.customDataUrl;
  }
  return null;
}
