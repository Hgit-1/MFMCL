// 后端记录 → 界面模型的适配层。
//
// 为什么需要这一层：界面组件是按 `types.ts` 里的 `GameInstance` / `Account`
// 写的，而后端返回的是 `InstanceRecord` / `AccountProfile`。两套形状不同，但
// 语义基本一一对应。
//
// 选择在边界上做映射，而不是把界面的类型改成后端形状：后者要动所有用到
// `instance.version` / `account.username` 的 JSX，而前者只改一个文件。界面
// 结构保持不变，也就不会因为「接后端」而顺带改掉布局。
//
// 映射原则：后端有的一一对应；后端没有的字段给一个**诚实**的占位值——
//   - `javaVersion` 与 `playTime` 后端不记录 → 空串 / 0，界面会自动隐藏相应文案，
//     而不是编一个假数字；
//   - `modCount` 由调用方按需用 `list_mods` 补上，未取到时为 0。

import type { Account, GameInstance } from '../types';
import type { AccountProfile, InstanceRecord, LauncherSettings } from './native';

/** 后端 loader 是自由字符串；界面只认五个已知值，其余一律按原版处理。 */
/**
 * `InstanceRecord.loader` → 界面加载器类型。
 *
 * 后端存的是 `neoforge@21.1.265` 这种「名字@版本」形式（见
 * `loaders.rs` / `forge_install.rs` 的 `set_instance_loader`），而界面只需要
 * 名字部分。之前这里直接拿整串去 match 四个裸名，于是**所有装过加载器的实例
 * 都被显示成「原版」**——数据库里明明是 `neoforge@21.1.265`，界面上写着原版，
 * 用户完全无法判断加载器是否装成功。
 */
function normalizeLoader(loader: string): GameInstance['loader'] {
  // 只取 `@` 之前的部分；没有 `@` 时就是裸名（如 'vanilla'）。
  const name = loader.split('@', 1)[0]?.trim().toLowerCase() ?? '';
  switch (name) {
    case 'fabric':
    case 'quilt':
    case 'forge':
    case 'neoforge':
      return name;
    default:
      return 'vanilla';
  }
}

/**
 * 从 `neoforge@21.1.265` 里取出加载器版本。
 *
 * 后端一直记着这个值，但界面从来没读过，所以实例卡片与详情页都只说「NeoForge」，
 * 说不出装的是哪个构建。加载器版本决定它能装哪些模组，属于用户会想知道的信息。
 */
function loaderVersionOf(loader: string): string | undefined {
  const index = loader.indexOf('@');
  if (index < 0) return undefined;
  const version = loader.slice(index + 1).trim();
  return version || undefined;
}

/**
 * `InstanceRecord.state` → 界面状态。
 *
 * `failed` 映射为 `error`：界面用它决定显示「需要修复」。后端没有「更新中」这一
 * 状态，所以 `updating` 永远不会出现，但保留映射以维持类型完整。
 */
function normalizeState(state: InstanceRecord['state']): GameInstance['status'] {
  switch (state) {
    case 'ready':
      return 'ready';
    case 'installing':
      return 'installing';
    case 'failed':
      return 'error';
    default:
      return 'ready';
  }
}

/**
 * 转换一个实例。
 *
 * `modCount` 需要额外一次 `list_mods` 才能拿到。列表页要为每个实例都查一次
 * 吗？值得——模组数是用户在实例卡片上最先看的信息之一，而且 `list_mods` 只读
 * 本地目录，不触网。调用方可以并发取完再传进来。
 */
export function toGameInstance(record: InstanceRecord, modCount = 0): GameInstance {
  return {
    id: record.id,
    name: record.name,
    version: record.gameVersion,
    loader: normalizeLoader(record.loader),
    /*
     * 加载器版本。之前这里写死 `undefined`（注释说「后端不记录」），但后端其实
     * 一直记着（`neoforge@21.1.265`）。补上之后，实例页能显示具体构建，
     * 用户才能判断「我装的这个加载器能不能跑我要的模组」。
     */
    loaderVersion: loaderVersionOf(record.loader),
    javaVersion: '',
    // 后端现在记录上次成功启动的时间；解析失败时留 undefined，让卡片上那一行
    // 自动不渲染，而不是显示 Invalid Date。
    lastPlayed: record.lastLaunchedAt ? new Date(record.lastLaunchedAt) : undefined,
    playTime: 0,
    modCount,
    status: normalizeState(record.state),
    gameDir: record.gameDir,
  };
}

/**
 * 转换一个账户。
 *
 * `uuid` 用后端 id 填充：界面只把它当唯一键用，真正参与启动的是后端自己存的
 * profile id。头像后端不提供，留 undefined 让界面落到占位头像。
 */
export function toAccount(profile: AccountProfile): Account {
  return {
    id: profile.id,
    username: profile.displayName,
    uuid: profile.id,
    type: profile.kind,
    avatar: undefined,
    // 微软账户必然拥有游戏；离线与皮肤站无法在本地判断，标为 false 让界面
    // 显示「未验证」——这是诚实的默认值，而不是乐观地标成 true。
    hasMinecraft: profile.kind === 'microsoft',
  };
}

/**
 * 从后端设置里取出界面需要的主题相关值。
 *
 * 只取主题三件套：`themeMode`、`themePreset`、`customAccent`。其余设置由
 * 各自的页面直接读写 `LauncherSettings`，不经过这里——多绕一层只会让字段
 * 名字在不同文件里出现两份。
 */
export function themeFromSettings(settings: LauncherSettings): {
  mode: LauncherSettings['themeMode'];
  preset: LauncherSettings['themePreset'];
  accent: string;
} {
  return {
    mode: settings.themeMode,
    preset: settings.themePreset,
    accent: settings.customAccent,
  };
}
