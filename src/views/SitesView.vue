<script setup lang="ts">
import { onMounted, ref, computed } from "vue";
import { useAppStore } from "../stores/app";
import type { Site, WorkerConfig } from "../api";
import { useMessage, useDialog } from "naive-ui";

const store = useAppStore();
const message = useMessage();
const dialog = useDialog();

// 编辑模态框状态
const showEdit = ref(false);
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
  showEdit.value = true;
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
  showEdit.value = true;
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

  const site: Site = {
    id: editing.value?.id ?? crypto.randomUUID(),
    name: formName.value.trim(),
    domains,
    port: formPort.value,
    root: formRoot.value.trim(),
    https: formHttps.value,
    worker,
    runtime_version: editing.value?.runtime_version ?? "",
    php_ini: editing.value?.php_ini ?? [],
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

import { h } from "vue";
import { NButton } from "naive-ui";
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
          <n-input
            v-model:value="formRoot"
            placeholder="网站根目录绝对路径"
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
