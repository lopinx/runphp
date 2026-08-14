<script setup lang="ts">
import { onMounted, ref } from "vue";
import { useAppStore } from "../stores/app";
import { useMessage } from "naive-ui";
import {
  hostsList,
  hostsWritable,
  hostsSync,
  hostsContent,
  hostsElevation,
  type HostEntry,
} from "../api";

const store = useAppStore();
const message = useMessage();

const entries = ref<HostEntry[]>([]);
const writable = ref(false);
const content = ref("");
const elevationCmd = ref("");
const loading = ref(false);
const syncing = ref(false);

async function load() {
  loading.value = true;
  try {
    await store.refreshSites();
    const [list, w, c, e] = await Promise.all([
      hostsList(),
      hostsWritable(),
      hostsContent(),
      hostsElevation(),
    ]);
    entries.value = list;
    writable.value = w;
    content.value = c;
    elevationCmd.value = e;
  } catch (err) {
    message.error(`加载失败：${err}`);
  } finally {
    loading.value = false;
  }
}

async function sync() {
  if (!writable.value) {
    message.warning("无写入权限，请复制下方提权命令以管理员身份执行");
    return;
  }
  syncing.value = true;
  try {
    const count = await hostsSync();
    message.success(`已同步 ${count} 条 hosts 记录`);
    await load();
  } catch (err) {
    message.error(`同步失败：${err}`);
  } finally {
    syncing.value = false;
  }
}

async function copyElevation() {
  try {
    await navigator.clipboard.writeText(elevationCmd.value);
    message.success("提权命令已复制到剪贴板");
  } catch {
    message.error("复制失败，请手动选择命令复制");
  }
}

// 预期同步条目（从站点列表推导）
import { computed } from "vue";
const expectedEntries = computed(() => {
  const list: { ip: string; host: string; site: string }[] = [];
  for (const s of store.sites) {
    for (const d of s.domains) {
      list.push({ ip: "127.0.0.1", host: d, site: s.name });
    }
  }
  return list;
});

onMounted(load);
</script>

<template>
  <n-space vertical size="large">
    <n-card title="Hosts 写入权限">
      <n-space align="center">
        <n-tag :type="writable ? 'success' : 'warning'" size="large" round>
          {{ writable ? "✅ 可直接写入" : "⚠️ 需要管理员权限" }}
        </n-tag>
        <n-button type="primary" :loading="syncing" :disabled="loading" @click="sync">
          同步站点域名
        </n-button>
        <n-button @click="load">刷新</n-button>
      </n-space>

      <n-alert
        v-if="!writable"
        type="warning"
        title="无写入权限"
        style="margin-top: 12px"
      >
        <n-space vertical>
          <span>系统 hosts 文件需要管理员权限才能写入。请复制以下命令到管理员终端执行：</span>
          <n-input
            :value="elevationCmd"
            type="textarea"
            :autosize="{ minRows: 2 }"
            readonly
          />
          <n-button size="small" @click="copyElevation">复制命令</n-button>
        </n-space>
      </n-alert>
    </n-card>

    <n-card title="受管区块条目">
      <n-data-table
        v-if="entries.length > 0"
        :columns="[
          { title: 'IP', key: 'ip' },
          { title: '主机名', key: 'host' },
          { title: '注释', key: 'comment' },
        ]"
        :data="entries"
        :bordered="false"
        :pagination="false"
      />
      <n-empty v-else description="受管区块为空，点击「同步站点域名」写入" />
    </n-card>

    <n-card title="预期同步内容（来自站点配置）">
      <n-data-table
        v-if="expectedEntries.length > 0"
        :columns="[
          { title: 'IP', key: 'ip' },
          { title: '主机名', key: 'host' },
          { title: '来源站点', key: 'site' },
        ]"
        :data="expectedEntries"
        :bordered="false"
        :pagination="false"
      />
      <n-empty v-else description="暂无站点" />
    </n-card>

    <n-card title="系统 hosts 全文（只读）">
      <n-input
        :value="content"
        type="textarea"
        :autosize="{ minRows: 5, maxRows: 20 }"
        readonly
      />
    </n-card>
  </n-space>
</template>
