# Minecraft 方块贴图素材

2026-10-25 抓取；仅用于本地 UI 预览（内容商店的版本类型图标），不随成品分发。

## 来源

- 仓库：`InventivetalentDev/minecraft-assets`，分支/标签 `1.20.4`
- 路径：`assets/minecraft/textures/block/*.png`
- 原始 URL：`https://raw.githubusercontent.com/InventivetalentDev/minecraft-assets/1.20.4/assets/minecraft/textures/block/<文件>`
- 抓取方式：因 raw.githubusercontent.com 在本机直连被重置，经 `ghproxy.net` 镜像转发取得

## 文件清单

| 文件 | 尺寸 | 格式 | 用途 |
| --- | --- | --- | --- |
| `grass_block_side.png` | 16×16 | 8 位调色板 | 正式版（release）版本图标；「原版游戏」分类图标（侧栏菜单 + 工作区当前类型） |
| `command_block.png` | 16×16 | 8 位调色板 | 快照版（snapshot）图标 |
| `cobblestone.png` | 16×16 | 4 位调色板 | 远古版（old_alpha / old_beta）图标 |
| `gold_block.png` | 16×16 | 4 位调色板 | 彩蛋版（special / 愚人节版本）图标 |
| `command_block_front.png` | 16×64 | 8 位调色板 | 命令方块正面动画条带（原始文件，保留） |
| `command_block_side.png` | 16×64 | 8 位调色板 | 命令方块侧面动画条带（原始文件，保留） |
| `command_block_back.png` | 16×64 | 8 位调色板 | 命令方块背面动画条带（原始文件，保留） |
| `command_block_side_frame.png` | 16×16 | 8 位调色板 | 侧面首帧裁切（备用） |
| `command_block_back_frame.png` | 16×16 | 8 位调色板 | 背面首帧裁切（备用） |
| `grass_block_top.png` | 16×16 | 8 位灰度 | 备用 |
| `stone.png` | 16×16 | 8 位灰度 | 备用 |

## 动画纹理的裁切

命令方块的三张 16×64 贴图是**动画纹理**：64 像素高由 4 帧 16×16 纵向堆叠而成。
界面里需要静态图标，因此用脚本裁出首帧（顶部 16 行）另存为 `command_block.png`
（正面）、`command_block_side_frame.png`、`command_block_back_frame.png`。

裁切脚本（未纳入仓库，可复现）：环境里没有 Pillow 与 ImageMagick，所以用 Python
标准库 `zlib` + `struct` 手写了解码/编码：

1. 解析 PNG 分块，读取 `IHDR`（宽高、位深、颜色类型、是否隔行）、`PLTE`、`tRNS`、`IDAT`；
2. `zlib.decompress` 解压 IDAT，按每行的滤波类型（None/Sub/Up/Average/Paeth）逐字节还原；
3. 只保留前 16 行扫描线，其余丢弃；
4. 重新压缩并逐块写回 `IHDR` / `PLTE` / `tRNS` / `IDAT` / `IEND`，CRC32 用 `zlib.crc32` 计算。

校验结果：裁出的 `command_block.png` 顶部 16 行与原文件对应行逐字节一致，且原文件
4 帧互不相同（确认它确实是动画条带，不是静态图纵向重复）。

## 渲染约定

- 保持 16×16 原始像素，用 `image-rendering: pixelated` 放大到 32–40 px，避免插值糊边；
- **不加边框、圆角遮罩、背景色块或外阴影**；
- 加载失败时隐藏图片并回退到文字/图标，不显示破图图标。

## 授权

这些是 Mojang 的 Minecraft 游戏素材（Minecraft EULA 下的资产），所有权归 Mojang
Studios / Microsoft。此处仅作为本地 UI 原型预览使用，**本项目不因此获得任何再分发
许可**。公开分发前必须核实 Minecraft EULA 与素材使用条款，或替换为已获授权的
资源。公开可下载不等于可以再分发。
