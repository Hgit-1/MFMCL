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

import { PREVIEW_JAVA_RUNTIMES, type JavaRuntime } from './java';
import { translate, type Language } from './i18n';
import { loc, locFromError, type Localized } from './i18n/localized';

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

export function isDesktopShell(): boolean {
  return Boolean(bridge()?.core?.invoke);
}

export interface DetectResult {
  runtimes: JavaRuntime[];
  note: Localized;
  simulated: boolean;
}

/** Scan for Java runtimes. Mirrors PCL2's "自动搜索 Java". */
export async function detectJavaRuntimes(): Promise<DetectResult> {
  const invoke = bridge()?.core?.invoke;
  if (!invoke) {
    return {
      runtimes: PREVIEW_JAVA_RUNTIMES,
      note: loc('java.previewFound', { count: PREVIEW_JAVA_RUNTIMES.length }),
      simulated: true,
    };
  }
  try {
    const runtimes = await invoke<JavaRuntime[]>('list_java_runtimes');
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
  const tauri = bridge();
  if (!tauri?.dialog || !tauri.core) {
    const path = window.prompt(translate(language, 'java.importPrompt'), '/usr/bin/java');
    if (!path?.trim()) return { runtime: null, note: null };
    return {
      runtime: {
        path: path.trim(),
        majorVersion: 21,
        versionText: translate(language, 'java.importPlaceholder'),
      },
      note: loc('java.importAdded', { path: path.trim() }),
    };
  }
  const selected = await tauri.dialog.open({ multiple: false, title: translate(language, 'java.importDialogTitle') });
  if (typeof selected !== 'string') return { runtime: null, note: null };
  try {
    const runtime = await tauri.core.invoke<JavaRuntime>('inspect_java_path', { path: selected });
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
