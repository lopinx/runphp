<script setup lang="ts">
import { onMounted, ref, computed, h } from "vue";
import { useMessage, useDialog, NButton } from "naive-ui";
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
  type DatabaseFile,
  type TableInfo,
  type QueryResult,
  type ConnectionProfile,
  type DbDriver,
} from "../api";

const message = useMessage();
const dialog = useDialog();

// SQLite 部分
const sqliteDbs = ref<DatabaseFile[]>([]);
const selectedDb = ref<string | null>(null);
const tables = ref<TableInfo[]>([]);
const selectedTable = ref<string | null>(null);
const queryResult = ref<QueryResult | null>(null);
const sqlEditor = ref("SELECT * FROM sqlite_master WHERE type='table'");
const loadingTables = ref(false);
const loadingQuery = ref(false);

// 远程连接部分
const remoteProfiles = ref<ConnectionProfile[]>([]);
const showAddRemote = ref(false);
const testingId = ref<string | null>(null);
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

async function createSqlite() {
  const name = prompt("数据库名称：");
  if (!name) return;
  try {
    await dbSqliteCreate(name);
    message.success("数据库创建成功");
    await loadSqliteDbs();
  } catch (e) {
    message.error(`创建失败：${e}`);
  }
}

async function selectDb(name: string) {
  selectedDb.value = name;
  selectedTable.value = null;
  queryResult.value = null;
  loadingTables.value = true;
  try {
    tables.value = await dbSqliteTables(name);
  } catch (e) {
    message.error(`加载表失败：${e}`);
  } finally {
    loadingTables.value = false;
  }
}

async function selectTable(table: string) {
  if (!selectedDb.value) return;
  selectedTable.value = table;
  loadingQuery.value = true;
  try {
    queryResult.value = await dbSqliteQueryTable(selectedDb.value, table, 100, 0);
  } catch (e) {
    message.error(`查询失败：${e}`);
  } finally {
    loadingQuery.value = false;
  }
}

async function runSql() {
  if (!selectedDb.value) {
    message.warning("请先选择数据库");
    return;
  }
  if (!sqlEditor.value.trim()) {
    message.warning("请输入 SQL 语句");
    return;
  }
  loadingQuery.value = true;
  try {
    queryResult.value = await dbSqliteExecute(selectedDb.value, sqlEditor.value);
    message.success(`执行成功，影响 ${queryResult.value.affected} 行`);
    // 如果是 SELECT 则刷新表列表
    if (sqlEditor.value.trim().toUpperCase().startsWith("CREATE")) {
      await selectDb(selectedDb.value);
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
        if (selectedDb.value === name) {
          selectedDb.value = null;
          tables.value = [];
          queryResult.value = null;
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
  p.port = p.driver === "mysql" ? (p.port || 3306) : (p.port || 5432);
  try {
    await dbRemoteAdd(p);
    message.success("连接档案已保存");
    showAddRemote.value = false;
    await loadRemoteProfiles();
    // 重置
    newRemote.value = {
      id: "", name: "", driver: "mysql", host: "127.0.0.1",
      port: 3306, username: "root", password: "", database: null, created_at: "",
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
        await loadRemoteProfiles();
      } catch (e) {
        message.error(`删除失败：${e}`);
      }
    },
  });
}

onMounted(async () => {
  await Promise.all([loadSqliteDbs(), loadRemoteProfiles()]);
});
</script>

<template>
  <n-space vertical size="large">
    <!-- SQLite 管理 -->
    <n-card title="SQLite 数据库（内置）">
      <n-space vertical>
        <n-space align="center">
          <n-button type="primary" @click="createSqlite">+ 新建数据库</n-button>
          <n-button @click="loadSqliteDbs">刷新</n-button>
        </n-space>

        <n-layout has-sider style="height: 400px">
          <n-layout-sider :width="200" bordered content-style="padding: 8px;">
            <n-text depth="3" style="font-size: 12px">数据库列表</n-text>
            <n-menu
              :value="selectedDb ?? undefined"
              :options="
                sqliteDbs.map((d) => ({
                  label: d.name,
                  key: d.name,
                }))
             "
              @update:value="selectDb"
            />
          </n-layout-sider>
          <n-layout-sider v-if="selectedDb" :width="180" bordered content-style="padding: 8px;">
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
            <n-space vertical v-if="selectedDb">
              <n-text depth="3" style="font-size: 12px">
                {{ selectedDb }} → {{ selectedTable ?? "SQL 编辑器" }}
              </n-text>
              <n-input
                v-model:value="sqlEditor"
                type="textarea"
                :autosize="{ minRows: 3, maxRows: 6 }"
                placeholder="输入 SQL 语句…"
                style="font-family: monospace"
              />
              <n-space>
                <n-button type="primary" :loading="loadingQuery" @click="runSql">
                  执行 SQL
                </n-button>
                <n-button
                  v-if="selectedDb"
                  quaternary
                  type="error"
                  size="small"
                  @click="confirmDeleteDb(selectedDb)"
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
                      render: (_: any, i: number) =>
                        String(queryResult!.rows[i]?.[queryResult!.columns.indexOf(c)] ?? ''),
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
    <n-card title="远程数据库（MySQL / PostgreSQL）">
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
