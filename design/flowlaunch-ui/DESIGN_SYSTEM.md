# FlowLaunch 设计系统

## 品牌标识

**名称：** FlowLaunch（流启）  
**理念：** 流畅、自然、随需而动  
**目标：** 让Minecraft启动器像呼吸一样自然

---

## 色彩系统

### 浅色主题
```
主色调（Primary）
  - 500: #2563EB （品牌蓝，主要操作）
  - 600: #1D4ED8 （悬停）
  - 700: #1E40AF （按下）

次要色（Secondary）
  - 500: #7C3AED （紫色，强调）
  - 600: #6D28D9

成功/安全
  - 500: #10B981
  - 600: #059669

警告
  - 500: #F59E0B
  - 600: #D97706

危险
  - 500: #EF4444
  - 600: #DC2626

中性色
  - 50: #F9FAFB （背景）
  - 100: #F3F4F6 （次级背景）
  - 200: #E5E7EB （分割线）
  - 400: #9CA3AF （禁用文本）
  - 600: #4B5563 （次要文本）
  - 900: #111827 （主要文本）
```

### 深色主题
```
主色调
  - 500: #3B82F6
  - 600: #2563EB

背景色
  - 950: #0A0E14 （主背景）
  - 900: #111827 （次级背景）
  - 800: #1F2937 （卡片背景）
  - 700: #374151 （悬停背景）

中性色
  - 50: #F9FAFB （主要文本）
  - 400: #9CA3AF （次要文本）
  - 600: #4B5563 （禁用文本）
```

---

## 排版系统

### 字体栈
```css
--font-sans: -apple-system, BlinkMacSystemFont, "Segoe UI", "Noto Sans", 
             "Microsoft YaHei", sans-serif;
--font-mono: "SF Mono", Monaco, "Cascadia Code", "Consolas", monospace;
```

### 字号与行高
```
特大标题: 32px / 40px (font-bold)
大标题: 24px / 32px (font-semibold)
标题: 20px / 28px (font-semibold)
小标题: 16px / 24px (font-medium)
正文: 14px / 20px (font-normal)
小字: 12px / 16px (font-normal)
辅助: 11px / 14px (font-normal)
```

---

## 间距系统

基础单位：4px

```
xs: 4px
sm: 8px
md: 12px
base: 16px
lg: 24px
xl: 32px
2xl: 48px
3xl: 64px
```

---

## 组件状态

### 按钮状态
- **默认：** 完整颜色，清晰边界
- **悬停：** 加深10%，轻微抬升阴影
- **按下：** 加深20%，移除阴影
- **焦点：** 2px外描边，主色调透明度40%
- **禁用：** 50%透明度，无交互反馈
- **加载：** 禁用+旋转图标

### 输入框状态
- **默认：** 1px边框，中性色
- **焦点：** 2px边框，主色调，外发光
- **错误：** 2px边框，危险色，错误提示
- **禁用：** 背景变浅，文字变灰

### 卡片状态
- **默认：** 轻微阴影，圆角8px
- **悬停：** 阴影加深，抬升2px
- **选中：** 2px边框主色调，强阴影

---

## 圆角规范

```
小控件（按钮、标签）: 6px
输入框、卡片: 8px
模态框、面板: 12px
大容器: 16px
```

---

## 阴影层级

```
sm: 0 1px 2px rgba(0,0,0,0.05)
base: 0 1px 3px rgba(0,0,0,0.1), 0 1px 2px rgba(0,0,0,0.06)
md: 0 4px 6px rgba(0,0,0,0.07), 0 2px 4px rgba(0,0,0,0.06)
lg: 0 10px 15px rgba(0,0,0,0.1), 0 4px 6px rgba(0,0,0,0.05)
xl: 0 20px 25px rgba(0,0,0,0.1), 0 10px 10px rgba(0,0,0,0.04)
```

深色主题下透明度减半。

---

## 动画时长

```
fast: 150ms （按钮反馈）
base: 200ms （状态切换）
slow: 300ms （页面转场）
```

缓动函数：`cubic-bezier(0.4, 0, 0.2, 1)`

---

## 可访问性

### 对比度
- 正文文字：至少 4.5:1
- 大文字（18px+）：至少 3:1
- 交互元素边界：至少 3:1

### 焦点可见性
- 键盘焦点环：2px，主色调，偏移2px
- 焦点顺序：从左到右，从上到下
- 跳过链接：隐藏但可键盘访问

### 动效控制
- 尊重 `prefers-reduced-motion`
- 减少动效时：禁用非必要过渡，保留状态反馈

---

## 响应式断点

```
sm: 1024px （最小支持）
md: 1280px （推荐）
lg: 1440px （舒适）
xl: 1920px （宽屏）
```

---

## 插件扩展点

### 主题覆盖
```typescript
{
  colors?: Partial<ColorPalette>,
  components?: {
    Button?: ComponentStyle,
    Card?: ComponentStyle,
    ...
  }
}
```

### 自定义组件注入
```typescript
{
  customViews: [{
    id: 'my-view',
    position: 'sidebar' | 'main' | 'modal',
    component: MyComponent
  }]
}
```

### 背景提供者
```typescript
{
  backgroundProvider: {
    type: 'image' | 'video' | 'shader',
    source: string | Function
  }
}
```
