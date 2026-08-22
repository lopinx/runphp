<script setup lang="ts">
import { onMounted, ref, h } from "vue";
import { useMessage, useDialog, NButton, NTag, NSwitch } from "naive-ui";
import {
  ftpServerStatus,
  ftpServerStart,
  ftpServerStop,
  ftpServerConfig,
  ftpServerUpdateConfig,
  ftpServerBackend,
  ftpUserList,
  ftpUserAdd,
  ftpUserUpdate,
  ftpUserRemove,
  siteList,
  type FtpServerUser,
  type FtpdConfig,
  type Site,
} from "../api";

const message = useMessage();
const dialog = useDialog();

const running = ref(false);
const backend = ref("");
const config = ref<FtpdConfig>({ port: 21, passive_from: 50000, passive_to: 50010, autostart: false });
const users = ref<FtpServerUser[]>([]);
const sites = ref<Site[]>([]);
const toggling = ref(false);

async function loadState() {
  try {
    [running.value, backend.value, config.value, users.value] = await Promise.all([
      ftpServerStatus(),
      ftpServerBackend(),
      ftpServerConfig(),
      ftpUserList(),
    ]);
  } catch (e) {
    message.error(`加载 FTP 服务状态失败：${e}`);
  }
}

async function startServer() {
  toggling.value = true;
  try {
    const used = await ftpServerStart();
    message.success(`FTP 服务已启动（后端：${used}）`);
  } catch (e) {
    message.error(`${e}`);
  } finally {
    toggling.value = false;
    await loadState();
  }
}

async function stopServer() {
  toggling.value = true;
  try {
    await ftpServerStop();
    message.success("FTP 服务已停止");
  } catch (e) {
    message.error(`${e}`);
  } finally {
    toggling.value = false;
    await loadState();
  }
}

async function saveConfig() {
  try {
    await ftpServerUpdateConfig(config.value);
    message.success("配置已保存（运行中的服务需重启后生效）");
  } catch (e) {
    message.error(`保存失败：${e}`);
  }
}

// ---- 用户 CRUD ----
const showUser = ref(false);
const editing = ref<FtpServerUser | null>(null);
const userForm = ref<FtpServerUser>(blankUser());

function blankUser(): FtpServerUser {
  return {
    id: "",
    username: "",
    password: "",
    home_dir: null,
    linked_site: null,
    enabled: true,
    created_at: "",
  };
}

function openAdd() {
  editing.value = null;
  userForm.value = blankUser();
  showUser.value = true;
}

function openEdit(u: FtpServerUser) {
  editing.value = u;
  userForm.value = { ...u };
  showUser.value = true;
}

async function submitUser() {
  const u = userForm.value;
  if (!u.username.trim() || !u.password) {
    message.warning("请填写用户名和密码");
    return;
  }
  try {
    if (editing.value) {
      await ftpUserUpdate(u);
      message.success("用户已更新");
    } else {
      u.id = crypto.randomUUID();
      u.created_at = new Date().toISOString();
      await ftpUserAdd(u);
      message.success("用户已添加");
    }
    showUser.value = false;
    await loadState();
  } catch (e) {
    message.error(`保存失败：${e}`);
  }
}

async function toggleEnabled(u: FtpServerUser) {
  try {
    await ftpUserUpdate({ ...u, enabled: !u.enabled });
    await loadState();
  } catch (e) {
    message.error(`${e}`);
  }
}

function removeUser(u: FtpServerUser) {
  dialog.warning({
    title: "删除用户",
    content: `确定删除 FTP 用户「${u.username}」？`,
    positiveText: "删除",
    negativeText: "取消",
    onPositiveClick: async () => {
      try {
        await ftpUserRemove(u.id);
        message.success("已删除");
        await loadState();
      } catch (e) {
        message.error(`删除失败：${e}`);
      }
    },
  });
}

function siteName(id: string | null): string {
  if (!id) return "—";
  return sites.value.find((s) => s.id === id)?.name ?? id;
}

onMounted(async () => {
  await loadState();
  try {
    sites.value = await siteList();
  } catch {
    // 站点列表加载失败不阻断用户管理
  }
});
</script>

<template>
  <n-space vertical size="large">
    <!-- 服务状态与配置 -->
    <n-card title="FTP 服务端" size="small">
      <template #header-extra>
        <n-space align="center">
          <n-tag :type="backend === 'Pure-FTPd' ? 'success' : 'info'" size="small" :bordered="false">
            后端：{{ backend }}
          </n-tag>
          <n-tag :type="running ? 'success' : 'default'" size="small">
            {{ running ? "运行中" : "已停止" }}
          </n-tag>
        </n-space>
      </template>
      <n-space align="center" justify="space-between">
        <n-space align="center">
          <n-button
            v-if="!running"
            type="primary"
            size="small"
            :loading="toggling"
            @click="startServer"
          >
            启动服务
          </n-button>
          <n-button v-else size="small" :loading="toggling" @click="stopServer">
            停止服务
          </n-button>
          <n-button size="small" @click="loadState">刷新状态</n-button>
        </n-space>
        <n-text depth="3" style="font-size: 12px">
          Linux 检测到 Pure-FTPd 时自动对接；否则使用内嵌 FTP 服务端（Windows 默认）。
        </n-text>
      </n-space>

      <n-divider style="margin: 12px 0" />
      <n-form inline label-placement="left" :label-width="80" size="small">
        <n-form-item label="控制端口">
          <n-input-number v-model:value="config.port" :min="1" :max="65535" />
        </n-form-item>
        <n-form-item label="被动端口">
          <n-space :wrap="false" align="center">
            <n-input-number v-model:value="config.passive_from" :min="1024" :max="65535" />
            <span>—</span>
            <n-input-number v-model:value="config.passive_to" :min="1024" :max="65535" />
          </n-space>
        </n-form-item>
        <n-form-item label="随应用自启">
          <n-switch v-model:value="config.autostart" />
        </n-form-item>
        <n-form-item label=" ">
          <n-button type="primary" size="small" @click="saveConfig">保存配置</n-button>
        </n-form-item>
      </n-form>
    </n-card>

    <!-- 虚拟用户 -->
    <n-card title="虚拟用户" size="small">
      <template #header-extra>
        <n-button size="small" type="primary" @click="openAdd">+ 添加用户</n-button>
      </template>
      <n-data-table
        :columns="[
          { title: '用户名', key: 'username' },
          {
            title: '根目录',
            key: 'home_dir',
            render: (u: FtpServerUser) => u.home_dir ?? `默认 ftp/${u.username}`,
          },
          {
            title: '关联站点',
            key: 'linked_site',
            render: (u: FtpServerUser) => siteName(u.linked_site),
          },
          {
            title: '启用',
            key: 'enabled',
            width: 80,
            render: (u: FtpServerUser) =>
              h(NSwitch, {
                size: 'small',
                value: u.enabled,
                onUpdateValue: () => toggleEnabled(u),
              } as Record<string, unknown>),
          },
          {
            title: '操作',
            key: 'actions',
            width: 150,
            render: (u: FtpServerUser) =>
              h('div', { style: 'display:flex; gap:6px' }, [
                h(NButton, { size: 'small', onClick: () => openEdit(u) } as Record<string, unknown>, () => '编辑'),
                h(NButton, { size: 'small', type: 'error', onClick: () => removeUser(u) } as Record<string, unknown>, () => '删除'),
              ]),
          },
        ]"
        :data="users"
        :bordered="false"
        :pagination="false"
      />
      <n-empty
        v-if="users.length === 0"
        description="尚无虚拟用户：添加用户后即可用 FTP 客户端连接本机端口访问其根目录"
        style="padding: 24px 0"
      />
    </n-card>

    <!-- 用户编辑模态框 -->
    <n-modal v-model:show="showUser" preset="card" :title="editing ? '编辑用户' : '添加用户'" style="width: 480px">
      <n-form label-placement="left" :label-width="90">
        <n-form-item label="用户名">
          <n-input v-model:value="userForm.username" placeholder="字母/数字/下划线/连字符" :disabled="!!editing" />
        </n-form-item>
        <n-form-item label="密码">
          <n-input v-model:value="userForm.password" type="password" show-password-on="click" />
        </n-form-item>
        <n-form-item label="根目录">
          <n-input
            v-model:value="userForm.home_dir"
            placeholder="留空使用默认 数据目录/ftp/<用户名>"
          />
        </n-form-item>
        <n-form-item label="关联站点">
          <n-select
            v-model:value="userForm.linked_site"
            :options="sites.map((s) => ({ label: s.name, value: s.id }))"
            placeholder="可选"
            clearable
          />
        </n-form-item>
        <n-form-item label="启用">
          <n-switch v-model:value="userForm.enabled" />
        </n-form-item>
      </n-form>
      <n-text depth="3" style="font-size: 12px">
        每个用户都被锁定（chroot）在自己的根目录内。内嵌后端的根目录须位于数据目录下。
      </n-text>
      <template #footer>
        <n-space justify="end">
          <n-button @click="showUser = false">取消</n-button>
          <n-button type="primary" @click="submitUser">{{ editing ? "保存" : "添加" }}</n-button>
        </n-space>
      </template>
    </n-modal>
  </n-space>
</template>

<script lang="ts">
export default { name: "FtpServerPanel" };
</script>
