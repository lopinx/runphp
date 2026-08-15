<script setup lang="ts">
import { h, computed } from "vue";
import { RouterLink, RouterView, useRoute } from "vue-router";
import { NIcon } from "naive-ui";
import {
  DesktopOutline,
  GlobeOutline,
  HomeOutline,
  ServerOutline,
  SettingsOutline,
} from "@vicons/ionicons5";

const route = useRoute();

const menuOptions = [
  { label: "仪表盘", key: "/dashboard", icon: HomeOutline },
  { label: "站点", key: "/sites", icon: GlobeOutline },
  { label: "数据库", key: "/databases", icon: ServerOutline },
  { label: "Hosts", key: "/hosts", icon: DesktopOutline },
  { label: "设置", key: "/settings", icon: SettingsOutline },
];

const activeKey = computed(() => route.path);
</script>

<template>
  <n-config-provider>
    <n-message-provider>
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
          <n-layout-content content-style="padding: 20px;">
            <RouterView />
          </n-layout-content>
        </n-layout>
      </n-layout>
    </n-message-provider>
  </n-config-provider>
</template>

<style scoped>
.logo {
  font-size: 18px;
  font-weight: 700;
  padding: 16px 20px;
}
</style>
