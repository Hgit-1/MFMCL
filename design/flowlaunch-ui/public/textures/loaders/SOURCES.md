# 加载器标志素材

2026-10-29 抓取；仅用于本地 UI 预览（游戏库的实例图标），不随成品分发。

这些是各加载器项目**自己的官方标志**，不是 Minecraft 的素材。用途仅限于在
界面上指示「这个实例装的是哪个加载器」，不表示任何隶属、背书或合作关系。

## 来源

| 文件 | 项目 | 原始地址 | 处理 |
| --- | --- | --- | --- |
| `fabric.png` | Fabric | `https://fabricmc.net/assets/logo.png` | 原始 52×56，按整数倍 2× 放大到 104×112（保持硬边，避免小尺寸模糊） |
| `quilt.svg` | Quilt | `https://quiltmc.org/assets/img/logo.svg` | 把 `<style>` 里的 `.cls-N` 颜色内联成 `fill` 属性，去掉无用的 `id`/`data-name`；图形未改动 |
| `forge.png` | Forge | `MinecraftForge/MinecraftForge` 仓库 `fmlearlydisplay/src/main/resources/forge_anvil.png` | 原文件是 128×4096 的 32 帧动画条带，提取第 0 帧（顶部 128×128） |
| `neoforge.png` | NeoForge | `neoforged/NeoForge` 仓库 `src/main/resources/assets/neoforge/textures/gui/sprites/icon/neo_logo.png` | 原始 15×15 像素画，按整数倍 8× 放大到 120×120（保持硬边） |

原版（vanilla）不使用加载器标志，沿用 `public/textures/blocks/grass_block_side.png`，
其来源与授权见同目录上层的 `blocks/SOURCES.md`。

## 为什么不用每个标志的官方 bleed/margin

四个标志的画布留白比例不同：Fabric 的织纹几乎填满画布，Forge 的铁砧与 NeoForge
的狐狸头自带边距。若都按同一外框边长渲染，填满的那个会显得大一圈。因此
`utils/store/loaders.ts` 里给 Fabric 记了 `scale: 0.9`，其余靠自身的留白自然收敛；
实测四个图标的内容占比分别是 1.00 / 0.93 / 0.93（Fabric / Forge / NeoForge）。

## 渲染约定

- 外框尺寸由调用方给定（实例卡片是 48px），图片在框内 `contain` 居中，不裁切；
- 像素画（原版方块、Fabric、NeoForge）用 `image-rendering: pixelated` 硬边放大；
  插画与矢量（Forge、Quilt）用默认平滑缩放；
- 图标本身不加边框、圆角或阴影；底板属于调用方容器（实例卡片的 `.instanceIcon`）。

## 授权

这些标志的版权归各自项目所有，各项目对其使用有自己的商标/品牌条款：

- Fabric：<https://fabricmc.net/>
- Quilt：<https://quiltmc.org/>
- Forge：<https://minecraftforge.net/>
- NeoForge：<https://neoforged.net/>

此处仅作为本地 UI 原型预览使用，**本项目不因此获得任何再分发许可**。公开分发前
必须逐项核实各项目的品牌使用条款，或改用不涉及第三方商标的自绘制图标。公开可
下载不等于可以再分发。
