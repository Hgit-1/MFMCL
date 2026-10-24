import type { Plugin, GameInstance, Mod } from '../types';

/**
 * 插件管理器
 * 负责注册、启用/禁用插件，执行生命周期钩子
 */
export class PluginManager {
  private plugins: Map<string, Plugin> = new Map();
  private enabledPlugins: Set<string> = new Set();

  /**
   * 注册插件
   */
  register(plugin: Plugin) {
    if (this.plugins.has(plugin.id)) {
      console.warn(`插件 ${plugin.id} 已存在，将被覆盖`);
    }
    this.plugins.set(plugin.id, plugin);
    if (plugin.enabled) {
      this.enabledPlugins.add(plugin.id);
    }
    console.log(`✅ 插件已注册: ${plugin.name} v${plugin.version}`);
  }

  /**
   * 启用插件
   */
  enable(pluginId: string) {
    const plugin = this.plugins.get(pluginId);
    if (!plugin) {
      console.error(`插件 ${pluginId} 不存在`);
      return;
    }
    this.enabledPlugins.add(pluginId);
    plugin.enabled = true;
    console.log(`✅ 插件已启用: ${plugin.name}`);
  }

  /**
   * 禁用插件
   */
  disable(pluginId: string) {
    const plugin = this.plugins.get(pluginId);
    if (!plugin) {
      console.error(`插件 ${pluginId} 不存在`);
      return;
    }
    this.enabledPlugins.delete(pluginId);
    plugin.enabled = false;
    console.log(`⏸️  插件已禁用: ${plugin.name}`);
  }

  /**
   * 获取所有插件
   */
  getAll(): Plugin[] {
    return Array.from(this.plugins.values());
  }

  /**
   * 获取已启用的插件
   */
  getEnabled(): Plugin[] {
    return this.getAll().filter(p => this.enabledPlugins.has(p.id));
  }

  /**
   * 触发钩子: 实例创建
   */
  async triggerInstanceCreate(instance: GameInstance) {
    for (const plugin of this.getEnabled()) {
      if (plugin.hooks.onInstanceCreate) {
        try {
          await plugin.hooks.onInstanceCreate(instance);
        } catch (error) {
          console.error(`插件 ${plugin.name} 执行 onInstanceCreate 失败:`, error);
        }
      }
    }
  }

  /**
   * 触发钩子: 实例启动
   */
  async triggerInstanceLaunch(instance: GameInstance) {
    for (const plugin of this.getEnabled()) {
      if (plugin.hooks.onInstanceLaunch) {
        try {
          await plugin.hooks.onInstanceLaunch(instance);
        } catch (error) {
          console.error(`插件 ${plugin.name} 执行 onInstanceLaunch 失败:`, error);
        }
      }
    }
  }

  /**
   * 触发钩子: 模组安装
   */
  async triggerModInstall(mod: Mod, instance: GameInstance) {
    for (const plugin of this.getEnabled()) {
      if (plugin.hooks.onModInstall) {
        try {
          await plugin.hooks.onModInstall(mod, instance);
        } catch (error) {
          console.error(`插件 ${plugin.name} 执行 onModInstall 失败:`, error);
        }
      }
    }
  }

  /**
   * 获取所有自定义视图
   */
  getCustomViews() {
    const views: any[] = [];
    for (const plugin of this.getEnabled()) {
      if (plugin.hooks.customViews) {
        views.push(...plugin.hooks.customViews);
      }
    }
    return views;
  }

  /**
   * 合并所有主题覆盖
   */
  getMergedThemeOverrides() {
    const merged: any = {};
    for (const plugin of this.getEnabled()) {
      if (plugin.hooks.themeOverrides) {
        Object.assign(merged, plugin.hooks.themeOverrides);
      }
    }
    return merged;
  }

  /**
   * 获取所有背景提供者
   */
  getBackgroundProviders() {
    const providers: any[] = [];
    for (const plugin of this.getEnabled()) {
      if (plugin.hooks.backgroundProviders) {
        providers.push(...plugin.hooks.backgroundProviders);
      }
    }
    return providers;
  }
}

// 单例实例
export const pluginManager = new PluginManager();
