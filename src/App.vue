<script setup lang="ts">
import { h, computed, onMounted, ref } from "vue";
import { RouterLink, RouterView, useRoute } from "vue-router";
import { NIcon } from "naive-ui";
import {
  DesktopOutline,
  GlobeOutline,
  HomeOutline,
  ServerOutline,
  SettingsOutline,
} from "@vicons/ionicons5";
import { systemInfo, type SystemInfo } from "./api";

const route = useRoute();

const menuOptions = [
  { label: "仪表盘", key: "/dashboard", icon: HomeOutline },
  { label: "站点", key: "/sites", icon: GlobeOutline },
  { label: "数据库", key: "/databases", icon: ServerOutline },
  { label: "Hosts", key: "/hosts", icon: DesktopOutline },
  { label: "设置", key: "/settings", icon: SettingsOutline },
];

const activeKey = computed(() => route.path);

// 全局底部状态栏
const sysInfo = ref<SystemInfo | null>(null);

onMounted(async () => {
  try {
    sysInfo.value = await systemInfo();
  } catch (e) {
    console.error("加载系统信息失败", e);
  }
});

function fmtBytes(n: number): string {
  const units = ["B", "KB", "MB", "GB", "TB"];
  let v = n;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i += 1;
  }
  return `${v >= 100 || i === 0 ? Math.round(v) : v.toFixed(1)} ${units[i]}`;
}
</script>

<template>
  <n-config-provider>
    <n-message-provider>
      <n-dialog-provider>
        <n-layout has-sider style="height: 100%">
          <n-layout-sider bordered :width="180">
            <div class="logo">RunPHP</div>
            <n-menu
              :value="activeKey"
              :options="
                menuOptions.map((m) => ({
                  label: () =>
                    h(RouterLink, { to: m.key }, { default: () => m.label }),
                  key: m.key,
                  icon: () => h(NIcon, null, { default: () => h(m.icon) }),
                }))
              "
            />
          </n-layout-sider>
          <n-layout>
            <n-layout-header bordered style="padding: 12px 20px; font-size: 16px; font-weight: 600;">
              {{ String(route.meta.title ?? "") }}
            </n-layout-header>
            <n-layout-content content-style="padding: 20px 20px 44px;">
              <RouterView />
            </n-layout-content>
          </n-layout>
        </n-layout>
      </n-dialog-provider>
    </n-message-provider>

    <div v-if="sysInfo" class="sys-bar">
      <n-text depth="3">
        RunPHP 0.1.0 · {{ sysInfo.cpu_arch }} · 内存 {{
          fmtBytes(sysInfo.memory_total)
        }} · 硬盘 {{ fmtBytes(sysInfo.disk_total) }}（可用 {{
          fmtBytes(sysInfo.disk_free)
        }}） · {{ sysInfo.os }}
      </n-text>
    </div>
  </n-config-provider>
</template>

<style scoped>
.logo {
  font-size: 18px;
  font-weight: 700;
  padding: 16px 20px;
}
.sys-bar {
  position: fixed;
  left: 0;
  right: 0;
  bottom: 0;
  z-index: 100;
  display: flex;
  justify-content: center;
  padding: 8px 16px;
  font-size: 12px;
  background: rgba(250, 250, 250, 0.95);
  border-top: 1px solid rgba(128, 128, 128, 0.25);
  backdrop-filter: blur(4px);
}
</style>
