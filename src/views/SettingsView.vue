<script setup lang="ts">
import { h, onMounted, ref } from "vue";
import { useAppStore } from "../stores/app";
import { NButton, NSelect, NTag, useMessage } from "naive-ui";
import {
  runtimeSetDefault,
  runtimeVersions,
  runtimeDetectLocal,
  runtimeImportLocal,
  runtimeInstall,
  type RuntimeInfo,
  type LocalDetection,
  type DetectedBinary,
  type DetectedService,
} from "../api";

const store = useAppStore();
const message = useMessage();

const availableVersions = ref<string[]>([]);
const loadingVersions = ref(false);
const installing = ref(false);

// ---- 本地环境检测 ----
const detection = ref<LocalDetection | null>(null);
const detecting = ref(false);
const importingPath = ref<string | null>(null);
// 每个检测到的二进制对应一个版本选择
const binaryVersionMap = ref<Record<string, string | null>>({});

onMounted(async () => {
  await store.refreshRuntimes();
  void loadVersions();
  void detectLocal();
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
  } catch (e) {
    message.error(`获取版本列表失败：${e}`);
  } finally {
    loadingVersions.value = false;
  }
}

async function detectLocal() {
  detecting.value = true;
  try {
    detection.value = await runtimeDetectLocal();
    // 为每个检测到的二进制初始化版本选择
    for (const b of detection.value.frankenphp) {
      if (!(b.path in binaryVersionMap.value)) {
        binaryVersionMap.value[b.path] = availableVersions.value[0] ?? null;
      }
    }
  } catch (e) {
    message.error(`本地检测失败：${e}`);
  } finally {
    detecting.value = false;
  }
}

async function importBinary(binary: DetectedBinary) {
  importingPath.value = binary.path;
  try {
    const result = await runtimeImportLocal(binary.path);
    message.success(`已导入 FrankenPHP（版本 ${result.version}）`);
    await store.refreshRuntimes();
    await detectLocal();
  } catch (e) {
    message.error(`导入失败：${e}`);
  } finally {
    importingPath.value = null;
  }
}

async function installForBinary(binary: DetectedBinary) {
  const version = binaryVersionMap.value[binary.path];
  if (!version) {
    message.warning("请先选择版本");
    return;
  }
  installing.value = true;
  try {
    await runtimeInstall(version);
    message.success(`FrankenPHP v${version} 安装成功`);
    await store.refreshRuntimes();
    await detectLocal();
  } catch (e) {
    message.error(`安装失败：${e}`);
  } finally {
    installing.value = false;
  }
}

/** 检查某个检测到的二进制是否已导入（按导入时记录的来源路径精确匹配） */
function isImported(binaryPath: string): boolean {
  const target = binaryPath.toLowerCase();
  return store.runtimes.some(
    (r) => r.imported_from?.toLowerCase() === target,
  );
}
</script>

<template>
  <n-space vertical size="large">
    <!-- 本地环境检测 -->
    <n-card title="本地环境检测">
      <n-space vertical size="large">
        <n-space align="center">
          <n-button :loading="detecting" @click="detectLocal">
            重新检测
          </n-button>
          <n-text depth="3" style="font-size: 13px">
            扫描 PATH、常见安装目录和数据库服务端口
          </n-text>
        </n-space>

        <!-- FrankenPHP 二进制检测列表 -->
        <div>
          <n-text depth="2" style="font-size: 14px; font-weight: 600">
            FrankenPHP 二进制
          </n-text>
          <n-data-table
            v-if="detection && detection.frankenphp.length > 0"
            :columns="[
              { title: '文件名', key: 'name', width: 180, ellipsis: { tooltip: true } },
              { title: '路径', key: 'path', ellipsis: { tooltip: true } },
              {
                title: '状态',
                key: 'status',
                width: 80,
                render: (row: DetectedBinary) =>
                  isImported(row.path)
                    ? h(NTag, { type: 'success', size: 'small' }, () => '已导入')
                    : h(NTag, { type: 'info', size: 'small' }, () => '可导入'),
              },
              {
                title: '安装版本',
                key: 'install',
                width: 280,
                render: (row: DetectedBinary) =>
                  h('div', { style: 'display:flex; gap:6px; align-items:center' }, [
                    h(NSelect, {
                      size: 'small',
                      value: binaryVersionMap[row.path] ?? null,
                      options: availableVersions.map((v) => ({
                        label: `v${v}`,
                        value: v,
                      })),
                      placeholder: '选择版本',
                      filterable: true,
                      loading: loadingVersions,
                      disabled: installing,
                      style: 'width: 140px',
                      'onUpdate:value': (val: string | null) => {
                        binaryVersionMap[row.path] = val;
                      },
                    }),
                    h(
                      NButton,
                      {
                        size: 'small',
                        type: 'primary',
                        disabled: installing || isImported(row.path),
                        loading: installing,
                        onClick: () => installForBinary(row),
                      },
                      () => '安装',
                    ),
                    h(
                      NButton,
                      {
                        size: 'small',
                        type: 'default',
                        disabled: importingPath === row.path || isImported(row.path),
                        loading: importingPath === row.path,
                        onClick: () => importBinary(row),
                      },
                      () => '导入',
                    ),
                  ]),
              },
            ]"
            :data="detection?.frankenphp ?? []"
            :bordered="false"
            :pagination="false"
            size="small"
          />
          <n-text v-else-if="detection" depth="3">
            未检测到本地 FrankenPHP 二进制
          </n-text>
        </div>

        <!-- 数据库服务检测列表 -->
        <div>
          <n-text depth="2" style="font-size: 14px; font-weight: 600">
            数据库服务
          </n-text>
          <n-data-table
            v-if="detection && detection.services.length > 0"
            :columns="[
              { title: '类型', key: 'name', width: 120 },
              { title: '主机', key: 'host', width: 140 },
              { title: '端口', key: 'port', width: 80 },
              {
                title: '状态',
                key: 'running',
                width: 100,
                render: (row: DetectedService) =>
                  h(
                    NTag,
                    {
                      type: row.running ? 'success' : 'error',
                      size: 'small',
                    },
                    () => (row.running ? '运行中' : '未响应'),
                  ),
              },
            ]"
            :data="detection?.services ?? []"
            :bordered="false"
            :pagination="false"
            size="small"
          />
          <n-text v-else-if="detection" depth="3">
            未检测到本地数据库服务
          </n-text>
        </div>
      </n-space>
    </n-card>

    <!-- 运行时管理 -->
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

      </n-space>
    </n-card>
  </n-space>
</template>
