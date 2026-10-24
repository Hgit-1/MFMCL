# FlowLaunch UI - 使用文档

## 项目概述

FlowLaunch 是一个现代化的 Minecraft 启动器 UI 设计原型，采用**智能情境切换**模式，基于 React + TypeScript + Vite 构建。

## 核心特性

### 1. 智能情境系统
界面会根据用户当前状态自动调整：
- **首次使用** → 欢迎引导页
- **无游戏** → 空状态提示 + 快速创建入口
- **有游戏** → 快速启动中心
- **浏览商店** → 内容发现模式
- **下载中** → 任务监控模式

### 2. 插件化架构
完整的插件系统支持：
- 主题覆盖
- 自定义视图注入
- 背景提供者
- 生命周期钩子

### 3. 可持续设计
- 模块化组件系统
- CSS Modules 隔离样式
- TypeScript 类型安全
- 语义化颜色变量

---

## 快速开始

### 安装依赖
```bash
cd /home/mdfz31/MFMCL/design/flowlaunch-ui
npm install
```

### 开发模式
```bash
npm run dev
```
访问 http://localhost:5173

### 构建生产版本
```bash
npm run build
```

---

## 设计系统

### 色彩系统
所有颜色使用 CSS 变量定义，支持浅色/深色主题切换：
```css
var(--color-primary-500)    /* 主色调 */
var(--color-bg-primary)      /* 主背景 */
var(--color-text-primary)    /* 主要文字 */
```

### 组件库
- **Button**: 4种变体（primary/secondary/danger/ghost），3种尺寸
- **Card**: 可悬停、可选中、多种内边距
- **Input**: 带标签、错误提示、图标支持
- **Modal**: 键盘导航、焦点管理、Escape关闭

### 间距系统
基于 4px 网格：
```
xs: 4px, sm: 8px, md: 12px, base: 16px, 
lg: 24px, xl: 32px, 2xl: 48px, 3xl: 64px
```

---

## 插件开发

### 创建插件
```typescript
const myPlugin: Plugin = {
  id: 'my-plugin',
  name: '我的插件',
  version: '1.0.0',
  author: '作者名',
  description: '插件描述',
  enabled: true,
  hooks: {
    // 实例创建时触发
    onInstanceCreate: async (instance) => {
      console.log('新实例:', instance.name);
    },
    
    // 启动前触发
    onInstanceLaunch: async (instance) => {
      console.log('启动:', instance.name);
    },
    
    // 自定义视图
    customViews: [{
      id: 'my-view',
      label: '我的视图',
      icon: 'star',
      position: 'sidebar',
      component: MyCustomComponent
    }],
    
    // 主题覆盖
    themeOverrides: {
      colors: {
        primary: '#FF6B6B'
      }
    },
    
    // 背景提供者
    backgroundProviders: [{
      id: 'my-bg',
      name: '自定义背景',
      type: 'image',
      source: 'https://example.com/bg.jpg'
    }]
  }
};
```

### 注册插件
```typescript
// 在 App.tsx 中
import { PluginManager } from './utils/PluginManager';

const pluginManager = new PluginManager();
pluginManager.register(myPlugin);
```

---

## 主题定制

### 修改颜色
编辑 `styles/globals.css`：
```css
:root {
  --color-primary-500: #YOUR_COLOR;
  --color-secondary-500: #YOUR_COLOR;
}
```

### 创建新主题
```typescript
const customTheme: Theme = {
  colors: {
    primary: '#2563EB',
    // ... 其他颜色
  },
  spacing: { /* ... */ },
  radius: { /* ... */ },
  shadows: { /* ... */ }
};
```

---

## 组件使用示例

### Button
```tsx
<Button 
  variant="primary" 
  size="lg" 
  icon={<PlayIcon />}
  loading={isLoading}
  onClick={handleClick}
>
  启动游戏
</Button>
```

### Card
```tsx
<Card 
  hoverable 
  selected={isSelected}
  padding="lg"
  onClick={handleSelect}
>
  <h3>卡片标题</h3>
  <p>卡片内容</p>
</Card>
```

### Input
```tsx
<Input
  label="游戏名称"
  placeholder="输入名称"
  error={errors.name}
  hint="不超过32个字符"
  icon={<SearchIcon />}
  value={name}
  onChange={(e) => setName(e.target.value)}
/>
```

### Modal
```tsx
<Modal
  isOpen={isOpen}
  onClose={() => setIsOpen(false)}
  title="确认操作"
  size="md"
  footer={
    <>
      <Button variant="ghost" onClick={() => setIsOpen(false)}>取消</Button>
      <Button variant="primary" onClick={handleConfirm}>确认</Button>
    </>
  }
>
  <p>确定要执行此操作吗？</p>
</Modal>
```

---

## 响应式适配

### 支持的窗口尺寸
- **最小**: 1024×720
- **推荐**: 1280×900
- **舒适**: 1440×900+

### 断点
```css
@media (max-width: 1024px) {
  /* 小窗口布局 */
}
```

---

## 可访问性

### 键盘导航
- `Tab`: 焦点移动
- `Enter/Space`: 激活按钮
- `Escape`: 关闭模态框

### 焦点管理
所有交互元素都有清晰的焦点环：
```css
:focus-visible {
  outline: 2px solid var(--color-primary-500);
  outline-offset: 2px;
}
```

### 减少动效
自动检测系统偏好：
```css
@media (prefers-reduced-motion: reduce) {
  * {
    animation-duration: 0.01ms !important;
    transition-duration: 0.01ms !important;
  }
}
```

---

## 文件结构

```
flowlaunch-ui/
├── components/          # 通用组件
│   ├── Button.tsx
│   ├── Button.module.css
│   ├── Card.tsx
│   ├── Input.tsx
│   └── Modal.tsx
├── hooks/              # 自定义 Hooks
│   └── useTheme.ts
├── src/                # 应用主体
│   ├── App.tsx
│   ├── App.module.css
│   └── main.tsx
├── styles/             # 全局样式
│   └── globals.css
├── types.ts            # TypeScript 类型定义
└── DESIGN_SYSTEM.md    # 设计系统规范
```

---

## 下一步扩展

### 待实现的功能
1. **实例详情页** - 模组管理、设置、日志查看
2. **模组商店** - Modrinth/CurseForge 集成
3. **整合包安装** - .mrpack 支持
4. **账户系统** - 微软登录流程
5. **下载管理** - 断点续传、多线程
6. **诊断系统** - 崩溃分析、日志查看
7. **备份恢复** - 存档管理

### 扩展点
- `hooks/` - 添加更多自定义 Hooks
- `utils/` - 工具函数（API 调用、文件处理等）
- `pages/` - 独立页面组件
- `plugins/` - 内置插件示例

---

## 修改建议

### 改变品牌色
1. 打开 `styles/globals.css`
2. 修改 `--color-primary-500` 和 `--color-secondary-500`
3. 保存后自动热重载

### 添加新页面
1. 在 `src/` 下创建新组件
2. 在 `types.ts` 中添加新的 `AppContext`
3. 在 `App.tsx` 的导航和主内容区添加对应逻辑

### 自定义组件
1. 复制现有组件作为模板
2. 保持 CSS Modules 模式
3. 导出类型接口以便复用

---

## 技术栈

- **React 18** - UI 框架
- **TypeScript** - 类型安全
- **Vite** - 构建工具
- **CSS Modules** - 样式隔离
- **Lucide React** - 图标库（可选）

---

## 浏览器支持

- Chrome/Edge 90+
- Firefox 88+
- Safari 14+

---

## 许可

此设计原型作为 MFMCL 项目的一部分提供。
