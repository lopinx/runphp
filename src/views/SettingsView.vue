<script setup lang="ts">
import { h, onMounted, onUnmounted, ref } from "vue";
import { useAppStore } from "../stores/app";
import { NButton, useMessage } from "naive-ui";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import {
  isDesktop,
  runtimeSetDefault,
  runtimeVersions,
  type RuntimeInfo,
} from "../api";

const store = useAppStore();
const message = useMessage();

const installVersion = ref<string | null>(null);
const availableVersions = ref<string[]>([]);
const loadingVersions = ref(false);
const installing = ref(false);
const downloadProgress = ref(0);
let unlisten: UnlistenFn | null = null;

onMounted(async () => {
  await store.refreshRuntimes();
  void loadVersions();
  if (isDesktop) {
    try {
      unlisten = await listen<[number, number]>(
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

onUnmounted(() => {
  unlisten?.();
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

async function loadVersions() {
  loadingVersions.value = true;
  try {
    availableVersions.value = await runtimeVersions();
    installVersion.value = availableVersions.value[0] ?? null;
  } catch (e) {
    message.error(`获取版本列表失败：${e}`);
  } finally {
    loadingVersions.value = false;
  }
}

async function install() {
  if (!installVersion.value) {
    message.warning("请选择要安装的版本");
    return;
  }
  installing.value = true;
  downloadProgress.value = 0;
  const version = installVersion.value;
  try {
    await store.installRuntime(version);
    message.success(`FrankenPHP v${version} 安装成功`);
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
              render: (row: RuntimeInfo) => (row.is_default ? '✅' : ''),
            },
            {
              title: '操作',
              key: 'actions',
              render: (row: RuntimeInfo) =>
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
          <n-select
            v-model:value="installVersion"
            :options="availableVersions.map((v) => ({ label: `v${v}`, value: v }))"
            :loading="loadingVersions"
            placeholder="选择版本"
            filterable
            tag
            style="width: 200px"
            :disabled="installing"
          />
          <n-button
            type="primary"
            :loading="installing"
            :disabled="!installVersion"
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
  </n-space>
</template>
