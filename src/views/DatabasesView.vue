<script setup lang="ts">
import { onMounted, ref, h } from "vue";
import { useMessage, useDialog, NButton } from "naive-ui";
import { useRoute, useRouter } from "vue-router";
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
  type DatabaseFile,
  type TableInfo,
  type QueryResult,
  type ConnectionProfile,
  type DbDriver,
} from "../api";

const message = useMessage();
const dialog = useDialog();
const route = useRoute();
const router = useRouter();

// ---- 共享类型 ----
interface ActiveDb {
  kind: "sqlite" | "remote";
  /** SQLite 文件名或远程档案 id */
  key: string;
  /** 显示名 */
  label: string;
  /** 远程连接档案（仅 remote 模式） */
  profile?: ConnectionProfile;
}

// SQLite 部分
const sqliteDbs = ref<DatabaseFile[]>([]);
const tables = ref<TableInfo[]>([]);
const selectedTable = ref<string | null>(null);
const queryResult = ref<QueryResult | null>(null);
const sqlEditor = ref("SELECT * FROM sqlite_master WHERE type='table'");
const loadingTables = ref(false);
const loadingQuery = ref(false);

const activeDb = ref<ActiveDb | null>(null);

const sqlPlaceholder = "输入 SQL 语句…";

// 远程连接部分
const remoteProfiles = ref<ConnectionProfile[]>([]);
const showAddRemote = ref(false);
const testingId = ref<string | null>(null);
const browsingId = ref<string | null>(null);
const DEFAULT_PORTS: Record<DbDriver, number> = {
  mysql: 3306,
  postgres: 5432,
  mongodb: 27017,
  redis: 6379,
  qdrant: 6333,
};
const newRemote = ref<ConnectionProfile>({
  id: "",
  name: "",
  driver: "mysql" as DbDriver,
  host: "127.0.0.1",
  port: 3306,
  username: "root",
  password: "",
  database: null,
  created_at: "",
});

/** 可浏览的远程驱动（MySQL / PostgreSQL） */
function isBrowsable(driver: DbDriver): boolean {
  return driver === "mysql" || driver === "postgres";
}

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

const showCreateSqlite = ref(false);
const newSqliteName = ref("");

async function createSqlite() {
  if (!newSqliteName.value.trim()) {
    message.warning("请输入数据库名称");
    return;
  }
  try {
    await dbSqliteCreate(newSqliteName.value.trim());
    message.success("数据库创建成功");
    showCreateSqlite.value = false;
    newSqliteName.value = "";
    await loadSqliteDbs();
  } catch (e) {
    message.error(`创建失败：${e}`);
  }
}

/** 选择 SQLite 数据库，加载其表列表 */
async function selectSqliteDb(name: string) {
  activeDb.value = { kind: "sqlite", key: name, label: name };
  selectedTable.value = null;
  queryResult.value = null;
  sqlEditor.value = "SELECT * FROM sqlite_master WHERE type='table'";
  loadingTables.value = true;
  try {
    tables.value = await dbSqliteTables(name);
  } catch (e) {
    message.error(`加载表失败：${e}`);
  } finally {
    loadingTables.value = false;
  }
}

/** 选择远程连接，加载其表列表 */
async function selectRemoteDb(profile: ConnectionProfile) {
  activeDb.value = {
    kind: "remote",
    key: profile.id,
    label: profile.name,
    profile,
  };
  selectedTable.value = null;
  queryResult.value = null;
  sqlEditor.value = "SELECT 1";
  browsingId.value = profile.id;
  loadingTables.value = true;
  try {
    tables.value = await dbRemoteTables(profile);
  } catch (e) {
    message.error(`加载表失败：${e}`);
  } finally {
    loadingTables.value = false;
    browsingId.value = null;
  }
}

async function selectTable(table: string) {
  if (!activeDb.value || loadingQuery.value) return;
  selectedTable.value = table;
  loadingQuery.value = true;
  try {
    queryResult.value =
      activeDb.value.kind === "sqlite"
        ? await dbSqliteQueryTable(activeDb.value.key, table, 100, 0)
        : await dbRemoteQueryTable(activeDb.value.profile!, table, 100, 0);
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
    queryResult.value =
      activeDb.value.kind === "sqlite"
        ? await dbSqliteExecute(activeDb.value.key, sql)
        : await dbRemoteExecute(activeDb.value.profile!, sql);
    message.success(`执行成功，影响 ${queryResult.value.affected} 行`);
    // DDL 语句后刷新表列表，但不清空 SQL 编辑器内容
    const upper = sql.trim().toUpperCase();
    if (upper.startsWith("CREATE") || upper.startsWith("ALTER") || upper.startsWith("DROP")) {
      // 重新加载表列表而不重置编辑器
      const currentActive = activeDb.value;
      selectedTable.value = null;
      queryResult.value = null;
      loadingTables.value = true;
      try {
        tables.value =
          currentActive.kind === "sqlite"
            ? await dbSqliteTables(currentActive.key)
            : await dbRemoteTables(currentActive.profile!);
      } catch (e) {
        message.error(`刷新表列表失败：${e}`);
      } finally {
        loadingTables.value = false;
      }
    }
  } catch (e) {
    message.error(`执行失败：${e}`);
  } finally {
    loadingQuery.value = false;
  }
}

function confirmDeleteDb(name: string) {
  dialog.warning({
    title: "删除数据库",
    content: `确定删除数据库「${name}」？此操作不可恢复。`,
    positiveText: "删除",
    negativeText: "取消",
    onPositiveClick: async () => {
      try {
        await dbSqliteDelete(name);
        message.success("已删除");
        if (activeDb.value?.key === name) {
          activeDb.value = null;
          tables.value = [];
          queryResult.value = null;
          selectedTable.value = null;
        }
        await loadSqliteDbs();
      } catch (e) {
        message.error(`删除失败：${e}`);
      }
    },
  });
}

// 远程连接操作
async function addRemote() {
  const p = newRemote.value;
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
    showAddRemote.value = false;
    await loadRemoteProfiles();
    // 重置：保留上次选中的驱动类型，仅清空其余字段
    const prevDriver = p.driver;
    newRemote.value = {
      id: "", name: "", driver: prevDriver, host: "127.0.0.1",
      port: DEFAULT_PORTS[prevDriver] ?? 3306, username: "", password: "", database: null, created_at: "",
    };
  } catch (e) {
    message.error(`保存失败：${e}`);
  }
}

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

function confirmDeleteRemote(p: ConnectionProfile) {
  dialog.warning({
    title: "删除连接档案",
    content: `确定删除「${p.name}」？`,
    positiveText: "删除",
    negativeText: "取消",
    onPositiveClick: async () => {
      try {
        await dbRemoteRemove(p.id);
        message.success("已删除");
        if (activeDb.value?.key === p.id) {
          activeDb.value = null;
          tables.value = [];
          queryResult.value = null;
          selectedTable.value = null;
        }
        await loadRemoteProfiles();
      } catch (e) {
        message.error(`删除失败：${e}`);
      }
    },
  });
}

onMounted(async () => {
  await Promise.all([loadSqliteDbs(), loadRemoteProfiles()]);
  // 从设置页跳转：携带 add_db 查询参数时自动打开添加连接表单并预填
  const q = route.query;
  if (q.add_db) {
    const driver = String(q.add_db) as DbDriver;
    if (driver in DEFAULT_PORTS) {
      const port = Number(q.port) || DEFAULT_PORTS[driver];
      newRemote.value = {
        id: "",
        name: q.name ? String(q.name) : "",
        driver,
        host: q.host ? String(q.host) : "127.0.0.1",
        port,
        username: "",
        password: "",
        database: null,
        created_at: "",
      };
      showAddRemote.value = true;
    }
    // 清理查询参数，避免刷新页面重复触发
    router.replace({ path: route.path, query: {} });
  }
});
</script>

<template>
  <n-space vertical size="large">
    <!-- SQLite 管理 -->
    <n-card title="SQLite 数据库（内置）">
      <n-space vertical>
        <n-space align="center">
          <n-button type="primary" @click="showCreateSqlite = true">+ 新建数据库</n-button>
          <n-button @click="loadSqliteDbs">刷新</n-button>
        </n-space>

        <n-modal
          v-model:show="showCreateSqlite"
          preset="card"
          title="新建 SQLite 数据库"
          style="width: 400px"
        >
          <n-input
            v-model:value="newSqliteName"
            placeholder="数据库名称"
            @keydown.enter="createSqlite"
          />
          <template #footer>
            <n-space justify="end">
              <n-button @click="showCreateSqlite = false">取消</n-button>
              <n-button type="primary" @click="createSqlite">创建</n-button>
            </n-space>
          </template>
        </n-modal>

        <n-layout has-sider style="height: 400px">
          <n-layout-sider :width="200" bordered content-style="padding: 8px;">
            <n-text depth="3" style="font-size: 12px">数据库列表</n-text>
            <n-menu
              :value="activeDb?.kind === 'sqlite' ? activeDb.key : undefined"
              :options="
                sqliteDbs.map((d) => ({
                  label: d.name,
                  key: d.name,
                }))
             "
              @update:value="selectSqliteDb"
            />
          </n-layout-sider>
          <n-layout-sider v-if="activeDb" :width="180" bordered content-style="padding: 8px;">
            <n-text depth="3" style="font-size: 12px">表（{{ tables.length }}）</n-text>
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
          <n-layout content-style="padding: 12px;">
            <n-space vertical v-if="activeDb">
              <n-text depth="3" style="font-size: 12px">
                {{ activeDb.label }} → {{ selectedTable ?? "SQL 编辑器" }}
              </n-text>
              <n-input
                v-model:value="sqlEditor"
                type="textarea"
                :autosize="{ minRows: 3, maxRows: 6 }"
                :placeholder="sqlPlaceholder"
                style="font-family: monospace"
              />
              <n-space>
                <n-button type="primary" :loading="loadingQuery" @click="runSql">
                  执行 SQL
                </n-button>
                <n-button
                  v-if="activeDb.kind === 'sqlite'"
                  quaternary
                  type="error"
                  size="small"
                  @click="confirmDeleteDb(activeDb.key)"
                >
                  删除此数据库
                </n-button>
              </n-space>

              <!-- 查询结果 -->
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
                  :data="queryResult.rows.map((r) => {
                    const obj: Record<string, any> = {};
                    queryResult!.columns.forEach((c, i) => { obj[c] = r[i]; });
                    return obj;
                  })"
                  :max-height="300"
                  :bordered="true"
                  size="small"
                />
                <n-text v-else depth="3">无结果</n-text>
              </div>
            </n-space>
            <n-empty v-else description="请从左侧选择数据库" />
          </n-layout>
        </n-layout>
      </n-space>
    </n-card>

    <!-- 远程连接 -->
    <n-card title="远程数据库（MySQL / PostgreSQL / MongoDB / Redis / Qdrant）">
      <n-space vertical>
        <n-space align="center">
          <n-button type="primary" @click="showAddRemote = true">
            + 添加连接
          </n-button>
          <n-button @click="loadRemoteProfiles">刷新</n-button>
        </n-space>

        <n-data-table
          v-if="remoteProfiles.length > 0"
          :columns="[
            { title: '名称', key: 'name' },
            { title: '类型', key: 'driver' },
            { title: '主机', key: 'host' },
            { title: '端口', key: 'port' },
            { title: '数据库', key: 'database' },
            {
              title: '操作',
              key: 'actions',
              render: (row: ConnectionProfile) =>
                h('div', { style: 'display:flex; gap:8px' }, [
                  h(NButton, { size: 'small', loading: testingId === row.id, onClick: () => testRemote(row) } as Record<string, unknown>, () => '测试'),
                  h(NButton, {
                    size: 'small',
                    type: 'info',
                    loading: browsingId === row.id,
                    disabled: !isBrowsable(row.driver),
                    onClick: () => selectRemoteDb(row),
                  } as Record<string, unknown>, () => '浏览'),
                  h(NButton, { size: 'small', type: 'error', onClick: () => confirmDeleteRemote(row) }, () => '删除'),
                ]),
            },
          ]"
          :data="remoteProfiles"
          :bordered="false"
          :pagination="false"
        />
        <n-empty v-else description="暂无远程连接" />

        <!-- 添加连接模态框 -->
        <n-modal
          v-model:show="showAddRemote"
          preset="card"
          title="添加远程数据库连接"
          style="width: 500px"
        >
          <n-form label-placement="left" :label-width="70">
            <n-form-item label="名称">
              <n-input v-model:value="newRemote.name" placeholder="如：本地 MySQL" />
            </n-form-item>
            <n-form-item label="类型">
              <n-select
                v-model:value="newRemote.driver"
                :options="[
                  { label: 'MySQL / MariaDB', value: 'mysql' },
                  { label: 'PostgreSQL', value: 'postgres' },
                  { label: 'MongoDB', value: 'mongodb' },
                  { label: 'Redis', value: 'redis' },
                  { label: 'Qdrant', value: 'qdrant' },
                ]"
              />
            </n-form-item>
            <n-form-item label="主机">
              <n-input v-model:value="newRemote.host" placeholder="127.0.0.1" />
            </n-form-item>
            <n-form-item label="端口">
              <n-input-number
                v-model:value="newRemote.port"
                :min="1"
                :max="65535"
                style="width: 100%"
              />
            </n-form-item>
            <n-form-item label="用户名">
              <n-input v-model:value="newRemote.username" placeholder="root" />
            </n-form-item>
            <n-form-item label="密码">
              <n-input
                v-model:value="newRemote.password"
                type="password"
                show-password-on="click"
                placeholder="••••••"
              />
            </n-form-item>
            <n-form-item label="数据库">
              <n-input
                v-model:value="newRemote.database"
                placeholder="可选，默认数据库"
              />
            </n-form-item>
          </n-form>
          <template #footer>
            <n-space justify="end">
              <n-button @click="showAddRemote = false">取消</n-button>
              <n-button type="primary" @click="addRemote">保存</n-button>
            </n-space>
          </template>
        </n-modal>
      </n-space>
    </n-card>
  </n-space>
</template>

<script lang="ts">
export default { name: "DatabasesView" };
</script>
