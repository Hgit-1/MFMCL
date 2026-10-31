// Desktop bridge for Java discovery.
//
// PCL2 scans the machine for Java installations (ModJava.vb: JavaListRefreshWorker),
// verifies each one by running `java -version`, and lets the user import a
// specific java.exe. In the browser preview there is no filesystem, so the same
// calls return the simulated list instead and the UI says so — the rest of the
// flow (ordering, per-instance selection, version ranges) behaves identically.
//
// 这个模块不属于 React，但它的返回值会直接显示在设置面板里。所以返回的是
// `Localized`（词典键 + 参数）而不是拼好的句子，由界面层按当前语言渲染——否则
// 状态文字会停留在调用那一刻的语言上。只有系统对话框的标题和 prompt 文案必须
// 立即成句，因此接受一个语言参数。

import { previewJavaRuntimes, type JavaRuntime } from './java';
import { translate, type Language } from './i18n';
import { loc, locFromError, type Localized } from './i18n/localized';
import {
  inspectJavaPath as nativeInspectJavaPath,
  isDesktopShell as shellIsDesktop,
  listJavaRuntimes as nativeListJavaRuntimes,
  pickFile,
} from './native';
import { PREVIEW_DATA } from './preview';

/**
 * 选择 java 可执行文件。
 *
 * 复用 `native.pickFile`（走 Tauri 的对话框插件），而不是自己再读
 * `window.__TAURI__.dialog`：两套桥接会各自判断一次「是否桌面版」，迟早出现
 * 「探测说在桌面版、导入说在预览」这种自相矛盾的状态。
 *
 * 不按扩展名过滤：Linux/macOS 上的 java 通常没有后缀，加 `exe` 过滤反而会把
 * 它们藏起来。是否有效由后端的 `inspect_java_path` 实际执行来判定。
 */
async function pickJavaExecutable(language: Language): Promise<string | null> {
  return pickFile(translate(language, 'java.importDialogTitle'));
}

interface TauriWindow {
  __TAURI__?: {
    core?: { invoke: <T>(command: string, args?: unknown) => Promise<T> };
    dialog?: { open: (options: unknown) => Promise<string | string[] | null> };
    opener?: { openPath: (path: string) => Promise<void> };
  };
}

function bridge(): TauriWindow['__TAURI__'] {
  return (window as Window & TauriWindow).__TAURI__;
}

// `isDesktopShell` 现在从 `native.ts` 导入：这里曾有一份只看 `__TAURI__.core`
// 的实现，而 `native.ts` 看的是 `__TAURI_INTERNALS__`。两套判断在某些构建下
// （`withGlobalTauri` 关掉时）会得出相反结论，导致「探测认为在桌面版、导入认为
// 在预览」这种自相矛盾的状态。统一用同一份实现。

export interface DetectResult {
  runtimes: JavaRuntime[];
  note: Localized;
  simulated: boolean;
}

/** Scan for Java runtimes. Mirrors PCL2's "自动搜索 Java". */
export async function detectJavaRuntimes(): Promise<DetectResult> {
  if (!shellIsDesktop()) {
    // 浏览器里没有文件系统，探测不出任何真实的 Java。开发预览给一组示例值
    // （否则实例的 Java 下拉框是空的，界面没法调）；生产构建里 PREVIEW_DATA 是
    // 编译期 false，这两块示例数据都不会被打包。
    if (!PREVIEW_DATA) {
      return { runtimes: [], note: loc('java.detectNone'), simulated: false };
    }
    const preview = previewJavaRuntimes();
    return {
      runtimes: preview,
      note: loc('java.previewFound', { count: preview.length }),
      simulated: true,
    };
  }
  try {
    const runtimes = await nativeListJavaRuntimes();
    return {
      runtimes,
      note: runtimes.length ? loc('java.detectFound', { count: runtimes.length }) : loc('java.detectNone'),
      simulated: false,
    };
  } catch (error) {
    return { runtimes: [], note: locFromError('java.detectFailed', error), simulated: false };
  }
}

export interface ImportResult {
  runtime: JavaRuntime | null;
  note: Localized | null;
}

/** Import one Java by picking its executable. Mirrors PCL2's ManuallyImportJava. */
export async function importJavaRuntime(language: Language): Promise<ImportResult> {
  // 桌面版：走系统文件选择 + 后端校验（`inspect_java_path` 会真的跑 `java -version`）。
  const desktop = shellIsDesktop();
  if (!desktop) {
    // 浏览器预览拿不到文件系统，也没有后端可以执行 java。这里**不再编造**一个
    // 「21 / 已添加」的运行时：那会让预览里出现一条真机上并不存在的记录，用户
    // 以为导入成功了。改为明确说明该操作需要桌面版。
    return { runtime: null, note: loc('java.importNeedsDesktop') };
  }
  const selected = await pickJavaExecutable(language);
  if (!selected) return { runtime: null, note: null };
  try {
    const runtime = await nativeInspectJavaPath(selected);
    return { runtime, note: loc('java.importVerified', { major: runtime.majorVersion, path: runtime.path }) };
  } catch (error) {
    return { runtime: null, note: locFromError('java.importInvalid', error) };
  }
}

/** Open the folder that contains the Java executable (PCL2: 打开文件夹). */
export async function revealJavaFolder(runtime: JavaRuntime): Promise<Localized> {
  const trimmed = runtime.path.replace(/[\\/]+$/, '');
  const folder = trimmed.slice(0, Math.max(trimmed.lastIndexOf('/'), trimmed.lastIndexOf('\\'))) || trimmed;
  const opener = bridge()?.opener;
  if (opener) {
    try {
      await opener.openPath(folder);
      return loc('java.revealOpened', { folder });
    } catch { /* fall through to copying the path */ }
  }
  try {
    await navigator.clipboard.writeText(folder);
    return loc('java.revealCopied', { folder });
  } catch {
    return folder;
  }
}
