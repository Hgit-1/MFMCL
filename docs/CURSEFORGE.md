# CurseForge API: key format, auth, and endpoints

Verified by direct requests against the live API on 2026-10-28. This file records what the public CurseForge API actually accepts so future work does not repeat the guesswork.

## Which token is the API key

| Token shape | Accepted by `api.curseforge.com` | Verdict |
| --- | --- | --- |
| `$2a$10$` + 53 chars (60 total) | **200** | This is the `x-api-key` the API wants |
| `cfc_pat_` + 64 chars (72 total) | 403 in every scheme tried | Not the CurseForge API key |

The `$2a$10$` prefix is a bcrypt hash marker, so the key is opaque: do not try to validate it by decoding. The only reliable validation is one cheap authenticated request.

The `cfc_pat_` prefix reads as "personal access token". It was rejected as `x-api-key`, as `Authorization: Bearer`, as `Authorization: <token>`, and against `core.`/`studios.`/`authors.`/`www.` hosts. It belongs to some other console or service; where it was issued determines what it is for.

## Authentication

```sh
curl -H 'x-api-key: <key>' -H 'Accept: application/json' \
  https://api.curseforge.com/v1/games/432
```

- The header name is exactly `x-api-key`. A valid key sent as `Authorization: Bearer <key>` returns **403**, so the scheme matters, not just the value.
- Do not put the key in the query string (`?apiKey=`) — also 403.
- Failures return **403 with an empty body**. Missing key and wrong key are indistinguishable, so the UI can only report "the key is missing or invalid".
- Rate limiting was not documented and not observed: 30 consecutive requests all returned 200. Handle 429 defensively anyway.

## CORS (relevant to browser preview)

The API supports browser calls: it echoes the request `Origin` and its preflight allows the `x-api-key` header.

```
access-control-allow-origin: http://127.0.0.1:5173
access-control-allow-headers: x-api-key
```

Because the origin is echoed, any page can call the API with a key it holds. That is an argument for keeping the key in the desktop backend rather than in page-level storage once the feature is real.

## Endpoints used by a launcher

Base: `https://api.curseforge.com`. Minecraft `gameId` is **432**.

| Purpose | Request |
| --- | --- |
| Search | `GET /v1/mods/search` |
| Mod detail | `GET /v1/mods/{modId}` |
| File list | `GET /v1/mods/{modId}/files` |
| Single file | `GET /v1/mods/{modId}/files/{fileId}` |
| Download URL | `GET /v1/mods/{modId}/files/{fileId}/download-url` |
| Bulk file lookup | `POST /v1/mods/files` with `{"fileIds":[...]}` |
| Categories | `GET /v1/categories?gameId=432&classId=6` |
| Minecraft versions | `GET /v1/minecraft/version` |

Search parameters that matter:

- `classId`: 6 = mods, 4471 = modpacks, 12 = resource packs, 17 = worlds. All four return data for `gameId=432`.
- `gameVersion` plus `modLoaderType` must be used together. Observed counts for "jei" on 1.20.1: 1 (Forge) = 201, 4 (Fabric) = 55, 5 (Quilt) = 8, 6 (NeoForge) = 25.
- `searchFilter` for the text query. The docs do not list a `searchField` parameter; matching spans names and descriptions, and broad queries also return near-miss projects, so rank or filter results again in the launcher rather than trusting the query.
- `pagination.totalCount` is capped at **10,000** even for queries that match far more (a bare mods query reports exactly 10000), so treat it as "at least this many" and do not derive an exact result total from it.
- `index` and `pageSize` for paging. `pageSize` is capped at 50 and `index + pageSize` must stay within 10,000; `pageSize=200` and `index=9999&pageSize=50` both return **400**.
- `sortField` accepts the documented enum (1 featured, 2 popularity, 3 last updated, 4 name, 5 author, 6 total downloads, 11 released date).

## Obtaining hashes and dependencies

Search results carry `latestFiles`, which is not enough for installation because the launcher needs the exact file for the instance's game version and loader. Two workable paths:

1. `GET /v1/mods/{modId}/files?gameVersion=…&modLoaderType=…` — paged list already filtered for the target combination (`pagination.totalCount` gives the count).
2. `POST /v1/mods/files` with a body of file IDs — returns `hashes` (algo 1 = SHA-1, algo 2 = MD5), `dependencies` (`relationType` 2 = required, 3 = optional/incompatible variants), `fileLength`, `downloadUrl`, and `fileName` in one call. A 200-file body worked in testing.

There is no `/v1/mods/{modId}/dependencies` endpoint; dependency data comes from the file objects.

Hashes are trustworthy for verification: a test download of `indestructibletools-1.0.0.jar` matched both the declared SHA-1 (algo 1) and MD5 (algo 2), and its byte length matched `fileLength`. Note that `fileLength` can be a large integer — an All the Mods 10 pack reported about 200 MB — so plan disk and progress display accordingly.

Downloads come from `edge.forgecdn.net`. `modLoaderType` is accepted without `gameVersion` and returns 200, but the documented contract couples them; always send both so results stay meaningful.

## Distribution-restricted projects

Projects may set `allowModDistribution: false`. Their files then report `downloadUrl: null`, and `/download-url` returns **404** even with a valid key (confirmed with Entity Culling, mod 448233). A launcher must detect the null URL, skip the automatic download, and tell the user to fetch it from the website — not retry or fabricate a mirror.

## 已实现（2026-10-29）

后端在 `crates/launcher-core/src/curseforge.rs`（协议与解析），命令在
`src-tauri/src/curseforge.rs`（凭据存取 + 六个 Tauri 命令）：

| 命令 | 作用 |
| --- | --- |
| `curseforge_key_status` | 只返回「有没有 key」，绝不返回 key 本身 |
| `curseforge_save_key` | 先用真实请求验证，通过后才写入系统凭据库 |
| `curseforge_clear_key` | 从凭据库删除 |
| `curseforge_search` | 按类型 / 游戏版本 / 加载器搜索 |
| `curseforge_files` | 某项目的文件列表（含 SHA-1 与下载地址） |
| `curseforge_install` | 下载到临时文件、SHA-1 校验、`hard_link` 原子发布 |

设计要点：

- **key 存操作系统凭据库**（与账户凭据同一个 service，不同账户键），数据库只保存
  「是否设置过」。前端永远拿不到 key；所有请求由 Rust 附加 `x-api-key`。
- **保存前先验证**：无效的 key 不会写进凭据库，避免之后每次搜索都遇到莫名的 403。
- **安装绝不覆盖**：先用 `hard_link` 发布（目标存在时由文件系统拒绝），而不是
  「先检查再写」——后者存在竞态。
- **只信任官方分发域名**（`edge.forgecdn.net` / `mediafilez.forgecdn.net`），服务端
  返回其他主机时丢弃 URL。
- **受限文件不显示安装按钮**：`downloadUrl` 为 null 时界面显示「需手动下载」，
  不会给一个点了必然失败的按钮。
- 8 项单元测试覆盖解析边界：空名条目丢弃、非 HTTPS 图标丢弃、只取 SHA-1、
  攻击者域名拒绝、受限文件不可安装、枚举值固定、非法 key 前置拒绝、分页越界拒绝。

真实网络验证（用一个有效 key 走完整后端路径）：搜索返回 20 条；JEI 的文件列表
返回 50 个版本，SHA-1 与下载地址齐全。

## 默认（内置）key

2026-10-30 起支持构建期注入一个默认 key，让内容商店的 CurseForge 来源开箱可用，
不需要用户先去 console 申请。

机制：

- `src-tauri/src/curseforge.rs` 的 `DEFAULT_KEY` 用 `option_env!("MFMCL_CURSEFORGE_API_KEY")`
  在**编译期**读取。仓库源码里没有任何 key。
- `src-tauri/build.rs` 会读仓库根目录的 `.env`（该文件在 `.gitignore` 里），
  通过 `cargo:rustc-env` 把值交给 rustc——**不需要先 `source scripts/tauri-env.sh`**。
  真实环境变量优先于 `.env`，所以 CI 仍可用 `env:` 覆盖。
- 把变量显式设成空串可以关掉内置 key（即使 `.env` 里有值）：
  `MFMCL_CURSEFORGE_API_KEY= npm run tauri -- build`。这是从一台本地存了 key 的
  机器上产出对外构建的正确做法。
- `src-tauri/build.rs` 还声明了 `rerun-if-env-changed` / `rerun-if-changed`，改 `.env`
  后重新编译会生效，不会残留旧的常量。
- 运行时优先级：**用户自己保存的 key > 内置 key**。用户填过就完全以他的为准；
  在内置 key 状态下点「使用自己的 Key」可以展开表单覆盖，清除后回到内置 key。
- 设置 → 下载 里有常驻的 Key 入口。这是必要的兜底：内置 key 配额耗尽或被轮换时，
  模组分类不会显示配置入口（那里的卡片只在「没有 key」时出现），用户必须能在
  设置里换成自己的 key，否则只能等新版本。

安全边界（重要）：

- 二进制里的默认 key 对所有拿到客户端的人都可见（`strings` 能直接读出来），
  并且共用同一份配额。因此**只适用于个人 / 内部构建**。公开发布的安装包应当
  不设置这个变量，让用户填自己的 key。
- 不要在源码里硬编码 key：那会永久留在 git 历史中，且无法通过重新构建轮换。

验证记录（2026-10-30，真实请求）：`/v1/games/432`、`/v1/mods/search`、
`/v1/mods/{id}/files` 均返回 200 且带真实数据。

## 浏览模式（空关键词）

CurseForge 的搜索接口在**不传** `searchFilter` 时返回全库列表，但默认按名称字母序。
要得到「该版本 + 加载器下最受欢迎的模组」必须显式带 `sortField=6&sortOrder=desc`
（6 = total downloads）。实测要点：

- 服务端排序是**近似**降序：200 行内约 1.9% 的相邻对逆序，单页最大反向偏差约 29%。
  需要对拿到的这一页再按 `downloadCount` 排一次，不要把它当权威排行榜。
- `pagination.totalCount` 硬顶在 **10000**，`index >= 10000` 直接返回 400。
- `downloadCount` 是项目全生命周期总量，不限于该版本，所以语义是「有该版本文件的
  项目里，按历史总下载量排序」。

## 历史状态（重构后、接入前）

以下描述的是 2026-10-28 时的状态，保留下来是为了解释为什么界面上曾出现过一个
永远失败的 CurseForge 面板。

> **2026-10-30 更新**：CurseForge 已接入（`crates/launcher-core/src/curseforge.rs`
> 与 `src-tauri/src/curseforge.rs`），下面的「没有 CurseForge 代码」已不成立，
> 仅作为历史记录保留。

## 当时的仓库状态（历史记录）

The CurseForge client was removed in the store refactor: `utils/store/curseforge.ts`, `http.ts`, `modrinth.ts`, `vanilla.ts`, `pagination.ts`, and `sort.ts` are deleted, and the store now calls the Rust backend (`search_mods`), which implements **Modrinth only**. There is no CurseForge code in `crates/` or `src-tauri/`.

Consequences:

- The "Add a CurseForge key" panel and its error row exist only in the pre-refactor revision. Both the current source and the current `dist/` bundle contain the strings solely inside the i18n message tables, with no component that renders them. A screenshot showing that panel therefore came from an older build (for example a previously deployed preview), and its Retry button cannot succeed with any key.
- The `store.key*`, `store.fillKey`, `store.curseforge*` entries in `utils/i18n/messages/*` are now unused; `keyPanel` styles at `components/StoreWorkspace.module.css:425` are dead.
- Pasting a key into the current build has no effect, because nothing reads it.

Adding CurseForge later means choosing where the key lives: a backend command that signs requests server-side (keeps the key out of the web view, but requires the Rust client), or a direct browser call (works in preview, exposes the key to page context). Either way the key is user-supplied — never compile one in, since every client copy would share the same quota and identity.
