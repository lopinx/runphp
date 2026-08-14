<script setup lang="ts">
import { ref, onMounted } from "vue";
import { greet, getDataDir, isDesktop } from "../api";

const greeting = ref("");
const dataDir = ref("");
const loading = ref(false);

async function load() {
  loading.value = true;
  try {
    greeting.value = await greet("开发者");
    dataDir.value = await getDataDir();
  } catch (e) {
    greeting.value = `调用失败：${e}`;
  } finally {
    loading.value = false;
  }
}

onMounted(load);
</script>

<template>
  <n-space vertical size="large">
    <n-card title="运行状态">
      <n-descriptions :column="1" label-placement="left">
        <n-descriptions-item label="运行模式">
          {{ isDesktop ? "桌面端（Tauri）" : "Web 面板" }}
        </n-descriptions-item>
        <n-descriptions-item label="数据目录">
          <n-text code>{{ dataDir || "加载中…" }}</n-text>
        </n-descriptions-item>
      </n-descriptions>
    </n-card>

    <n-card title="后端连通性测试">
      <n-space align="center">
        <n-button :loading="loading" @click="load">调用 greet 命令</n-button>
        <n-text v-if="greeting">{{ greeting }}</n-text>
      </n-space>
    </n-card>

    <n-alert type="info" title="M1 阶段">
      脚手架验证中。运行时下载、站点管理等功能将在后续里程碑实现。
    </n-alert>
  </n-space>
</template>
