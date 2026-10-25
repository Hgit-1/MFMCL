/// <reference types="vite/client" />

// CSS Modules 的类型声明由 vite/client 提供（declare module '*.module.css'），
// 但这个项目之前没有引入它，导致 `tsc --noEmit` 对每个 *.module.css 导入报
// TS2307。补上之后类型检查才反映真实问题，而不是被一片“找不到模块”掩盖。
