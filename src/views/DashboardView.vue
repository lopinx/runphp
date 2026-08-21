<script setup lang="ts">
import { onMounted, ref, computed } from "vue";
import { useAppStore } from "../stores/app";
import { isDesktop, logsRead, getDataDir } from "../api";
import { useMessage } from "naive-ui";

const store = useAppStore();
const message = useMessage();
const dataDir = ref("");
const starting = ref(false);
const logText = ref("");

async function load() {
  store.loading = true;
  try {
    await Promise.all([store.refreshRuntimes(), store.refreshSites()]);
    await store.refreshStatus();
    dataDir.value = await getDataDir();
    await refreshLogs();
  } catch (e) {
    console.error("加载仪表盘失败", e);
  } finally {
    store.loading = false;
  }
}

async function refreshLogs() {
  try {
    logText.value = await logsRead(100);
  } catch (e) {
    logText.value = `加载日志失败：${e}`;
  }
}

async function toggleRuntime() {
  starting.value = true;
  try {
    if (store.running) {
      await store.stopRuntime();
    } else {
      await store.startRuntime();
    }
  } catch (e) {
    message.error(`操作失败：${e}`);
  } finally {
    starting.value = false;
  }
}

const statusType = computed(() =>
  store.running ? "success" : "default"
);
const statusText = computed(() => (store.running ? "运行中" : "已停止"));

onMounted(() => {
  void load();
});
</script>

<template>
  <n-space vertical size="large" style="padding-bottom: 44px">
    <n-grid :cols="3" :x-gap="16">
      <n-gi>
        <n-card>
          <n-statistic label="运行状态">
            <template #default>
              <n-tag :type="statusType" size="large" round>
                {{ statusText }}
              </n-tag>
            </template>
          </n-statistic>
        </n-card>
      </n-gi>
      <n-gi>
        <n-card>
          <n-statistic label="已安装站点" :value="store.sites.length" />
        </n-card>
      </n-gi>
      <n-gi>
        <n-card>
          <n-statistic label="运行时版本">
            <template #default>
              {{ store.defaultRuntime?.version ?? "未安装" }}
            </template>
          </n-statistic>
        </n-card>
      </n-gi>
    </n-grid>

    <n-card title="快捷操作">
      <n-space>
        <n-button
          :type="store.running ? 'error' : 'primary'"
          :loading="starting"
          :disabled="!store.hasRuntime"
          @click="toggleRuntime"
        >
          {{ store.running ? "停止服务" : "启动服务" }}
        </n-button>
        <n-button :disabled="!store.running" @click="store.reloadRuntime()">
          热重载配置
        </n-button>
        <n-button @click="load">刷新状态</n-button>
      </n-space>
    </n-card>

    <n-card title="运行时日志（最近 100 行）">
      <template #header-extra>
        <n-button size="small" @click="refreshLogs">刷新</n-button>
      </template>
      <n-scrollbar style="max-height: 300px">
        <pre class="log-view">{{ logText || "日志为空或尚未启动。" }}</pre>
      </n-scrollbar>
    </n-card>

    <n-card title="运行时详情">
      <n-descriptions :column="1" label-placement="left" bordered>
        <n-descriptions-item label="运行模式">
          {{ isDesktop ? "桌面端（Tauri）" : "Web 面板" }}
        </n-descriptions-item>
        <n-descriptions-item label="数据目录">
          <n-text code>{{ dataDir || "加载中…" }}</n-text>
        </n-descriptions-item>
        <n-descriptions-item label="已安装运行时">
          <n-space>
            <n-tag
              v-for="rt in store.runtimes"
              :key="rt.version"
              :type="rt.is_default ? 'success' : 'default'"
            >
              v{{ rt.version }}
            </n-tag>
            <n-text v-if="!store.hasRuntime" depth="3">未安装</n-text>
          </n-space>
        </n-descriptions-item>
      </n-descriptions>
    </n-card>

    <n-alert
      v-if="!store.loading && !store.hasRuntime"
      type="info"
      :bordered="false"
    >
      尚未安装运行时，请到「设置」页面下载或导入 FrankenPHP。
    </n-alert>

  </n-space>
</template>

<style scoped>
.log-view {
  margin: 0;
  font-family: Consolas, "Courier New", monospace;
  font-size: 12px;
  white-space: pre-wrap;
  word-break: break-all;
}
</style>
