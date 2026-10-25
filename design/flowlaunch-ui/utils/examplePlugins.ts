import type { Plugin } from '../types';

/**
 * 示例插件 1: 启动日志记录器
 * 记录每次游戏启动的时间和实例信息
 */
export const LaunchLoggerPlugin: Plugin = {
  id: 'launch-logger',
  name: '启动日志记录器',
  version: '1.0.0',
  author: 'MFMC Team',
  description: '记录游戏启动历史，生成统计报告',
  enabled: true,
  hooks: {
    onInstanceLaunch: async (instance) => {
      const timestamp = new Date().toISOString();
      console.log(`[启动日志] ${timestamp} - 启动实例: ${instance.name} (${instance.version})`);
      
      // 可以在这里保存到 localStorage 或发送到服务器
      const logs = JSON.parse(localStorage.getItem('launch-logs') || '[]');
      logs.push({
        timestamp,
        instanceId: instance.id,
        instanceName: instance.name,
        version: instance.version,
      });
      localStorage.setItem('launch-logs', JSON.stringify(logs));
    },
  },
};

/**
 * 示例插件 2: 自动备份
 * 在游戏启动前自动创建存档备份
 */
export const AutoBackupPlugin: Plugin = {
  id: 'auto-backup',
  name: '自动备份',
  version: '1.0.0',
  author: 'MFMC Team',
  description: '启动前自动备份存档，保护游戏数据',
  enabled: true,
  hooks: {
    onInstanceLaunch: async (instance) => {
      console.log(`[自动备份] 正在为 ${instance.name} 创建备份...`);
      
      // 模拟备份过程
      await new Promise(resolve => setTimeout(resolve, 1000));
      
      console.log(`[自动备份] ✅ 备份完成: backup_${instance.id}_${Date.now()}.zip`);
    },
  },
};

/**
 * 示例插件 3: 模组冲突检测
 * 安装模组时自动检测潜在冲突
 */
export const ModConflictDetectorPlugin: Plugin = {
  id: 'mod-conflict-detector',
  name: '模组冲突检测',
  version: '1.0.0',
  author: 'MFMC Team',
  description: '智能检测模组冲突，提前预警',
  enabled: true,
  hooks: {
    onModInstall: async (mod, instance) => {
      console.log(`[冲突检测] 检查 ${mod.name} 与现有 ${instance.modCount} 个模组的兼容性...`);
      
      // 模拟冲突检测
      await new Promise(resolve => setTimeout(resolve, 500));
      
      // 这里可以实现真实的冲突检测逻辑
      const hasConflict = Math.random() > 0.9; // 10% 概率模拟冲突
      
      if (hasConflict) {
        console.warn(`[冲突检测] ⚠️  发现潜在冲突: ${mod.name} 可能与某些模组不兼容`);
      } else {
        console.log(`[冲突检测] ✅ 未发现冲突`);
      }
    },
  },
};

/**
 * 示例插件 4: 自定义主题
 * 提供粉色主题配色
 */
export const PinkThemePlugin: Plugin = {
  id: 'pink-theme',
  name: '粉色主题',
  version: '1.0.0',
  author: 'MFMC Team',
  description: '温柔的粉色配色方案',
  enabled: false, // 默认禁用
  hooks: {
    themeOverrides: {
      colors: {
        primary: '#EC4899',
        primaryHover: '#DB2777',
        primaryActive: '#BE185D',
        secondary: '#F472B6',
        success: '#10B981',
        warning: '#F59E0B',
        danger: '#EF4444',
        bgPrimary: '#FFF5F7',
        bgSecondary: '#FFE4E9',
        bgElevated: '#FFFFFF',
        border: '#FBCFE8',
        textPrimary: '#1F2937',
        textSecondary: '#6B7280',
        textDisabled: '#9CA3AF',
      },
    },
  },
};

/**
 * 示例插件 5: 性能监控
 * 监控启动器性能，显示资源使用情况
 */
export const PerformanceMonitorPlugin: Plugin = {
  id: 'performance-monitor',
  name: '性能监控',
  version: '1.0.0',
  author: 'MFMC Team',
  description: '实时监控启动器性能和资源占用',
  enabled: false,
  hooks: {
    onInstanceLaunch: async (instance) => {
      console.log(`[性能监控] 启动实例: ${instance.name}`);
      console.log(`[性能监控] 内存使用: ${(performance as any).memory?.usedJSHeapSize / 1024 / 1024 || 'N/A'} MB`);
    },
    customViews: [
      {
        id: 'performance-view',
        label: '性能监控',
        icon: 'activity',
        position: 'sidebar',
        component: () => null, // 实际使用时需要提供真实组件
      },
    ],
  },
};

/**
 * 所有示例插件
 */
export const examplePlugins = [
  LaunchLoggerPlugin,
  AutoBackupPlugin,
  ModConflictDetectorPlugin,
  PinkThemePlugin,
  PerformanceMonitorPlugin,
];
