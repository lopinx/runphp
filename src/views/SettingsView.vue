<script setup lang="ts">
import { h, onMounted, ref } from "vue";
import { useAppStore } from "../stores/app";
import { NButton, useMessage } from "naive-ui";
import { listen } from "@tauri-apps/api/event";
import { isDesktop, runtimeSetDefault } from "../api";

const store = useAppStore();
const message = useMessage();

const installVersion = ref("1.12.7");
const installing = ref(false);
const downloadProgress = ref(0);

onMounted(async () => {
  await store.refreshRuntimes();
  // 监听下载进度事件（仅桌面端）
  if (isDesktop) {
    try {
      await listen<[number, number]>(
        "runtime-download-progress",
        (event) => {
          const [downloaded, total] = event.payload;
          downloadProgress.value =
            total > 0 ? Math.round((downloaded / total) * 100) : 0;
        },
      );
    } catch (e) {
      console.warn("事件监听不可用", e);
    }
  }
});

async function setDefault(version: string) {
  try {
    await runtimeSetDefault(version);
    message.success(`已将默认运行时设置为 v${version}`);
    await store.refreshRuntimes();
  } catch (e) {
    message.error(`设置失败：${e}`);
  }
}

async function install() {
  if (!installVersion.value.trim()) {
    message.warning("请填写版本号");
    return;
  }
  installing.value = true;
  downloadProgress.value = 0;
  try {
    await store.installRuntime(installVersion.value.trim());
    message.success(`FrankenPHP v${installVersion.value} 安装成功`);
  } catch (e) {
    message.error(`安装失败：${e}`);
  } finally {
    installing.value = false;
  }
}
</script>

<template>
  <n-space vertical size="large">
    <n-card title="运行时管理">
      <n-space vertical>
        <n-data-table
          :columns="[
            { title: '版本', key: 'version' },
            { title: '路径', key: 'path', ellipsis: { tooltip: true } },
            {
              title: '默认',
              key: 'is_default',
              render: (row: any) => (row.is_default ? '✅' : ''),
            },
            {
              title: '操作',
              key: 'actions',
              render: (row: any) =>
                row.is_default
                  ? null
                  : h(
                      NButton,
                      { size: 'small', onClick: () => setDefault(row.version) },
                      () => '设为默认',
                    ),
            },
          ]"
          :data="store.runtimes"
          :bordered="false"
          :pagination="false"
        />

        <n-divider />

        <n-space align="center">
          <n-input
            v-model:value="installVersion"
            placeholder="版本号，如 1.12.7"
            style="width: 200px"
            :disabled="installing"
          />
          <n-button
            type="primary"
            :loading="installing"
            @click="install"
          >
            下载安装
          </n-button>
        </n-space>

        <n-progress
          v-if="installing"
          type="line"
          :percentage="downloadProgress"
          indicator-placement="inside"
        />
        <n-text depth="3" style="font-size: 13px">
          从 GitHub Releases 下载对应平台的 FrankenPHP 二进制。Windows 为 zip 包（含 PHP 扩展），Linux 为静态二进制。
        </n-text>
      </n-space>
    </n-card>

    <n-card title="关于">
      <n-descriptions :column="1" label-placement="left" bordered>
        <n-descriptions-item label="软件名称">RunPHP</n-descriptions-item>
        <n-descriptions-item label="版本">0.1.0</n-descriptions-item>
        <n-descriptions-item label="运行模式">
          {{ isDesktop ? "桌面端（Tauri 2）" : "Web 面板" }}
        </n-descriptions-item>
        <n-descriptions-item label="技术栈">
          Rust + Tauri 2 + Vue 3 + FrankenPHP
        </n-descriptions-item>
      </n-descriptions>
    </n-card>
  </n-space>
</template>
