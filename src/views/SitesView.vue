<script setup lang="ts">
import { onMounted, ref, computed, h } from "vue";
import { useAppStore } from "../stores/app";
import type { Site, WorkerConfig, SiteDbBinding } from "../api";
import { dbServiceList, dbServiceDatabases } from "../api";
import { useMessage, useDialog, NButton } from "naive-ui";
import DirectoryPicker from "../components/DirectoryPicker.vue";

const store = useAppStore();
const message = useMessage();
const dialog = useDialog();

// 编辑模态框状态
const showEdit = ref(false);
const showRootPicker = ref(false);
const editing = ref<Site | null>(null);
const saving = ref(false);

// 表单字段
const formName = ref("");
const formDomains = ref("");
const formPort = ref(0);
const formRoot = ref("");
const formHttps = ref(false);
const formWorkerEnabled = ref(false);
const formWorkerScript = ref("public/index.php");
const formWorkerNum = ref(4);
const formDatabases = ref<string[]>([]);

// 受管数据库选项（服务 × 数据库）
interface DbOption {
  value: string;
  label: string;
  binding: SiteDbBinding;
}
const dbOptions = ref<DbOption[]>([]);
const dbServiceNames = ref<Record<string, string>>({});

/** 拉取受管数据库服务及其数据库清单，生成「服务名 / 库名」选项 */
async function loadDbOptions() {
  try {
    const services = (await dbServiceList()).filter(
      (s) => s.kind === "mysql" || s.kind === "mariadb" || s.kind === "postgresql",
    );
    for (const s of services) {
      dbServiceNames.value[s.id] = s.name;
    }
    const lists = await Promise.all(
      services.map(async (s) => ({ s, dbs: await dbServiceDatabases(s.id).catch(() => [] as string[]) })),
    );
    dbOptions.value = lists.flatMap(({ s, dbs }) =>
      dbs.map((d) => ({
        value: `${s.id}:${d}`,
        label: `${s.name} / ${d}`,
        binding: { service_id: s.id, database: d },
      })),
    );
  } catch {
    dbOptions.value = [];
  }
}

const isEdit = computed(() => !!editing.value);

onMounted(async () => {
  await store.refreshSites();
});

function openCreate() {
  editing.value = null;
  formName.value = "";
  formDomains.value = "";
  formPort.value = 0;
  formRoot.value = "";
  formHttps.value = false;
  formWorkerEnabled.value = false;
  formWorkerScript.value = "public/index.php";
  formWorkerNum.value = 4;
  formDatabases.value = [];
  showEdit.value = true;
  void loadDbOptions();
}

function openEdit(site: Site) {
  editing.value = site;
  formName.value = site.name;
  formDomains.value = site.domains.join(", ");
  formPort.value = site.port;
  formRoot.value = site.root;
  formHttps.value = site.https;
  formWorkerEnabled.value = !!site.worker;
  formWorkerScript.value = site.worker?.script ?? "public/index.php";
  formWorkerNum.value = site.worker?.num ?? 4;
  formDatabases.value = (site.databases ?? []).map((b) => `${b.service_id}:${b.database}`);
  showEdit.value = true;
  void loadDbOptions();
}

async function save() {
  if (!formName.value.trim()) {
    message.warning("请填写站点名称");
    return;
  }
  if (!formDomains.value.trim()) {
    message.warning("请填写至少一个域名");
    return;
  }
  if (!formRoot.value.trim()) {
    message.warning("请填写网站根目录");
    return;
  }

  const domains = formDomains.value
    .split(",")
    .map((s) => s.trim())
    .filter(Boolean);

  const worker: WorkerConfig | null = formWorkerEnabled.value
    ? { script: formWorkerScript.value, num: formWorkerNum.value }
    : null;

  // 选项 value 反查绑定结构（服务 id + 库名）
  const databases: SiteDbBinding[] = formDatabases.value
    .map((v) => dbOptions.value.find((o) => o.value === v)?.binding)
    .filter((b): b is SiteDbBinding => !!b);
  // 编辑时保留已失效（服务被删）的历史绑定提示不了太多，直接以当前选择为准

  const site: Site = {
    id: editing.value?.id ?? crypto.randomUUID(),
    name: formName.value.trim(),
    domains,
    port: formPort.value,
    root: formRoot.value.trim(),
    https: formHttps.value,
    worker,
    php_ini: editing.value?.php_ini ?? [],
    databases,
    created_at: editing.value?.created_at ?? new Date().toISOString(),
    updated_at: new Date().toISOString(),
  };

  saving.value = true;
  try {
    if (isEdit.value) {
      await store.updateSite(site);
      message.success("站点已更新");
    } else {
      await store.createSite(site);
      message.success("站点已创建");
    }
    showEdit.value = false;
  } catch (e) {
    message.error(`保存失败：${e}`);
  } finally {
    saving.value = false;
  }
}

function confirmDelete(site: Site) {
  dialog.warning({
    title: "删除站点",
    content: `确定要删除站点「${site.name}」吗？此操作不可恢复。`,
    positiveText: "删除",
    negativeText: "取消",
    onPositiveClick: async () => {
      try {
        await store.deleteSite(site.id);
        message.success("站点已删除");
      } catch (e) {
        message.error(`删除失败：${e}`);
      }
    },
  });
}

const columns = computed(() => [
  { title: "名称", key: "name" },
  { title: "域名", key: "domains", render: (row: Site) => row.domains.join(", ") },
  {
    title: "端口",
    key: "port",
    render: (row: Site) => (row.port ? String(row.port) : "自动"),
  },
  {
    title: "HTTPS",
    key: "https",
    render: (row: Site) =>
      row.https ? "✅ 开启" : "—",
  },
  {
    title: "Worker",
    key: "worker",
    render: (row: Site) =>
      row.worker ? `${row.worker.script} ×${row.worker.num}` : "—",
  },
  {
    title: "数据库",
    key: "databases",
    render: (row: Site) =>
      row.databases?.length
        ? row.databases
            .map((b) => `${dbServiceNames.value[b.service_id] ?? b.service_id}/${b.database}`)
            .join(", ")
        : "—",
  },
  {
    title: "操作",
    key: "actions",
    render: (row: Site) =>
      h("div", { style: "display:flex; gap:8px" }, [
        h(NButton, { size: "small", onClick: () => openEdit(row) }, () => "编辑"),
        h(
          NButton,
          { size: "small", type: "error", onClick: () => confirmDelete(row) },
          () => "删除",
        ),
      ]),
  },
]);
</script>

<template>
  <n-space vertical size="large">
    <n-space justify="space-between" align="center">
      <n-text strong style="font-size: 16px">站点列表</n-text>
      <n-button type="primary" @click="openCreate">+ 新建站点</n-button>
    </n-space>

    <n-data-table
      :columns="columns"
      :data="store.sites"
      :bordered="false"
      :pagination="false"
    />

    <!-- 新建/编辑模态框 -->
    <n-modal
      v-model:show="showEdit"
      preset="card"
      :title="isEdit ? '编辑站点' : '新建站点'"
      style="width: 600px"
    >
      <n-form label-placement="left" :label-width="90">
        <n-form-item label="名称">
          <n-input v-model:value="formName" placeholder="如：我的博客" />
        </n-form-item>
        <n-form-item label="域名">
          <n-input
            v-model:value="formDomains"
            placeholder="多个域名用逗号分隔，如：blog.test, www.blog.test"
          />
        </n-form-item>
        <n-form-item label="根目录">
          <div style="display: flex; gap: 8px; width: 100%">
            <n-input
              v-model:value="formRoot"
              placeholder="网站根目录绝对路径，可点「浏览」选择"
              style="flex: 1"
            />
            <n-button @click="showRootPicker = true">浏览</n-button>
          </div>
          <DirectoryPicker
            v-model:show="showRootPicker"
            @select="(p: string) => (formRoot = p)"
          />
        </n-form-item>
        <n-form-item label="端口">
          <n-input-number
            v-model:value="formPort"
            :min="0"
            :max="65535"
            placeholder="0 表示由 Caddy 自动分配"
            style="width: 100%"
          />
        </n-form-item>
        <n-form-item label="HTTPS">
          <n-switch v-model:value="formHttps">
            <template #checked>开启</template>
            <template #unchecked>关闭</template>
          </n-switch>
          <n-text depth="3" style="margin-left: 12px">
            使用 Caddy 内置本地 CA，自动签发证书
          </n-text>
        </n-form-item>
        <n-form-item label="Worker 模式">
          <n-switch v-model:value="formWorkerEnabled">
            <template #checked>启用</template>
            <template #unchecked>禁用</template>
          </n-switch>
        </n-form-item>
        <n-form-item v-if="formWorkerEnabled" label="入口脚本">
          <n-input
            v-model:value="formWorkerScript"
            placeholder="如 public/index.php"
          />
        </n-form-item>
        <n-form-item v-if="formWorkerEnabled" label="进程数">
          <n-input-number
            v-model:value="formWorkerNum"
            :min="1"
            :max="64"
            style="width: 100%"
          />
        </n-form-item>
        <n-form-item label="关联数据库">
          <n-select
            v-model:value="formDatabases"
            :options="dbOptions"
            multiple
            clearable
            placeholder="选择受管数据库服务中的库（可选）"
          />
          <n-text depth="3" style="font-size: 12px; width: 100%">
            选项来自「数据库 · 服务」页管理的服务，需服务已在运行
          </n-text>
        </n-form-item>
      </n-form>

      <template #footer>
        <n-space justify="end">
          <n-button @click="showEdit = false">取消</n-button>
          <n-button type="primary" :loading="saving" @click="save">
            {{ isEdit ? "保存" : "创建" }}
          </n-button>
        </n-space>
      </template>
    </n-modal>
  </n-space>
</template>
