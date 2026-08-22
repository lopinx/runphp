<script setup lang="ts">
import { h, onMounted, ref } from "vue";
import { useRouter } from "vue-router";
import { useAppStore } from "../stores/app";
import { NButton, NSelect, NSwitch, NTag, NTooltip, useMessage } from "naive-ui";
import {
  runtimeSetDefault,
  runtimeVersions,
  runtimeDetectLocal,
  runtimeImportLocal,
  runtimeInstall,
  dbServiceList,
  dbServiceStatus,
  dbServiceStart,
  dbServiceStop,
  dbServiceUpdate,
  ftpServerBackend,
  ftpServerConfig,
  ftpServerStatus,
  ftpServerStart,
  ftpServerStop,
  ftpServerUpdateConfig,
  type RuntimeInfo,
  type LocalDetection,
  type DetectedBinary,
  type DetectedService,
  type ManagedService,
  type ServiceStatus,
  type FtpdConfig,
} from "../api";

const store = useAppStore();
const message = useMessage();
const router = useRouter();

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
  void loadServiceCard();
});

// ---- 数据库 / FTP 服务简单配置（完整管理在对应页面） ----
const dbServices = ref<ManagedService[]>([]);
const dbStatuses = ref<Record<string, ServiceStatus>>({});
const ftpRunning = ref(false);
const ftpBackend = ref("");
const ftpConfig = ref<FtpdConfig>({ port: 21, passive_from: 50000, passive_to: 50010, autostart: false });
const serviceToggling = ref<string | null>(null);

async function loadServiceCard() {
  try {
    dbServices.value = await dbServiceList();
    const map: Record<string, ServiceStatus> = {};
    await Promise.all(
      dbServices.value.map(async (s) => {
        try {
          map[s.id] = await dbServiceStatus(s.id);
        } catch {
          map[s.id] = { running: false, pid: null };
        }
      }),
    );
    dbStatuses.value = map;
  } catch {
    dbServices.value = [];
  }
  try {
    [ftpRunning.value, ftpBackend.value, ftpConfig.value] = await Promise.all([
      ftpServerStatus(),
      ftpServerBackend(),
      ftpServerConfig(),
    ]);
  } catch {
    // FTP 状态读取失败不阻断页面
  }
}

async function toggleDbService(s: ManagedService) {
  serviceToggling.value = s.id;
  try {
    if (dbStatuses.value[s.id]?.running) {
      await dbServiceStop(s.id);
      message.success(`${s.name} 已停止`);
    } else {
      message.info(`正在启动 ${s.name}…`);
      await dbServiceStart(s.id);
      message.success(`${s.name} 已启动`);
    }
  } catch (e) {
    message.error(`${e}`);
  } finally {
    serviceToggling.value = null;
    await loadServiceCard();
  }
}

async function toggleDbAutostart(s: ManagedService, autostart: boolean) {
  try {
    await dbServiceUpdate({ ...s, autostart });
    message.success(`${s.name} 自启已${autostart ? "开启" : "关闭"}`);
    await loadServiceCard();
  } catch (e) {
    message.error(`${e}`);
  }
}

async function toggleFtp() {
  serviceToggling.value = "ftp";
  try {
    if (ftpRunning.value) {
      await ftpServerStop();
      message.success("FTP 服务已停止");
    } else {
      const backend = await ftpServerStart();
      message.success(`FTP 服务已启动（${backend}）`);
    }
  } catch (e) {
    message.error(`${e}`);
  } finally {
    serviceToggling.value = null;
    await loadServiceCard();
  }
}

async function saveFtpConfig() {
  try {
    await ftpServerUpdateConfig(ftpConfig.value);
    message.success("FTP 配置已保存（运行中需重启生效）");
  } catch (e) {
    message.error(`${e}`);
  }
}

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
            {
              title: '路径',
              key: 'path',
              ellipsis: { tooltip: false },
              render: (row: RuntimeInfo) =>
                row.imported_from
                  ? h(
                      NTooltip,
                      { trigger: 'hover' },
                      {
                        trigger: () => h('span', { style: 'cursor: help' }, row.imported_from ?? ''),
                        default: () => `托管副本：${row.path}`,
                      },
                    )
                  : row.path,
            },
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

    <!-- 数据库与 FTP 服务简单配置（完整管理在对应页面） -->
    <n-card title="数据库与 FTP 服务">
      <n-space vertical size="large">
        <div>
          <n-space align="center" justify="space-between" style="margin-bottom: 8px">
            <n-text depth="2" style="font-size: 14px; font-weight: 600">
              数据库服务
            </n-text>
            <n-button size="small" @click="router.push('/databases')">
              前往数据库页管理
            </n-button>
          </n-space>
          <n-data-table
            :columns="[
              { title: '名称', key: 'name' },
              { title: '类型', key: 'kind', width: 110 },
              { title: '端口', key: 'port', width: 80 },
              {
                title: '状态',
                key: 'running',
                width: 100,
                render: (row: ManagedService) =>
                  h(
                    NTag,
                    { type: dbStatuses[row.id]?.running ? 'success' : 'default', size: 'small' },
                    () => (dbStatuses[row.id]?.running ? '运行中' : '已停止'),
                  ),
              },
              {
                title: '自启',
                key: 'autostart',
                width: 80,
                render: (row: ManagedService) =>
                  h(NSwitch, {
                    size: 'small',
                    value: row.autostart,
                    onUpdateValue: (v: boolean) => toggleDbAutostart(row, v),
                  } as Record<string, unknown>),
              },
              {
                title: '操作',
                key: 'actions',
                width: 90,
                render: (row: ManagedService) =>
                  h(
                    NButton,
                    {
                      size: 'small',
                      type: dbStatuses[row.id]?.running ? 'default' : 'primary',
                      loading: serviceToggling === row.id,
                      onClick: () => toggleDbService(row),
                    } as Record<string, unknown>,
                    () => (dbStatuses[row.id]?.running ? '停止' : '启动'),
                  ),
              },
            ]"
            :data="dbServices"
            :bordered="false"
            :pagination="false"
            size="small"
          />
          <n-text v-if="dbServices.length === 0" depth="3">
            尚未注册数据库服务，可在数据库页检测接管或下载便携包
          </n-text>
        </div>

        <div>
          <n-space align="center" justify="space-between" style="margin-bottom: 8px">
            <n-text depth="2" style="font-size: 14px; font-weight: 600">
              FTP 服务
            </n-text>
            <n-button size="small" @click="router.push('/ftp')">
              前往 FTP 页管理
            </n-button>
          </n-space>
          <n-space align="center" :wrap="false" style="margin-bottom: 8px">
            <n-tag :type="ftpRunning ? 'success' : 'default'" size="small">
              {{ ftpRunning ? "运行中" : "已停止" }}
            </n-tag>
            <n-tag size="small" :bordered="false">后端：{{ ftpBackend }}</n-tag>
            <n-button
              size="small"
              :type="ftpRunning ? 'default' : 'primary'"
              :loading="serviceToggling === 'ftp'"
              @click="toggleFtp"
            >
              {{ ftpRunning ? "停止" : "启动" }}
            </n-button>
          </n-space>
          <n-form inline size="small" label-placement="left" :label-width="70">
            <n-form-item label="控制端口">
              <n-input-number
                v-model:value="ftpConfig.port"
                :min="1"
                :max="65535"
                size="small"
                style="width: 110px"
              />
            </n-form-item>
            <n-form-item label="被动区间">
              <n-input-number
                v-model:value="ftpConfig.passive_from"
                :min="1024"
                :max="65535"
                size="small"
                style="width: 110px"
              />
              <span style="padding: 0 4px">—</span>
              <n-input-number
                v-model:value="ftpConfig.passive_to"
                :min="1024"
                :max="65535"
                size="small"
                style="width: 110px"
              />
            </n-form-item>
            <n-form-item label="随应用自启">
              <n-switch v-model:value="ftpConfig.autostart" size="small" />
            </n-form-item>
            <n-form-item label=" ">
              <n-button size="small" type="primary" @click="saveFtpConfig">保存</n-button>
            </n-form-item>
          </n-form>
        </div>
      </n-space>
    </n-card>
  </n-space>
</template>
