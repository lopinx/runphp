<script setup lang="ts">
import { h, onMounted, onUnmounted, ref } from "vue";
import { useRouter } from "vue-router";
import { useAppStore } from "../stores/app";
import { NButton, NTag, useMessage } from "naive-ui";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import {
  isDesktop,
  runtimeSetDefault,
  runtimeDetectLocal,
  runtimeImportLocal,
  type DetectedBinary,
  type DetectedService,
  type LocalDetection,
  type RuntimeInfo,
} from "../api";

const store = useAppStore();
const message = useMessage();
const router = useRouter();

const installVersion = ref("1.12.7");
const installing = ref(false);
const downloadProgress = ref(0);
const detecting = ref(false);
const detection = ref<LocalDetection | null>(null);
const importing = ref<string | null>(null);
let unlisten: UnlistenFn | null = null;

onMounted(async () => {
  await store.refreshRuntimes();
  // 先检测本地环境：已有 FrankenPHP 或数据库服务时列出并提供可用链接
  void detectLocal();
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

async function install() {
  if (!installVersion.value.trim()) {
    message.warning("请填写版本号");
    return;
  }
  installing.value = true;
  downloadProgress.value = 0;
  const version = installVersion.value.trim();
  try {
    await store.installRuntime(version);
    message.success(`FrankenPHP v${version} 安装成功`);
  } catch (e) {
    message.error(`安装失败：${e}`);
  } finally {
    installing.value = false;
  }
}

async function detectLocal() {
  detecting.value = true;
  try {
    detection.value = await runtimeDetectLocal();
  } catch (e) {
    message.error(`本地检测失败：${e}`);
  } finally {
    detecting.value = false;
  }
}

function isImported(bin: DetectedBinary): boolean {
  return store.runtimes.some((r) => r.path === bin.path);
}

async function importLocal(bin: DetectedBinary) {
  importing.value = bin.path;
  try {
    const result = await runtimeImportLocal(bin.path);
    message.success(`已导入 FrankenPHP v${result.version}`);
    await store.refreshRuntimes();
  } catch (e) {
    message.error(`导入失败：${e}`);
  } finally {
    importing.value = null;
  }
}

function gotoAddConnection(svc: DetectedService) {
  router.push({
    path: "/databases",
    query: {
      add_db: svc.driver,
      host: svc.host,
      port: String(svc.port),
      name: svc.name,
    },
  });
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

        <n-divider />

        <n-space align="center" justify="space-between">
          <n-text strong>本地环境检测</n-text>
          <n-button size="small" :loading="detecting" @click="detectLocal">
            重新检测
          </n-button>
        </n-space>

        <n-space v-if="detection" vertical size="small">
          <div v-if="detection.frankenphp.length > 0">
            <n-text depth="3" style="font-size: 13px">
              检测到本地 FrankenPHP：
            </n-text>
            <div
              v-for="bin in detection.frankenphp"
              :key="bin.path"
              style="
                display: flex;
                align-items: center;
                justify-content: space-between;
                margin-top: 8px;
              "
            >
              <n-space vertical size="small">
                <n-text>{{ bin.name }}</n-text>
                <n-text depth="3" style="font-size: 12px">
                  {{ bin.path }}
                </n-text>
              </n-space>
              <n-button
                v-if="!isImported(bin)"
                size="small"
                type="primary"
                :loading="importing === bin.path"
                @click="importLocal(bin)"
              >
                导入使用
              </n-button>
              <n-tag v-else size="small" type="success">已导入</n-tag>
            </div>
          </div>

          <n-data-table
            v-if="detection.services.length > 0"
            :columns="[
              { title: '数据库服务', key: 'name' },
              {
                title: '地址',
                key: 'addr',
                render: (row: DetectedService) => `${row.host}:${row.port}`,
              },
              {
                title: '状态',
                key: 'running',
                render: (row: DetectedService) =>
                  h(
                    NTag,
                    { size: 'small', type: row.running ? 'success' : 'default' },
                    () => (row.running ? '运行中' : '未运行'),
                  ),
              },
              {
                title: '操作',
                key: 'actions',
                render: (row: DetectedService) =>
                  row.running
                    ? h(
                        NButton,
                        { size: 'small', onClick: () => gotoAddConnection(row) },
                        () => '添加连接',
                      )
                    : null,
              },
            ]"
            :data="detection.services"
            :bordered="false"
            :pagination="false"
          />

          <n-text
            v-if="
              detection.frankenphp.length === 0 &&
              !detection.services.some((s) => s.running)
            "
            depth="3"
          >
            未在本地检测到 FrankenPHP 或运行中的数据库服务。
          </n-text>
        </n-space>
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
