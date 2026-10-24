#!/bin/bash

# FlowLaunch UI - 快速启动脚本

echo "======================================"
echo "  FlowLaunch UI - Minecraft 启动器"
echo "======================================"
echo ""

# 检查是否在正确的目录
if [ ! -f "package.json" ]; then
    echo "❌ 错误: 请在项目根目录运行此脚本"
    echo "   cd /home/mdfz31/MFMCL/design/flowlaunch-ui"
    exit 1
fi

# 检查依赖是否安装
if [ ! -d "node_modules" ]; then
    echo "📦 首次运行，正在安装依赖..."
    npm install
    echo ""
fi

echo "✅ 准备就绪！"
echo ""
echo "📖 使用说明:"
echo "   - 浏览器将自动打开 http://localhost:5173"
echo "   - 修改代码后会自动热重载"
echo "   - 按 Ctrl+C 停止开发服务器"
echo ""
echo "🎨 快速定制:"
echo "   - 修改品牌色: styles/globals.css"
echo "   - 修改组件: components/*.tsx"
echo "   - 添加页面: src/App.tsx"
echo ""
echo "📚 文档:"
echo "   - README.md - 使用文档"
echo "   - DESIGN_SYSTEM.md - 设计规范"
echo "   - PROJECT_SUMMARY.md - 项目总结"
echo ""
echo "🚀 正在启动开发服务器..."
echo "======================================"
echo ""

npm run dev
