# 交接说明

原仓库在 `/home/mdfz31/MFMCL`，远端是 <https://github.com/Hgit-1/MFMCL>。

这份文档同时存在于仓库根目录（随 git 走）和 H 盘副本里（`/run/media/mdfz31/H/MFMCL/HANDOFF.md`），
内容相同。区别只在第二节：H 盘那份额外带了可直接安装的产物。

状态对应远端 `main` 的 `94de28d`（PR #19 合并后）。

---

## 一、这份副本里有什么

下面是 H 盘那份额外的内容。如果是从 git 克隆的，`dist-artifacts/` 与他人提供的
`node_modules/`、`target/` 都不会有——按第三节重新生成即可。

| 内容 | H 盘副本 | 说明 |
| --- | --- | --- |
| 全部源码 | ✅ | `crates/`、`src-tauri/`、`design/` |
| Git 历史 | ✅ | `.git`，含全部分支 |
| 文档 | ✅ | `docs/`、`README.md`、`PLUGIN.md`、`UI_WORKFLOW.md` |
| 可直接安装的产物 | ✅ | `dist-artifacts/`（deb / rpm / AppImage / 裸二进制） |
| `.env`（含 CurseForge API Key） | ⚠️ | **见第五节安全提示** |
| `node_modules/` | ❌ | 需重新 `npm ci` |
| `target/`（Rust 编译缓存） | ❌ | 31.7 GB，需重新编译 |
| `PCL/`（参考仓库） | ❌ | 188 MB，与本项目无关的第三方参考代码 |

H 盘那份总计 453 MB。原目录 32 GB，差别主要在 `target/`。

---

## 二、开箱即用的部分

如果你手上是 **H 盘那份副本**，`dist-artifacts/` 里的四个文件可以直接用，不需要编译：

| 文件 | 用途 |
| --- | --- |
| `MFMCL_0.1.0_amd64.deb` | Debian / Ubuntu 安装包 |
| `MFMCL-0.1.0-1.x86_64.rpm` | Fedora / RHEL 安装包 |
| `MFMCL-x86_64.AppImage` | 免安装，`chmod +x` 后直接运行 |
| `mfmcl-desktop` | 裸二进制 |
| `SHA256SUMS.txt` | 校验和 |

安装方式：

```bash
sudo apt install ./MFMCL_0.1.0_amd64.deb        # Debian/Ubuntu
sudo dnf install ./MFMCL-0.1.0-1.x86_64.rpm     # Fedora/RHEL
chmod +x MFMCL-x86_64.AppImage && ./MFMCL-x86_64.AppImage
```

校验完整性：

```bash
cd dist-artifacts && sha256sum -c SHA256SUMS.txt
```

### 这些产物的两个已知限制

**没有内置 Microsoft 应用 ID。** 因此「用 Microsoft 登录」会提示未配置。要能正版登录，必须按
[`docs/MICROSOFT_LOGIN.md`](docs/MICROSOFT_LOGIN.md) 注册自己的应用 ID 并重新编译。
这是有意设计，不是遗漏——公共客户端 ID 与发布者绑定，不能借用。

**没有内置 CurseForge API Key。** 内容商店的 CurseForge 来源会提示需要填写 Key。
用户可以在设置里填自己的（存进系统凭据库），或者构建时注入。

---

## 三、继续开发需要做什么

### 1. 装依赖

```bash
cd /run/media/mdfz31/H/MFMCL

# 前端
npm ci --prefix design/flowlaunch-ui

# Rust 工具链（若本机没有）
# https://rustup.rs
```

系统依赖（Linux 编译桌面端必需）：

```bash
sudo apt install build-essential curl wget file libxdo-dev libssl-dev \
  libayatana-appindicator3-dev librsvg2-dev libdbus-1-dev \
  libgtk-3-dev libwebkit2gtk-4.1-dev
```

### 2. 浏览器预览（不编译 Rust，最快看到界面）

```bash
npm run dev
# 打开 http://127.0.0.1:5173/
```

预览模式没有后端：任何需要 Rust 的功能（安装、下载、登录）都不会真的执行，
但界面与交互可以完整调整。详见 `UI_WORKFLOW.md`。

### 3. 编译桌面端

```bash
npm run tauri -- build --ci --no-sign
```

产物在 `target/release/bundle/`。

首次编译约 5–15 分钟（取决于机器），之后增量编译很快。

### 4. 跑检查

```bash
cargo fmt --all -- --check     # 格式
cargo test --workspace         # Rust 测试（17 组）
npm run typecheck && npm run build --prefix design/flowlaunch-ui   # 前端
```

CI 会跑这些，加上 CodeQL 与三平台（Linux / macOS / Windows）编译。

---

## 四、H 盘上的 Git 注意事项

`core.fileMode` 在 H 盘副本里已设为 `false`。

原因：那块盘是 exFAT，不保存 Unix 权限位，所有文件挂上来都是 `755`，Git 会
因此把整个仓库报成「已修改」。关掉 `fileMode` 后状态恢复正常。

**把仓库拷到别的外置盘时**，那边可能需要重新设一次：

```bash
git config core.fileMode false
```

设完之后 `git status` 是干净的，`git log` 显示真实的提交历史。
从内置盘上的仓库克隆不会有这个问题。

---

## 五、安全提示（重要）

`.env` 被一起拷到了这块盘上，里面有 **CurseForge API Key**（一个会消耗配额的凭据）。

需要知道的三件事：

1. 这个 key 在 `dist-artifacts/` 的产物里**也存在**（编译期 `option_env!` 注入，
   `strings` 就能读出来）。所以拿到这些安装包的人都能用它。
2. 这是**个人/内部使用**的取舍，`docs/BUILDING.md` 里有说明：公开发布应当留空，
   让用户填自己的 key。
3. 如果你要把这份副本或产物交给别人，先决定是否接受这一点。要更保守的话：
   - 编译时不设 `MFMCL_CURSEFORGE_API_KEY`
   - 或者删掉 `.env` 后重新编译

`MFMCL_MICROSOFT_CLIENT_ID` 在 `.env` 里是注释状态，未设置。

---

## 六、项目现状速览

截至这份副本，`main` 上已完成的工作（都经过 CI 全绿后合并）：

| 领域 | 状态 |
| --- | --- |
| 启动器插件系统 | 后端真正执行贡献；只保留两个有实现的插件 |
| 实例管理 | 默认装到 `~/.minecraft/shared`，可逐实例指定目录 |
| 加载器安装 | Fabric / Quilt / Forge / NeoForge 全部可用 |
| Forge 版本列表 | 从 maven 取全量（1.20.1 有 137 个），标出推荐版 |
| Fabric API | 装完加载器后可就地安装 |
| 内容商店 | Modrinth + CurseForge，模组与整合包都能搜索、翻页、装 |
| 崩溃分析 | 本地小模型（LFM3），流式输出，可即用即启引擎 |
| 下载任务 | 自动清理已结束记录，数字可直接输入 |
| 首次引导 | 语言 / 外观 / 内存 / Java / 模型五步 |

已知未完成（都不影响使用）：

- OptiFine 只做说明，不提供一键安装（官方无直链，且与 Fabric 类加载器互斥）
- Microsoft 应用 ID 没有运行时填写入口（见 issue #16）
- 自更新只到「检查 + 下载安装包」，不替换正在运行的二进制（无签名密钥）

---

## 七、常用命令速查

```bash
# 位置
cd /run/media/mdfz31/H/MFMCL

# 界面预览
npm run dev

# 完整桌面构建
npm run tauri -- build --ci --no-sign

# 只编译 Rust 后端（debug，快）
cargo build -p mfmcl-desktop

# 测试
cargo test --workspace
npm run typecheck --prefix design/flowlaunch-ui

# 检查当前状态
git log --oneline -5
git status
```

---

## 八、如果要从远端重新拉一份

这个副本的 `.git` 完整，随时可以：

```bash
git fetch origin
git reset --hard origin/main
```

但注意本机 git 的 TLS 栈长期握手失败（gnutls），历史提交是通过 GitHub REST API
推送的。如果 `git fetch` 报 `gnutls_handshake() failed`，那是本机网络/代理问题，
不是仓库问题——恢复代理后可正常使用。
