<script setup lang="ts">
import { onMounted, ref, h } from "vue";
import { useMessage, useDialog, NButton, NTag } from "naive-ui";
import DirectoryPicker from "./DirectoryPicker.vue";
import {
  dbServiceList,
  dbServiceDetect,
  dbServiceRegister,
  dbServiceUpdate,
  dbServiceRemove,
  dbServiceStart,
  dbServiceStop,
  dbServiceStatus,
  dbServiceLog,
  dbServiceDownloadPresets,
  dbServiceDownload,
  dbServiceRegisterConnection,
  dbServiceDatabases,
  dbServiceDatabaseCreate,
  dbServiceDatabaseDrop,
  dbServiceUsers,
  dbServiceUserCreate,
  dbServiceUserDrop,
  dbServiceUserPassword,
  dbServiceRootPassword,
  onDbServiceDownloadProgress,
  type ManagedService,
  type ServiceStatus,
  type ServiceDbUser,
  type DbServiceCandidate,
  type DownloadPreset,
} from "../api";

const emit = defineEmits<{ (e: "open-connection"): void }>();

const message = useMessage();
const dialog = useDialog();

const services = ref<ManagedService[]>([]);
const statuses = ref<Record<string, ServiceStatus>>({});
const candidates = ref<DbServiceCandidate[]>([]);
const selected = ref<ManagedService | null>(null);
const databases = ref<string[]>([]);
const users = ref<ServiceDbUser[]>([]);
const toggling = ref<string | null>(null);

const KIND_LABELS: Record<string, string> = {
  mysql: "MySQL",
  mariadb: "MariaDB",
  postgresql: "PostgreSQL",
  redis: "Redis",
};

/** 建库/建账号有意义的引擎 */
function isSqlKind(s: ManagedService | null): boolean {
  return !!s && (s.kind === "mysql" || s.kind === "mariadb" || s.kind === "postgresql");
}

async function loadServices(selectFirst = false) {
  try {
    services.value = await dbServiceList();
    await refreshStatuses();
    if (selectFirst && services.value.length > 0 && !selected.value) {
      await selectService(services.value[0]);
    }
    if (selected.value) {
      const fresh = services.value.find((s) => s.id === selected.value!.id);
      selected.value = fresh ?? null;
      if (fresh) await loadServiceDetail(fresh);
    }
  } catch (e) {
    message.error(`加载服务列表失败：${e}`);
  }
}

async function refreshStatuses() {
  const map: Record<string, ServiceStatus> = {};
  await Promise.all(
    services.value.map(async (s) => {
      try {
        map[s.id] = await dbServiceStatus(s.id);
      } catch {
        map[s.id] = { running: false, pid: null };
      }
    }),
  );
  statuses.value = map;
}

async function detectServices() {
  try {
    candidates.value = await dbServiceDetect();
    if (candidates.value.length === 0) {
      message.info("未检测到本机数据库服务（端口/安装目录/系统服务均无信号）");
    }
  } catch (e) {
    message.error(`检测失败：${e}`);
  }
}

async function selectService(s: ManagedService) {
  selected.value = s;
  await loadServiceDetail(s);
}

async function loadServiceDetail(s: ManagedService) {
  if (isSqlKind(s)) {
    await Promise.all([loadDatabases(s), loadUsers(s)]);
  } else {
    databases.value = [];
    users.value = [];
  }
}

async function loadDatabases(s: ManagedService) {
  try {
    databases.value = await dbServiceDatabases(s.id);
  } catch (e) {
    message.error(`加载数据库列表失败：${e}`);
  }
}

async function loadUsers(s: ManagedService) {
  try {
    users.value = await dbServiceUsers(s.id);
  } catch {
    // 未启动或凭据缺失时静默，操作时会给出明确报错
  }
}

async function startService(s: ManagedService) {
  toggling.value = s.id;
  try {
    message.info(`正在启动 ${s.name}（便携服务首次启动需初始化数据目录）…`);
    await dbServiceStart(s.id);
    message.success(`${s.name} 已启动`);
  } catch (e) {
    message.error(`${e}`);
    showLog(s);
  } finally {
    toggling.value = null;
    await loadServices();
  }
}

async function stopService(s: ManagedService) {
  toggling.value = s.id;
  try {
    await dbServiceStop(s.id);
    message.success(`${s.name} 已停止`);
  } catch (e) {
    message.error(`${e}`);
  } finally {
    toggling.value = null;
    await loadServices();
  }
}

function removeService(s: ManagedService) {
  dialog.warning({
    title: "删除服务",
    content: `确定删除服务「${s.name}」？仅移除 RunPHP 的注册信息，${s.source === "portable" ? "便携服务的数据目录会保留" : "不影响系统服务本身"}。`,
    positiveText: "删除",
    negativeText: "取消",
    onPositiveClick: async () => {
      try {
        await dbServiceRemove(s.id);
        message.success("已删除");
        if (selected.value?.id === s.id) selected.value = null;
        await loadServices();
      } catch (e) {
        message.error(`删除失败：${e}`);
      }
    },
  });
}

/** 一键接管检测到的候选服务 */
async function takeoverCandidate(c: DbServiceCandidate) {
  try {
    await dbServiceRegister({
      kind: c.kind,
      name: c.name,
      port: c.port,
      os_service_name: c.os_service_name,
      root_username: c.kind === "postgresql" ? "postgres" : "root",
    });
    message.success(`已接管 ${c.name}（${c.os_service_name ?? "进程/端口"}）`);
    candidates.value = candidates.value.filter((x) => x.kind !== c.kind);
    await loadServices(true);
  } catch (e) {
    message.error(`接管失败：${e}`);
  }
}

async function openInConnectionTab() {
  if (!selected.value) return;
  try {
    await dbServiceRegisterConnection(selected.value.id);
    message.success("已注册为连接档案，正在跳转连接页");
    emit("open-connection");
  } catch (e) {
    message.error(`注册连接失败：${e}`);
  }
}

// ---- 添加服务（接管 / 导入便携二进制） ----
const showAdd = ref(false);
const addForm = ref({
  kind: "mysql" as ManagedService["kind"],
  name: "",
  port: 3306,
  source: "takeover" as "takeover" | "portable",
  binary_path: "" as string | null,
  os_service_name: "" as string | null,
  root_username: "root",
  root_password: "",
  autostart: true,
});
const showBinaryPicker = ref(false);

function openAdd() {
  addForm.value = {
    kind: "mysql",
    name: "",
    port: 3306,
    source: "takeover",
    binary_path: null,
    os_service_name: null,
    root_username: "root",
    root_password: "",
    autostart: true,
  };
  showAdd.value = true;
}

function onAddKindChange(kind: string) {
  const ports: Record<string, number> = {
    mysql: 3306,
    mariadb: 3306,
    postgresql: 5432,
    redis: 6379,
  };
  addForm.value.port = ports[kind] ?? 3306;
  addForm.value.root_username = kind === "postgresql" ? "postgres" : "root";
}

async function submitAdd() {
  const f = addForm.value;
  if (!f.name.trim()) {
    message.warning("请填写服务名称");
    return;
  }
  if (f.source === "portable" && !f.binary_path) {
    message.warning("便携托管需要选择服务端二进制文件");
    return;
  }
  try {
    await dbServiceRegister({
      kind: f.kind,
      name: f.name.trim(),
      port: f.port,
      binary_path: f.source === "portable" ? f.binary_path : null,
      os_service_name: f.source === "takeover" ? f.os_service_name || null : null,
      root_username: f.root_username,
      root_password: f.root_password,
      autostart: f.autostart,
    });
    message.success("服务已添加");
    showAdd.value = false;
    await loadServices(true);
  } catch (e) {
    message.error(`添加失败：${e}`);
  }
}

// ---- 下载便携包 ----
const showDownload = ref(false);
const presets = ref<DownloadPreset[]>([]);
const customUrl = ref("");
const downloading = ref(false);
const downloadProgress = ref({ active: false, transferred: 0, total: 0 });

async function openDownload() {
  presets.value = await dbServiceDownloadPresets();
  customUrl.value = "";
  showDownload.value = true;
}

async function startDownload(preset?: { kind: ManagedService["kind"]; label: string; url: string }) {
  const kind = preset?.kind ?? addForm.value.kind;
  const url = preset?.url ?? customUrl.value.trim();
  const name = preset?.label ?? `便携 ${KIND_LABELS[kind] ?? kind}`;
  if (!url) {
    message.warning("请填写下载 URL 或选择预设");
    return;
  }
  downloading.value = true;
  downloadProgress.value = { active: true, transferred: 0, total: 0 };
  const unlisten = await onDbServiceDownloadProgress((p) => {
    downloadProgress.value = { active: true, ...p };
  });
  try {
    await dbServiceDownload(kind, name, url);
    message.success("下载完成，服务已注册");
    showDownload.value = false;
    await loadServices(true);
  } catch (e) {
    message.error(`下载失败：${e}`);
  } finally {
    unlisten();
    downloading.value = false;
    downloadProgress.value.active = false;
  }
}

// ---- 服务设置（端口 / 自启 / root 凭据） ----
const editForm = ref<ManagedService | null>(null);
const showRootPassword = ref(false);
const rootPasswordValue = ref("");

function openSettings() {
  editForm.value = selected.value ? { ...selected.value } : null;
}

async function saveSettings() {
  if (!editForm.value) return;
  try {
    await dbServiceUpdate(editForm.value);
    message.success("已保存");
    await loadServices();
  } catch (e) {
    message.error(`保存失败：${e}`);
  }
}

async function changeRootPassword() {
  if (!selected.value || !rootPasswordValue.value) return;
  try {
    await dbServiceRootPassword(selected.value.id, rootPasswordValue.value);
    message.success("密码已更新（服务端与注册凭据同步生效）");
    showRootPassword.value = false;
    rootPasswordValue.value = "";
    await loadServices();
  } catch (e) {
    message.error(`${e}`);
  }
}

// ---- 建库 / 建用户 ----
const showCreateDb = ref(false);
const newDbName = ref("");
const showCreateUser = ref(false);
const newUser = ref({ username: "", password: "", database: "" });
const showUserPassword = ref(false);
const userPasswordTarget = ref<{ username: string; host: string } | null>(null);
const userPasswordValue = ref("");

async function submitCreateDb() {
  if (!selected.value || !newDbName.value.trim()) return;
  try {
    await dbServiceDatabaseCreate(selected.value.id, newDbName.value.trim());
    message.success("数据库已创建");
    showCreateDb.value = false;
    newDbName.value = "";
    await loadDatabases(selected.value);
  } catch (e) {
    message.error(`${e}`);
  }
}

function dropDatabase(name: string) {
  if (!selected.value) return;
  dialog.warning({
    title: "删除数据库",
    content: `确定删除数据库「${name}」？数据将丢失且不可恢复。`,
    positiveText: "删除",
    negativeText: "取消",
    onPositiveClick: async () => {
      try {
        await dbServiceDatabaseDrop(selected.value!.id, name);
        message.success("已删除");
        await loadDatabases(selected.value!);
      } catch (e) {
        message.error(`${e}`);
      }
    },
  });
}

async function submitCreateUser() {
  if (!selected.value) return;
  const u = newUser.value;
  if (!u.username.trim() || !u.password) {
    message.warning("请填写用户名和密码");
    return;
  }
  try {
    await dbServiceUserCreate(
      selected.value.id,
      u.username.trim(),
      u.password,
      u.database.trim() || undefined,
    );
    message.success("账号已创建");
    showCreateUser.value = false;
    newUser.value = { username: "", password: "", database: "" };
    await loadUsers(selected.value);
  } catch (e) {
    message.error(`${e}`);
  }
}

function dropUser(row: { username: string; host: string }) {
  if (!selected.value) return;
  dialog.warning({
    title: "删除账号",
    content: `确定删除账号「${row.username}」？`,
    positiveText: "删除",
    negativeText: "取消",
    onPositiveClick: async () => {
      try {
        await dbServiceUserDrop(selected.value!.id, row.username, row.host);
        message.success("已删除");
        await loadUsers(selected.value!);
      } catch (e) {
        message.error(`${e}`);
      }
    },
  });
}

function promptUserPassword(row: { username: string; host: string }) {
  userPasswordTarget.value = row;
  userPasswordValue.value = "";
  showUserPassword.value = true;
}

async function submitUserPassword() {
  if (!selected.value || !userPasswordTarget.value || !userPasswordValue.value) return;
  try {
    await dbServiceUserPassword(
      selected.value.id,
      userPasswordTarget.value.username,
      userPasswordTarget.value.host,
      userPasswordValue.value,
    );
    message.success("密码已更新");
    showUserPassword.value = false;
  } catch (e) {
    message.error(`${e}`);
  }
}

// ---- 日志 ----
const showLogModal = ref(false);
const logText = ref("");

async function showLog(s: ManagedService) {
  try {
    logText.value = await dbServiceLog(s.id, 200);
    showLogModal.value = true;
  } catch (e) {
    message.error(`读取日志失败：${e}`);
  }
}

onMounted(() => {
  void loadServices();
  void detectServices();
});
</script>

<template>
  <n-space vertical size="large">
    <!-- 检测到的候选服务 -->
    <n-alert
      v-if="candidates.length > 0"
      type="info"
      title="检测到本机数据库服务"
      style="margin-bottom: 0"
    >
      <n-space>
        <n-tag v-for="c in candidates" :key="c.kind" :bordered="false" type="success">
          {{ c.name }} · 端口 {{ c.port }} · {{ c.running ? "运行中" : "未运行" }}
          <n-button
            size="tiny"
            type="primary"
            style="margin-left: 8px"
            @click="takeoverCandidate(c)"
          >
            接管
          </n-button>
        </n-tag>
      </n-space>
    </n-alert>

    <!-- 服务列表 -->
    <n-card title="数据库服务" size="small">
      <template #header-extra>
        <n-space>
          <n-button size="small" @click="detectServices">检测本机服务</n-button>
          <n-button size="small" @click="openDownload">下载便携包</n-button>
          <n-button size="small" type="primary" @click="openAdd">+ 添加服务</n-button>
        </n-space>
      </template>
      <n-data-table
        :columns="[
          { title: '名称', key: 'name' },
          {
            title: '类型',
            key: 'kind',
            width: 110,
            render: (s: ManagedService) =>
              h(NTag, { size: 'small', bordered: false }, { default: () => KIND_LABELS[s.kind] ?? s.kind }),
          },
          {
            title: '来源',
            key: 'source',
            width: 90,
            render: (s: ManagedService) =>
              h(NTag, { size: 'small', type: s.source === 'portable' ? 'info' : 'warning', bordered: false },
                { default: () => (s.source === 'portable' ? '便携托管' : '接管') }),
          },
          { title: '端口', key: 'port', width: 80 },
          {
            title: '状态',
            key: 'status',
            width: 100,
            render: (s: ManagedService) =>
              h(
                NTag,
                { size: 'small', type: statuses[s.id]?.running ? 'success' : 'default', bordered: false },
                { default: () => (statuses[s.id]?.running ? '运行中' : '已停止') },
              ),
          },
          {
            title: '操作',
            key: 'actions',
            width: 300,
            render: (s: ManagedService) =>
              h('div', { style: 'display:flex; gap:6px' }, [
                statuses[s.id]?.running
                  ? h(NButton, { size: 'small', loading: toggling === s.id, onClick: () => stopService(s) } as Record<string, unknown>, () => '停止')
                  : h(NButton, { size: 'small', type: 'primary', loading: toggling === s.id, onClick: () => startService(s) } as Record<string, unknown>, () => '启动'),
                h(NButton, { size: 'small', onClick: () => selectService(s) } as Record<string, unknown>, () => '管理'),
                h(NButton, { size: 'small', onClick: () => showLog(s) } as Record<string, unknown>, () => '日志'),
                h(NButton, { size: 'small', type: 'error', onClick: () => removeService(s) } as Record<string, unknown>, () => '删除'),
              ]),
          },
        ]"
        :data="services"
        :bordered="false"
        :pagination="false"
        :row-props="
          (s: ManagedService) => ({
            style: 'cursor: pointer',
            onClick: () => selectService(s),
          })
        "
      />
      <n-empty v-if="services.length === 0" description="尚无受管数据库服务：点击「检测本机服务」接管已有安装，或「下载便携包」由 RunPHP 托管" style="padding: 24px 0" />
    </n-card>

    <!-- 选中服务的管理面板 -->
    <n-card v-if="selected" :title="`服务管理 · ${selected.name}`" size="small">
      <template #header-extra>
        <n-space align="center">
          <n-tag :type="statuses[selected.id]?.running ? 'success' : 'default'" size="small">
            {{ statuses[selected.id]?.running ? "运行中" : "已停止" }}
          </n-tag>
          <n-button size="small" @click="openSettings">设置</n-button>
          <n-button size="small" @click="openInConnectionTab">在连接页打开</n-button>
        </n-space>
      </template>

      <n-space v-if="isSqlKind(selected)" vertical size="large">
        <!-- 数据库管理 -->
        <div>
          <n-space align="center" style="margin-bottom: 8px">
            <n-text strong>数据库（{{ databases.length }}）</n-text>
            <n-button size="small" type="primary" @click="showCreateDb = true">+ 建库</n-button>
          </n-space>
          <n-data-table
            :columns="[
              { title: '数据库名', key: 'name' },
              {
                title: '操作',
                key: 'op',
                width: 100,
                render: (row: { name: string }) =>
                  h(NButton, { size: 'small', type: 'error', onClick: () => dropDatabase(row.name) } as Record<string, unknown>, () => '删除'),
              },
            ]"
            :data="databases.map((d) => ({ name: d }))"
            :bordered="false"
            size="small"
            :pagination="false"
            :max-height="200"
          />
        </div>

        <!-- 账号管理 -->
        <div>
          <n-space align="center" style="margin-bottom: 8px">
            <n-text strong>账号（{{ users.length }}）</n-text>
            <n-button size="small" type="primary" @click="showCreateUser = true">+ 建账号</n-button>
          </n-space>
          <n-data-table
            :columns="[
              { title: '用户名', key: 'username' },
              { title: '主机', key: 'host', width: 120 },
              {
                title: '操作',
                key: 'op',
                width: 160,
                render: (row: { username: string; host: string }) =>
                  h('div', { style: 'display:flex; gap:6px' }, [
                    h(NButton, { size: 'small', onClick: () => promptUserPassword(row) } as Record<string, unknown>, () => '改密'),
                    h(NButton, { size: 'small', type: 'error', onClick: () => dropUser(row) } as Record<string, unknown>, () => '删除'),
                  ]),
              },
            ]"
            :data="users"
            :bordered="false"
            size="small"
            :pagination="false"
            :max-height="200"
          />
        </div>
      </n-space>

      <n-space v-else-if="selected.kind === 'redis'" vertical>
        <n-text depth="3">
          Redis 无数据库/账号概念。可设置访问密码（requirepass），保存后立即生效并随服务重启保留。
        </n-text>
        <n-button size="small" type="primary" @click="showRootPassword = true">
          设置访问密码
        </n-button>
      </n-space>
    </n-card>

    <!-- 添加服务模态框 -->
    <n-modal v-model:show="showAdd" preset="card" title="添加数据库服务" style="width: 520px">
      <n-form label-placement="left" :label-width="90">
        <n-form-item label="引擎">
          <n-select
            v-model:value="addForm.kind"
            :options="[
              { label: 'MySQL', value: 'mysql' },
              { label: 'MariaDB', value: 'mariadb' },
              { label: 'PostgreSQL', value: 'postgresql' },
              { label: 'Redis', value: 'redis' },
            ]"
            @update:value="onAddKindChange"
          />
        </n-form-item>
        <n-form-item label="名称">
          <n-input v-model:value="addForm.name" placeholder="如：本机 MySQL" />
        </n-form-item>
        <n-form-item label="来源">
          <n-radio-group v-model:value="addForm.source">
            <n-radio value="takeover">接管系统服务</n-radio>
            <n-radio value="portable">便携托管二进制</n-radio>
          </n-radio-group>
        </n-form-item>
        <n-form-item v-if="addForm.source === 'takeover'" label="系统服务名">
          <n-input
            v-model:value="addForm.os_service_name"
            placeholder="如 MySQL80 / postgresql-x64-16，留空则仅按端口探测管理"
          />
        </n-form-item>
        <n-form-item v-if="addForm.source === 'portable'" label="二进制">
          <n-input
            :value="addForm.binary_path ?? ''"
            placeholder="选择 mysqld / postgres / redis-server 可执行文件"
            @click="showBinaryPicker = true"
          />
        </n-form-item>
        <n-form-item label="端口">
          <n-input-number v-model:value="addForm.port" :min="1" :max="65535" style="width: 100%" />
        </n-form-item>
        <n-form-item label="管理员用户">
          <n-input v-model:value="addForm.root_username" placeholder="root / postgres" />
        </n-form-item>
        <n-form-item label="管理员密码">
          <n-input
            v-model:value="addForm.root_password"
            type="password"
            show-password-on="click"
            placeholder="便携 MySQL 首次初始化可留空"
          />
        </n-form-item>
        <n-form-item label="随应用自启">
          <n-switch v-model:value="addForm.autostart" />
        </n-form-item>
      </n-form>
      <template #footer>
        <n-space justify="end">
          <n-button @click="showAdd = false">取消</n-button>
          <n-button type="primary" @click="submitAdd">添加</n-button>
        </n-space>
      </template>
    </n-modal>

    <!-- 二进制选择器 -->
    <DirectoryPicker v-model:show="showBinaryPicker" mode="file" @select="(p: string) => (addForm.binary_path = p)" />

    <!-- 下载便携包模态框 -->
    <n-modal v-model:show="showDownload" preset="card" title="下载便携包" style="width: 560px">
      <n-space vertical>
        <n-text v-if="presets.length === 0" depth="3">
          当前平台无预置下载源（Linux 建议用系统包管理器安装后接管），可使用自定义 zip URL。
        </n-text>
        <n-space v-else vertical>
          <n-button
            v-for="p in presets"
            :key="p.url"
            :loading="downloading"
            :disabled="downloading"
            block
            @click="startDownload(p)"
          >
            {{ p.label }}（{{ p.size_hint }}）
          </n-button>
        </n-space>
        <n-divider style="margin: 8px 0">自定义下载</n-divider>
        <n-space>
          <n-select
            v-model:value="addForm.kind"
            :options="[
              { label: 'MySQL', value: 'mysql' },
              { label: 'MariaDB', value: 'mariadb' },
              { label: 'PostgreSQL', value: 'postgresql' },
              { label: 'Redis', value: 'redis' },
            ]"
            style="width: 140px"
          />
          <n-input v-model:value="customUrl" placeholder="zip 下载 URL" style="flex: 1" />
          <n-button type="primary" :loading="downloading" :disabled="downloading" @click="startDownload()">
            下载
          </n-button>
        </n-space>
        <div v-if="downloadProgress.active">
          <n-progress
            type="line"
            :percentage="
              downloadProgress.total > 0
                ? Math.min(100, Math.round((downloadProgress.transferred / downloadProgress.total) * 100))
                : 0
            "
            :show-indicator="downloadProgress.total > 0"
          />
        </div>
      </n-space>
    </n-modal>

    <!-- 服务设置模态框 -->
    <n-modal :show="!!editForm" preset="card" title="服务设置" style="width: 480px" @update:show="(v: boolean) => !v && (editForm = null)">
      <n-form v-if="editForm" label-placement="left" :label-width="90">
        <n-form-item label="名称">
          <n-input v-model:value="editForm.name" />
        </n-form-item>
        <n-form-item label="端口">
          <n-input-number v-model:value="editForm.port" :min="1" :max="65535" style="width: 100%" />
        </n-form-item>
        <n-form-item label="管理员用户">
          <n-input v-model:value="editForm.root_username" />
        </n-form-item>
        <n-form-item label="管理员密码">
          <n-input
            v-model:value="editForm.root_password"
            type="password"
            show-password-on="click"
            placeholder="仅保存为连接凭据，修改服务端密码请用「改密」"
          />
        </n-form-item>
        <n-form-item label="随应用自启">
          <n-switch v-model:value="editForm.autostart" />
        </n-form-item>
      </n-form>
      <template #footer>
        <n-space justify="end">
          <n-button @click="editForm = null">取消</n-button>
          <n-button type="primary" @click="saveSettings">保存</n-button>
        </n-space>
      </template>
    </n-modal>

    <!-- root/Redis 访问密码模态框 -->
    <n-modal v-model:show="showRootPassword" preset="card" title="设置密码" style="width: 400px">
      <n-input
        v-model:value="rootPasswordValue"
        type="password"
        show-password-on="click"
        placeholder="新密码"
        @keydown.enter="changeRootPassword"
      />
      <template #footer>
        <n-space justify="end">
          <n-button @click="showRootPassword = false">取消</n-button>
          <n-button type="primary" @click="changeRootPassword">保存</n-button>
        </n-space>
      </template>
    </n-modal>

    <!-- 建库模态框 -->
    <n-modal v-model:show="showCreateDb" preset="card" title="创建数据库" style="width: 400px">
      <n-input v-model:value="newDbName" placeholder="数据库名（字母/数字/下划线/连字符）" @keydown.enter="submitCreateDb" />
      <template #footer>
        <n-space justify="end">
          <n-button @click="showCreateDb = false">取消</n-button>
          <n-button type="primary" @click="submitCreateDb">创建</n-button>
        </n-space>
      </template>
    </n-modal>

    <!-- 建账号模态框 -->
    <n-modal v-model:show="showCreateUser" preset="card" title="创建账号" style="width: 440px">
      <n-form label-placement="left" :label-width="90">
        <n-form-item label="用户名">
          <n-input v-model:value="newUser.username" placeholder="字母/数字/下划线/连字符" />
        </n-form-item>
        <n-form-item label="密码">
          <n-input v-model:value="newUser.password" type="password" show-password-on="click" />
        </n-form-item>
        <n-form-item label="授权数据库">
          <n-select
            v-model:value="newUser.database"
            :options="databases.map((d) => ({ label: d, value: d }))"
            placeholder="可选，授予该库全部权限"
            clearable
          />
        </n-form-item>
      </n-form>
      <template #footer>
        <n-space justify="end">
          <n-button @click="showCreateUser = false">取消</n-button>
          <n-button type="primary" @click="submitCreateUser">创建</n-button>
        </n-space>
      </template>
    </n-modal>

    <!-- 账号改密模态框 -->
    <n-modal v-model:show="showUserPassword" preset="card" title="修改账号密码" style="width: 400px">
      <n-input
        v-model:value="userPasswordValue"
        type="password"
        show-password-on="click"
        placeholder="新密码"
        @keydown.enter="submitUserPassword"
      />
      <template #footer>
        <n-space justify="end">
          <n-button @click="showUserPassword = false">取消</n-button>
          <n-button type="primary" @click="submitUserPassword">保存</n-button>
        </n-space>
      </template>
    </n-modal>

    <!-- 日志模态框 -->
    <n-modal v-model:show="showLogModal" preset="card" title="服务日志（末尾 200 行）" style="width: 720px">
      <n-code :code="logText || '（暂无日志）'" language="text" style="white-space: pre-wrap; max-height: 400px; overflow: auto" />
    </n-modal>
  </n-space>
</template>

<script lang="ts">
export default { name: "DbServicePanel" };
</script>
