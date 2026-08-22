<script setup lang="ts">
import { onMounted, ref, computed, h } from "vue";
import { useMessage, useDialog, NButton, NTag } from "naive-ui";
import { useRoute, useRouter } from "vue-router";
import DbServicePanel from "../components/DbServicePanel.vue";
import {
  dbSqliteList,
  dbSqliteCreate,
  dbSqliteDelete,
  dbSqliteTables,
  dbSqliteQueryTable,
  dbSqliteExecute,
  dbRemoteList,
  dbRemoteAdd,
  dbRemoteRemove,
  dbRemoteTest,
  dbRemoteTables,
  dbRemoteQueryTable,
  dbRemoteExecute,
  dbLibsqlList,
  dbLibsqlAdd,
  dbLibsqlRemove,
  dbLibsqlTest,
  dbLibsqlTables,
  dbLibsqlQueryTable,
  dbLibsqlExecute,
  adminerManage,
  type DatabaseFile,
  type TableInfo,
  type QueryResult,
  type ConnectionProfile,
  type DbDriver,
  type SslMode,
  type LibsqlProfile,
  type LibsqlMode,
} from "../api";

const message = useMessage();
const dialog = useDialog();
const route = useRoute();
const router = useRouter();

/** 页面主 Tab：服务端管理为默认，连接管理保留为二级入口 */
const activeTab = ref("service");

/** 子面板请求跳转连接页（服务已注册为连接档案） */
function openConnectionTab() {
  activeTab.value = "connection";
  void refreshAll();
}

// ---- 统一选中模型 ----
interface ActiveDb {
  kind: "sqlite" | "remote" | "libsql";
  key: string;
  label: string;
  profile?: ConnectionProfile;
  libsqlProfile?: LibsqlProfile;
}

const activeDb = ref<ActiveDb | null>(null);
const tables = ref<TableInfo[]>([]);
const selectedTable = ref<string | null>(null);
const queryResult = ref<QueryResult | null>(null);
const sqlEditor = ref("SELECT * FROM sqlite_master WHERE type='table'");
const sqlPlaceholder = "输入 SQL 语句…";
const loadingTables = ref(false);
const loadingQuery = ref(false);

const sqliteDbs = ref<DatabaseFile[]>([]);
const remoteProfiles = ref<ConnectionProfile[]>([]);
const libsqlProfiles = ref<LibsqlProfile[]>([]);

const DEFAULT_PORTS: Record<string, number> = {
  sqlite: 0,
  mysql: 3306,
  postgres: 5432,
  mongodb: 27017,
  redis: 6379,
  qdrant: 6333,
};

/** 数据库类型选项（统一添加模态框） */
const dbTypeOptions = [
  { label: "SQLite（内置）", value: "sqlite" },
  { label: "libSQL（本地/远程/副本）", value: "libsql" },
  { label: "MySQL / MariaDB", value: "mysql" },
  { label: "PostgreSQL", value: "postgres" },
  { label: "MongoDB", value: "mongodb" },
  { label: "Redis", value: "redis" },
  { label: "Qdrant", value: "qdrant" },
];

/** libSQL 连接模式选项 */
const libsqlModeOptions = [
  { label: "本地文件", value: "local" },
  { label: "远程连接", value: "remote" },
  { label: "嵌入式副本", value: "replica" },
];

/** SSL 模式选项 */
const sslModeOptions = [
  { label: "禁用", value: "disabled" },
  { label: "优先（失败回退明文）", value: "preferred" },
  { label: "必须", value: "required" },
];

/** 创建空白远程连接档案（含 SSL/SSH 默认值） */
function blankProfile(driver: DbDriver): ConnectionProfile {
  return {
    id: "",
    name: "",
    driver,
    host: "127.0.0.1",
    port: DEFAULT_PORTS[driver] ?? 3306,
    username: "root",
    password: "",
    database: null,
    created_at: "",
    ssl_mode: null,
    ssl_ca: null,
    ssh_host: null,
    ssh_port: null,
    ssh_user: null,
    ssh_key: null,
    ssh_password: null,
  };
}

/** 可浏览表结构的类型 */
function isBrowsable(driver: string): boolean {
  return (
    driver === "sqlite" ||
    driver === "mysql" ||
    driver === "postgres" ||
    driver === "libsql"
  );
}

/** Adminer 支持管理的类型 */
function isAdminerSupported(driver: string): boolean {
  return driver === "sqlite" || driver === "mysql" || driver === "postgres";
}

// ---- 统一添加模态框 ----
const showAdd = ref(false);
const addType = ref<string>("sqlite");
// SQLite 字段
const addSqliteName = ref("");
// 远程字段
const addRemote = ref<ConnectionProfile>(blankProfile("mysql"));
// libSQL 字段
const addLibsql = ref<LibsqlProfile>(blankLibsqlProfile("local"));
// 高级选项（SSL/SSH）折叠开关
const showAdvanced = ref(false);

/** 创建空白 libSQL 连接档案 */
function blankLibsqlProfile(mode: LibsqlMode): LibsqlProfile {
  return {
    id: "",
    name: "",
    mode,
    path: null,
    url: null,
    auth_token: null,
    created_at: "",
  };
}

/** 类型切换时重置端口默认值 */
function onTypeChange(v: string) {
  addType.value = v;
  if (v === "libsql") {
    addLibsql.value = blankLibsqlProfile("local");
  } else if (v !== "sqlite") {
    addRemote.value = blankProfile(v as DbDriver);
  }
}

/** 统一添加：根据类型分发 */
async function addDatabase() {
  if (addType.value === "sqlite") {
    if (!addSqliteName.value.trim()) {
      message.warning("请输入数据库名称");
      return;
    }
    try {
      await dbSqliteCreate(addSqliteName.value.trim());
      message.success("SQLite 数据库创建成功");
      showAdd.value = false;
      addSqliteName.value = "";
      await loadSqliteDbs();
    } catch (e) {
      message.error(`创建失败：${e}`);
    }
  } else if (addType.value === "libsql") {
    const p = addLibsql.value;
    if (!p.name.trim()) {
      message.warning("请填写名称");
      return;
    }
    if (p.mode === "local" && !p.path?.trim()) {
      message.warning("本地模式需要提供文件路径");
      return;
    }
    if (p.mode !== "local" && !p.url?.trim()) {
      message.warning("远程/副本模式需要提供 URL");
      return;
    }
    p.id = crypto.randomUUID();
    p.created_at = new Date().toISOString();
    try {
      await dbLibsqlAdd(p);
      message.success("libSQL 连接档案已保存");
      showAdd.value = false;
      await loadLibsqlProfiles();
    } catch (e) {
      message.error(`保存失败：${e}`);
    }
  } else {
    const p = addRemote.value;
    if (!p.name.trim() || !p.host.trim()) {
      message.warning("请填写名称和主机");
      return;
    }
    p.id = crypto.randomUUID();
    p.created_at = new Date().toISOString();
    p.port = p.port || (DEFAULT_PORTS[p.driver] ?? 3306);
    try {
      await dbRemoteAdd(p);
      message.success("连接档案已保存");
      showAdd.value = false;
      await loadRemoteProfiles();
    } catch (e) {
      message.error(`保存失败：${e}`);
    }
  }
}

// ---- 统一数据库列表（合并 SQLite + 远程为表格行） ----
interface DbRow {
  name: string;
  type: string;
  host: string;
  port: number | string;
  database: string;
  raw: ActiveDb;
}

/** 合并后的数据库列表 */
const dbRows = computed<DbRow[]>(() => {
  const rows: DbRow[] = [];
  for (const d of sqliteDbs.value) {
    rows.push({
      name: d.name,
      type: "SQLite",
      host: "本地",
      port: "—",
      database: d.name,
      raw: { kind: "sqlite", key: d.name, label: d.name },
    });
  }
  for (const p of remoteProfiles.value) {
    rows.push({
      name: p.name,
      type: p.driver.toUpperCase(),
      host: p.host,
      port: p.port,
      database: p.database ?? "—",
      raw: {
        kind: "remote",
        key: p.id,
        label: p.name,
        profile: p,
      },
    });
  }
  for (const p of libsqlProfiles.value) {
    rows.push({
      name: p.name,
      type: `libSQL/${p.mode}`,
      host: p.mode === "local" ? "本地" : (p.url ?? "—"),
      port: "—",
      database: p.path ?? p.url ?? "—",
      raw: {
        kind: "libsql",
        key: p.id,
        label: p.name,
        libsqlProfile: p,
      },
    });
  }
  return rows;
});

// ---- 选择数据库 ----
async function selectDb(row: DbRow) {
  const db = row.raw;
  activeDb.value = db;
  selectedTable.value = null;
  queryResult.value = null;
  if (db.kind === "sqlite") {
    sqlEditor.value = "SELECT * FROM sqlite_master WHERE type='table'";
  } else if (db.kind === "libsql") {
    sqlEditor.value = "SELECT name FROM sqlite_master WHERE type='table'";
  } else {
    sqlEditor.value = "SELECT 1";
  }
  const driverStr =
    db.kind === "sqlite"
      ? "sqlite"
      : db.kind === "libsql"
        ? "libsql"
        : db.profile!.driver;
  if (isBrowsable(driverStr)) {
    await loadTables();
  } else {
    tables.value = [];
  }
}

const canBrowse = computed(() => {
  if (!activeDb.value) return false;
  if (activeDb.value.kind === "sqlite") return true;
  if (activeDb.value.kind === "libsql") return true;
  return isBrowsable(activeDb.value.profile!.driver);
});

const dbTypeLabel = computed(() => {
  if (!activeDb.value) return "";
  if (activeDb.value.kind === "sqlite") return "SQLite";
  if (activeDb.value.kind === "libsql")
    return `libSQL/${activeDb.value.libsqlProfile!.mode}`;
  return activeDb.value.profile!.driver.toUpperCase();
});

const activeMenuKey = computed(() => {
  if (!activeDb.value) return undefined;
  return `${activeDb.value.kind}:${activeDb.value.key}`;
});

async function loadTables() {
  if (!activeDb.value) return;
  loadingTables.value = true;
  try {
    if (activeDb.value.kind === "sqlite") {
      tables.value = await dbSqliteTables(activeDb.value.key);
    } else if (activeDb.value.kind === "libsql") {
      tables.value = await dbLibsqlTables(activeDb.value.libsqlProfile!);
    } else {
      tables.value = await dbRemoteTables(activeDb.value.profile!);
    }
  } catch (e) {
    message.error(`加载表失败：${e}`);
  } finally {
    loadingTables.value = false;
  }
}

async function selectTable(table: string) {
  if (!activeDb.value || loadingQuery.value) return;
  selectedTable.value = table;
  loadingQuery.value = true;
  try {
    if (activeDb.value.kind === "sqlite") {
      queryResult.value = await dbSqliteQueryTable(activeDb.value.key, table, 100, 0);
    } else if (activeDb.value.kind === "libsql") {
      queryResult.value = await dbLibsqlQueryTable(activeDb.value.libsqlProfile!, table, 100, 0);
    } else {
      queryResult.value = await dbRemoteQueryTable(activeDb.value.profile!, table, 100, 0);
    }
  } catch (e) {
    message.error(`查询失败：${e}`);
  } finally {
    loadingQuery.value = false;
  }
}

async function runSql() {
  if (!activeDb.value) {
    message.warning("请先选择数据库");
    return;
  }
  if (!sqlEditor.value.trim()) {
    message.warning("请输入 SQL 语句");
    return;
  }
  if (loadingQuery.value) return;
  loadingQuery.value = true;
  const sql = sqlEditor.value;
  try {
    if (activeDb.value.kind === "sqlite") {
      queryResult.value = await dbSqliteExecute(activeDb.value.key, sql);
    } else if (activeDb.value.kind === "libsql") {
      queryResult.value = await dbLibsqlExecute(activeDb.value.libsqlProfile!, sql);
    } else {
      queryResult.value = await dbRemoteExecute(activeDb.value.profile!, sql);
    }
    message.success(`执行成功，影响 ${queryResult.value.affected} 行`);
    const upper = sql.trim().toUpperCase();
    if (upper.startsWith("CREATE") || upper.startsWith("ALTER") || upper.startsWith("DROP")) {
      selectedTable.value = null;
      queryResult.value = null;
      await loadTables();
    }
  } catch (e) {
    message.error(`执行失败：${e}`);
  } finally {
    loadingQuery.value = false;
  }
}

// ---- 管理（Adminer） ----
const managing = ref(false);

async function manageDb(row: DbRow) {
  const db = row.raw;
  const driver = db.kind === "sqlite" ? "sqlite" : db.profile!.driver;
  if (!isAdminerSupported(driver)) {
    message.warning(`${driver} 暂不支持 Adminer 管理，仅支持连接测试`);
    return;
  }
  managing.value = true;
  try {
    const params: Record<string, unknown> = { db_type: driver };
    if (db.kind === "sqlite") {
      // SQLite 需要绝对路径，从数据目录拼接
      params.path = db.key;
    } else {
      const p = db.profile!;
      params.host = p.host;
      params.port = p.port;
      params.username = p.username;
      params.password = p.password;
      params.database = p.database;
    }
    const url = await adminerManage(params as any);
    // 在新窗口打开 Adminer
    window.open(url, "_blank");
    message.success("正在打开 Adminer 管理界面");
  } catch (e) {
    message.error(`打开管理失败：${e}`);
  } finally {
    managing.value = false;
  }
}

// ---- 优化 ----
async function optimizeDb(row: DbRow) {
  const db = row.raw;
  if (db.kind === "sqlite") {
    sqlEditor.value = "VACUUM;";
    activeDb.value = db;
    await runSql();
    message.success("SQLite VACUUM 优化已执行");
  } else {
    const driver = db.profile!.driver;
    if (driver === "mysql") {
      sqlEditor.value = "OPTIMIZE TABLE;";
      activeDb.value = db;
      message.info("MySQL 优化请针对具体表执行 OPTIMIZE TABLE");
    } else if (driver === "postgres") {
      sqlEditor.value = "VACUUM ANALYZE;";
      activeDb.value = db;
      message.info("PostgreSQL 优化请执行 VACUUM ANALYZE");
    } else {
      message.warning(`${driver} 暂不支持优化操作`);
    }
  }
}

// ---- 删除 ----
function deleteDb(row: DbRow) {
  const db = row.raw;
  dialog.warning({
    title: "删除数据库",
    content: `确定删除「${row.name}」（${row.type}）？此操作不可恢复。`,
    positiveText: "删除",
    negativeText: "取消",
    onPositiveClick: async () => {
      try {
        if (db.kind === "sqlite") {
          await dbSqliteDelete(db.key);
        } else if (db.kind === "libsql") {
          await dbLibsqlRemove(db.key);
        } else {
          await dbRemoteRemove(db.key);
        }
        message.success("已删除");
        if (activeDb.value?.key === db.key) {
          activeDb.value = null;
          tables.value = [];
          queryResult.value = null;
          selectedTable.value = null;
        }
        await refreshAll();
      } catch (e) {
        message.error(`删除失败：${e}`);
      }
    },
  });
}

// ---- 测试连接 ----
const testingId = ref<string | null>(null);

async function testRemote(profile: ConnectionProfile) {
  testingId.value = profile.id;
  try {
    const result = await dbRemoteTest(profile);
    message.success(result);
  } catch (e) {
    message.error(`连接失败：${e}`);
  } finally {
    testingId.value = null;
  }
}

async function testLibsql(profile: LibsqlProfile) {
  testingId.value = profile.id;
  try {
    const result = await dbLibsqlTest(profile);
    message.success(result);
  } catch (e) {
    message.error(`连接失败：${e}`);
  } finally {
    testingId.value = null;
  }
}

// ---- 加载 ----
async function loadSqliteDbs() {
  try {
    sqliteDbs.value = await dbSqliteList();
  } catch (e) {
    message.error(`加载 SQLite 列表失败：${e}`);
  }
}

async function loadRemoteProfiles() {
  try {
    remoteProfiles.value = await dbRemoteList();
  } catch (e) {
    message.error(`加载远程连接失败：${e}`);
  }
}

async function loadLibsqlProfiles() {
  try {
    libsqlProfiles.value = await dbLibsqlList();
  } catch (e) {
    message.error(`加载 libSQL 连接失败：${e}`);
  }
}

async function refreshAll() {
  await Promise.all([loadSqliteDbs(), loadRemoteProfiles(), loadLibsqlProfiles()]);
}

onMounted(async () => {
  await refreshAll();
  const q = route.query;
  if (q.add_db) {
    // 连接档案预填入口：切到连接页再打开添加框
    activeTab.value = "connection";
    const driver = String(q.add_db) as DbDriver;
    if (driver in DEFAULT_PORTS) {
      addType.value = driver;
      addRemote.value = blankProfile(driver);
      addRemote.value.name = q.name ? String(q.name) : "";
      addRemote.value.host = q.host ? String(q.host) : "127.0.0.1";
      addRemote.value.port = Number(q.port) || (DEFAULT_PORTS[driver] ?? 3306);
      addRemote.value.username = "";
      showAdd.value = true;
    }
    router.replace({ path: route.path, query: {} });
  }
});
</script>

<template>
  <n-space vertical size="large">
    <n-tabs v-model:value="activeTab" type="line" animated>
      <n-tab-pane name="service" tab="服务">
        <DbServicePanel @open-connection="openConnectionTab" />
      </n-tab-pane>
      <n-tab-pane name="connection" tab="连接">
        <n-card title="数据库连接">
      <!-- 顶部工具栏 -->
      <n-space align="center" style="margin-bottom: 12px">
        <n-button type="primary" @click="showAdd = true">+ 添加</n-button>
        <n-button @click="refreshAll">刷新</n-button>
      </n-space>

      <!-- 统一数据库列表（表格 + 每行操作按钮） -->
      <n-data-table
        :columns="[
          { title: '名称', key: 'name' },
          { title: '类型', key: 'type' },
          { title: '主机', key: 'host' },
          { title: '端口', key: 'port' },
          { title: '数据库', key: 'database' },
          {
            title: '操作',
            key: 'actions',
            width: 280,
            render: (row: DbRow) =>
              h('div', { style: 'display:flex; gap:6px' }, [
                h(
                  NButton,
                  {
                    size: 'small',
                    type: 'primary',
                    loading: managing && activeDb?.key === row.raw.key,
                    disabled: !isAdminerSupported(
                      row.raw.kind === 'sqlite'
                        ? 'sqlite'
                        : row.raw.profile!.driver
                    ),
                    onClick: () => manageDb(row),
                  } as Record<string, unknown>,
                  () => '管理',
                ),
                h(
                  NButton,
                  {
                    size: 'small',
                    onClick: () => optimizeDb(row),
                  } as Record<string, unknown>,
                  () => '优化',
                ),
                h(
                  NButton,
                  {
                    size: 'small',
                    type: 'error',
                    onClick: () => deleteDb(row),
                  } as Record<string, unknown>,
                  () => '删除',
                ),
              ]),
          },
        ]"
        :data="dbRows"
        :bordered="false"
        :pagination="false"
        :row-props="(row: DbRow) => ({
          style: 'cursor: pointer',
          onClick: () => selectDb(row),
        })"
      />

      <!-- 选中数据库后的三栏浏览区域 -->
      <n-layout v-if="activeDb" has-sider style="height: 420px; margin-top: 12px">
        <!-- 中间：表列表 -->
        <n-layout-sider
          v-if="canBrowse"
          :width="180"
          bordered
          content-style="padding: 8px"
        >
          <n-text depth="3" style="font-size: 12px">
            表（{{ tables.length }}）
          </n-text>
          <n-spin :show="loadingTables">
            <n-menu
              :value="selectedTable ?? undefined"
              :options="
                tables.map((t) => ({
                  label: `${t.name} (${t.row_count})`,
                  key: t.name,
                }))
              "
              @update:value="selectTable"
            />
          </n-spin>
        </n-layout-sider>

        <!-- 右侧：SQL 编辑器 / 连接详情 -->
        <n-layout content-style="padding: 12px;">
          <n-space vertical>
            <n-space align="center" justify="space-between">
              <n-space align="center">
                <n-text strong>{{ activeDb.label }}</n-text>
                <n-tag size="small" :type="activeDb.kind === 'sqlite' ? 'success' : 'info'">
                  {{ dbTypeLabel }}
                </n-tag>
              </n-space>
              <n-button
                v-if="activeDb.kind === 'remote'"
                size="small"
                :loading="testingId === activeDb.key"
                @click="testRemote(activeDb.profile!)"
              >
                测试连接
              </n-button>
              <n-button
                v-if="activeDb.kind === 'libsql'"
                size="small"
                :loading="testingId === activeDb.key"
                @click="testLibsql(activeDb.libsqlProfile!)"
              >
                测试连接
              </n-button>
            </n-space>

            <template v-if="canBrowse">
              <n-input
                v-model:value="sqlEditor"
                type="textarea"
                :autosize="{ minRows: 3, maxRows: 6 }"
                :placeholder="sqlPlaceholder"
                style="font-family: monospace"
              />
              <n-button type="primary" :loading="loadingQuery" @click="runSql">
                执行 SQL
              </n-button>

              <div v-if="queryResult" style="overflow: auto">
                <n-text depth="3" style="font-size: 12px">
                  {{ queryResult.columns.length }} 列，{{ queryResult.affected }} 行
                </n-text>
                <n-data-table
                  v-if="queryResult.rows.length > 0"
                  :columns="
                    queryResult.columns.map((c) => ({
                      title: c,
                      key: c,
                      render: (row: Record<string, unknown>) =>
                        String(row[c] ?? ''),
                    }))
                  "
                  :data="
                    queryResult.rows.map((r) => {
                      const obj: Record<string, any> = {};
                      queryResult!.columns.forEach((c, i) => { obj[c] = r[i]; });
                      return obj;
                    })
                  "
                  :max-height="300"
                  :bordered="true"
                  size="small"
                />
                <n-text v-else depth="3">无结果</n-text>
              </div>
            </template>

            <template v-else>
              <n-descriptions :column="1" label-placement="left" bordered>
                <n-descriptions-item label="类型">
                  {{ activeDb.profile?.driver }}
                </n-descriptions-item>
                <n-descriptions-item label="主机">
                  {{ activeDb.profile?.host }}:{{ activeDb.profile?.port }}
                </n-descriptions-item>
                <n-descriptions-item label="用户名">
                  {{ activeDb.profile?.username }}
                </n-descriptions-item>
                <n-descriptions-item label="数据库">
                  {{ activeDb.profile?.database ?? "—" }}
                </n-descriptions-item>
              </n-descriptions>
              <n-text depth="3">
                此数据库类型仅支持连接测试，不支持表浏览与 SQL 执行。
              </n-text>
            </template>
          </n-space>
        </n-layout>
      </n-layout>

      <!-- 统一添加模态框 -->
      <n-modal
        v-model:show="showAdd"
        preset="card"
        title="添加数据库"
        style="width: 520px"
      >
        <n-form label-placement="left" :label-width="70">
          <n-form-item label="类型">
            <n-select
              :value="addType"
              :options="dbTypeOptions"
              @update:value="onTypeChange"
            />
          </n-form-item>

          <!-- SQLite 表单 -->
          <template v-if="addType === 'sqlite'">
            <n-form-item label="名称">
              <n-input
                v-model:value="addSqliteName"
                placeholder="数据库名称"
                @keydown.enter="addDatabase"
              />
            </n-form-item>
          </template>

          <!-- libSQL 表单 -->
          <template v-else-if="addType === 'libsql'">
            <n-form-item label="名称">
              <n-input
                v-model:value="addLibsql.name"
                placeholder="如：本地 libSQL 或 Turso 实例"
              />
            </n-form-item>
            <n-form-item label="模式">
              <n-select
                v-model:value="addLibsql.mode"
                :options="libsqlModeOptions"
              />
            </n-form-item>
            <n-form-item
              v-if="addLibsql.mode === 'local' || addLibsql.mode === 'replica'"
              label="文件路径"
            >
              <n-input
                v-model:value="addLibsql.path"
                placeholder="本地 .db 文件路径（如 /data/app.db）"
              />
            </n-form-item>
            <n-form-item
              v-if="addLibsql.mode === 'remote' || addLibsql.mode === 'replica'"
              label="远程 URL"
            >
              <n-input
                v-model:value="addLibsql.url"
                placeholder="如 libsql://my-db.turso.io"
              />
            </n-form-item>
            <n-form-item
              v-if="addLibsql.mode === 'remote' || addLibsql.mode === 'replica'"
              label="Auth Token"
            >
              <n-input
                v-model:value="addLibsql.auth_token"
                type="password"
                show-password-on="click"
                placeholder="Turso/服务器认证 Token"
              />
            </n-form-item>
          </template>

          <!-- 远程连接表单 -->
          <template v-else>
            <n-form-item label="名称">
              <n-input
                v-model:value="addRemote.name"
                placeholder="如：本地 MySQL"
              />
            </n-form-item>
            <n-form-item label="主机">
              <n-input v-model:value="addRemote.host" placeholder="127.0.0.1" />
            </n-form-item>
            <n-form-item label="端口">
              <n-input-number
                v-model:value="addRemote.port"
                :min="1"
                :max="65535"
                style="width: 100%"
              />
            </n-form-item>
            <n-form-item label="用户名">
              <n-input v-model:value="addRemote.username" placeholder="root" />
            </n-form-item>
            <n-form-item label="密码">
              <n-input
                v-model:value="addRemote.password"
                type="password"
                show-password-on="click"
                placeholder="••••••"
              />
            </n-form-item>
            <n-form-item label="数据库">
              <n-input
                v-model:value="addRemote.database"
                placeholder="可选，默认数据库"
              />
            </n-form-item>

            <!-- 高级选项：SSL 加密 + SSH 隧道 -->
            <n-collapse-transition :show="showAdvanced">
              <n-divider style="margin: 8px 0">SSL 加密</n-divider>
              <n-form-item label="SSL 模式">
                <n-select
                  v-model:value="addRemote.ssl_mode"
                  :options="sslModeOptions"
                  placeholder="禁用"
                  clearable
                />
              </n-form-item>
              <n-form-item v-if="addRemote.ssl_mode && addRemote.ssl_mode !== 'disabled'" label="CA 证书">
                <n-input
                  v-model:value="addRemote.ssl_ca"
                  placeholder="CA 证书文件路径（可选，留空则跳过校验）"
                />
              </n-form-item>

              <n-divider style="margin: 8px 0">SSH 隧道</n-divider>
              <n-form-item label="SSH 主机">
                <n-input
                  v-model:value="addRemote.ssh_host"
                  placeholder="留空则不使用 SSH 隧道"
                />
              </n-form-item>
              <n-form-item v-if="addRemote.ssh_host" label="SSH 端口">
                <n-input-number
                  v-model:value="addRemote.ssh_port"
                  :min="1"
                  :max="65535"
                  placeholder="22"
                  style="width: 100%"
                />
              </n-form-item>
              <n-form-item v-if="addRemote.ssh_host" label="SSH 用户">
                <n-input
                  v-model:value="addRemote.ssh_user"
                  placeholder="root"
                />
              </n-form-item>
              <n-form-item v-if="addRemote.ssh_host" label="SSH 密钥">
                <n-input
                  v-model:value="addRemote.ssh_key"
                  placeholder="私钥文件路径（与密码二选一）"
                />
              </n-form-item>
              <n-form-item v-if="addRemote.ssh_host" label="SSH 密码">
                <n-input
                  v-model:value="addRemote.ssh_password"
                  type="password"
                  show-password-on="click"
                  placeholder="••••••"
                />
              </n-form-item>
            </n-collapse-transition>

            <n-button
              text
              type="primary"
              @click="showAdvanced = !showAdvanced"
              style="margin-top: 4px"
            >
              {{ showAdvanced ? "收起高级选项" : "展开高级选项（SSL / SSH）" }}
            </n-button>
          </template>
        </n-form>
        <template #footer>
          <n-space justify="end">
            <n-button @click="showAdd = false">取消</n-button>
            <n-button type="primary" @click="addDatabase">
              {{ addType === "sqlite" ? "创建" : "保存" }}
            </n-button>
          </n-space>
        </template>
      </n-modal>
        </n-card>
      </n-tab-pane>
    </n-tabs>
  </n-space>
</template>

<script lang="ts">
export default { name: "DatabasesView" };
</script>
