// 带超时与 sessionStorage 缓存的 fetch 封装。
//
// 三个数据源的共同需求：8 秒超时、可取消、失败信息说人话、短时间内的重复
// 请求不重复打网络。PCL2 自己也有下载缓存（ModDownload 的缓存文件），这里
// 用 sessionStorage 做「同一标签页生命周期内有效」的轻量等价物：关掉标签页
// 就失效，不会像 localStorage 那样留下过期数据。

import { loc, LocalizedError } from '../i18n/localized';
import { translate } from '../i18n';

const TIMEOUT_MS = 8000;
const CACHE_TTL_MS = 5 * 60 * 1000;
const CACHE_PREFIX = 'flowlaunch-store:';

interface CacheRecord {
  at: number;
  body: unknown;
}

function readCache(key: string): unknown | null {
  try {
    const raw = sessionStorage.getItem(CACHE_PREFIX + key);
    if (!raw) return null;
    const record = JSON.parse(raw) as CacheRecord;
    if (!record || typeof record.at !== 'number') return null;
    if (Date.now() - record.at > CACHE_TTL_MS) {
      sessionStorage.removeItem(CACHE_PREFIX + key);
      return null;
    }
    return record.body;
  } catch {
    return null;
  }
}

function writeCache(key: string, body: unknown): void {
  try {
    sessionStorage.setItem(CACHE_PREFIX + key, JSON.stringify({ at: Date.now(), body } satisfies CacheRecord));
  } catch {
    // 配额满或被禁用：缓存只是加速手段，失败不影响功能。
  }
}

export interface RequestOptions {
  /** 缓存键；不传则不缓存。 */
  cacheKey?: string;
  headers?: Record<string, string>;
  /** 外部取消信号（切换筛选时取消上一次请求）。 */
  signal?: AbortSignal;
  /** 跳过缓存，用于「重试」。 */
  fresh?: boolean;
}

export class HttpError extends Error {
  readonly status: number;
  constructor(status: number, message: string) {
    super(message);
    this.name = 'HttpError';
    this.status = status;
  }
}

/**
 * 发起 GET 请求并解析 JSON。
 *
 * 会抛出的错误：
 *   HttpError      - 非 2xx（status 供调用方区分 403 / 404）
 *   LocalizedError - 网络失败或超时，文案是词典键，由界面层按当前语言渲染
 *   DOMException('AbortError') - 被调用方取消，调用方应静默忽略
 */
export async function getJson<T>(url: string, options: RequestOptions = {}): Promise<T> {
  const { cacheKey, headers, signal, fresh } = options;

  if (cacheKey && !fresh) {
    const hit = readCache(cacheKey);
    if (hit !== null) return hit as T;
  }

  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(new Error('timeout')), TIMEOUT_MS);
  const onAbort = () => controller.abort(new Error('cancelled'));
  signal?.addEventListener('abort', onAbort, { once: true });

  try {
    const response = await fetch(url, { headers, signal: controller.signal, credentials: 'omit' });
    if (!response.ok) {
      // 只在这里把状态码翻成中文，具体文案由各数据源决定。
      throw new HttpError(response.status, `HTTP ${response.status}`);
    }
    const body = (await response.json()) as T;
    if (cacheKey) writeCache(cacheKey, body);
    return body;
  } catch (error) {
    if (error instanceof HttpError) throw error;
    if (signal?.aborted) throw new DOMException(translate('zh-CN', 'http.cancelled'), 'AbortError');
    if (controller.signal.reason instanceof Error && controller.signal.reason.message === 'timeout') {
      throw new LocalizedError(loc('http.timeout'));
    }
    throw new LocalizedError(loc('http.offline'));
  } finally {
    clearTimeout(timer);
    signal?.removeEventListener('abort', onAbort);
  }
}
