<script setup lang="ts">
import { onMounted, ref, computed, h, onUnmounted } from "vue";
import { useMessage, useDialog, NButton } from "naive-ui";
import DirectoryPicker from "../components/DirectoryPicker.vue";
import {
  ftpList,
  ftpAdd,
  ftpUpdate,
  ftpRemove,
  ftpTest,
  ftpListDir,
  ftpUpload,
  ftpUploadDir,
  ftpDownload,
  ftpDelete,
  ftpMkdir,
  ftpRename,
  onFtpProgress,
  type FtpProfile,
  type FtpProtocol,
  type FtpEntry,
} from "../api";

const message = useMessage();
const dialog = useDialog();

const profiles = ref<FtpProfile[]>([]);
const activeProfile = ref<FtpProfile | null>(null);
const currentPath = ref("/");
const entries = ref<FtpEntry[]>([]);
const loadingDir = ref(false);
const testingId = ref<string | null>(null);

/** 协议默认端口 */
const DEFAULT_PORTS: Record<FtpProtocol, number> = {
  ftp: 21,
  sftp: 22,
  ftps: 21,
};

/** 协议选项 */
const protocolOptions = [
  { label: "FTP（明文）", value: "ftp" },
  { label: "SFTP（基于 SSH）", value: "sftp" },
  { label: "FTPS（FTP over TLS）", value: "ftps" },
];

onMounted(async () => {
  await loadProfiles();
});

async function loadProfiles() {
  try {
    profiles.value = await ftpList();
    if (profiles.value.length > 0 && !activeProfile.value) {
      await selectProfile(profiles.value[0]);
    }
  } catch (e) {
    message.error(`加载连接档案失败：${e}`);
  }
}

async function selectProfile(p: FtpProfile) {
  activeProfile.value = p;
  currentPath.value = "/";
  await loadDir();
}

function blankProfile(protocol: FtpProtocol = "ftp"): FtpProfile {
  return {
    id: "",
    name: "",
    protocol,
    host: "127.0.0.1",
    port: DEFAULT_PORTS[protocol],
    username: "",
    password: "",
    ssh_key: null,
    ssh_password: null,
    created_at: "",
  };
}

// ---- 添加/编辑/测试/删除 ----
const showAdd = ref(false);
const addForm = ref<FtpProfile>(blankProfile());
const editing = ref<FtpProfile | null>(null);
const isEdit = computed(() => !!editing.value);

function openAdd() {
  addForm.value = blankProfile();
  editing.value = null;
  showAdd.value = true;
}

function openEdit(p: FtpProfile) {
  addForm.value = { ...p };
  editing.value = p;
  showAdd.value = true;
}

function onProtocolChange(v: FtpProtocol) {
  addForm.value.port = DEFAULT_PORTS[v];
}

async function submitProfile() {
  const p = addForm.value;
  if (!p.name.trim() || !p.host.trim()) {
    message.warning("请填写名称和主机");
    return;
  }
  try {
    if (isEdit.value) {
      await ftpUpdate(p);
      message.success("连接档案已更新");
    } else {
      p.id = crypto.randomUUID();
      p.created_at = new Date().toISOString();
      await ftpAdd(p);
      message.success("连接档案已保存");
    }
    showAdd.value = false;
    await loadProfiles();
  } catch (e) {
    message.error(`保存失败：${e}`);
  }
}

async function testActive() {
  if (!activeProfile.value) return;
  testingId.value = activeProfile.value.id;
  try {
    const msg = await ftpTest(activeProfile.value);
    message.success(msg);
  } catch (e) {
    message.error(`连接失败：${e}`);
  } finally {
    testingId.value = null;
  }
}

async function removeProfile(p: FtpProfile) {
  dialog.warning({
    title: "确认删除",
    content: `确定删除连接「${p.name}」吗？`,
    positiveText: "删除",
    negativeText: "取消",
    onPositiveClick: async () => {
      try {
        await ftpRemove(p.id);
        message.success("已删除");
        if (activeProfile.value?.id === p.id) {
          activeProfile.value = null;
          entries.value = [];
        }
        await loadProfiles();
      } catch (e) {
        message.error(`删除失败：${e}`);
      }
    },
  });
}

// ---- 文件浏览 ----
async function loadDir() {
  if (!activeProfile.value) return;
  loadingDir.value = true;
  try {
    entries.value = await ftpListDir(activeProfile.value, currentPath.value);
  } catch (e) {
    message.error(`列目录失败：${e}`);
    entries.value = [];
  } finally {
    loadingDir.value = false;
  }
}

/** 进入子目录 */
function enterDir(name: string) {
  const base = currentPath.value.endsWith("/")
    ? currentPath.value
    : currentPath.value + "/";
  currentPath.value = (base + name).replace(/\/+/g, "/");
  void loadDir();
}

/** 返回上级 */
function goUp() {
  const parts = currentPath.value.split("/").filter(Boolean);
  parts.pop();
  currentPath.value = "/" + parts.join("/");
  if (!currentPath.value.endsWith("/")) currentPath.value += "/";
  void loadDir();
}

/** 路径面包屑分段 */
const breadcrumbs = computed(() => {
  const parts = currentPath.value.split("/").filter(Boolean);
  return parts.map((name, i) => ({
    name,
    path: "/" + parts.slice(0, i + 1).join("/"),
  }));
});

function jumpTo(path: string) {
  currentPath.value = path.endsWith("/") ? path : path + "/";
  void loadDir();
}

function fmtSize(n: number): string {
  if (n === 0) return "—";
  const units = ["B", "KB", "MB", "GB", "TB"];
  let v = n;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i += 1;
  }
  return `${v >= 100 || i === 0 ? Math.round(v) : v.toFixed(1)} ${units[i]}`;
}

function fmtTime(s: string): string {
  if (!s) return "—";
  try {
    return new Date(s).toLocaleString("zh-CN");
  } catch {
    return s;
  }
}

// ---- 新建文件夹 / 重命名 ----
const showMkdir = ref(false);
const mkdirName = ref("");

function openMkdir() {
  mkdirName.value = "";
  showMkdir.value = true;
}

async function submitMkdir() {
  if (!activeProfile.value || !mkdirName.value.trim()) return;
  const base = currentPath.value.endsWith("/")
    ? currentPath.value
    : currentPath.value + "/";
  const path = (base + mkdirName.value.trim()).replace(/\/+/g, "/");
  try {
    await ftpMkdir(activeProfile.value, path);
    message.success("文件夹已创建");
    showMkdir.value = false;
    await loadDir();
  } catch (e) {
    message.error(`创建失败：${e}`);
  }
}

function promptRename(row: FtpEntry) {
  if (!activeProfile.value) return;
  const base = currentPath.value.endsWith("/")
    ? currentPath.value
    : currentPath.value + "/";
  const from = (base + row.name).replace(/\/+/g, "/");
  let newName = row.name;
  dialog.warning({
    title: "重命名",
    content: () =>
      h("input", {
        value: newName,
        onInput: (e: Event) => {
          newName = (e.target as HTMLInputElement).value;
        },
        style: "width:100%; padding:4px 8px;",
        placeholder: "新名称",
      }),
    positiveText: "确认",
    negativeText: "取消",
    onPositiveClick: async () => {
      if (!newName.trim() || newName === row.name) return;
      const to = (base + newName.trim()).replace(/\/+/g, "/");
      try {
        await ftpRename(activeProfile.value!, from, to);
        message.success("已重命名");
        await loadDir();
      } catch (e) {
        message.error(`重命名失败：${e}`);
      }
    },
  });
}

async function deleteEntry(row: FtpEntry) {
  if (!activeProfile.value) return;
  const base = currentPath.value.endsWith("/")
    ? currentPath.value
    : currentPath.value + "/";
  const path = (base + row.name).replace(/\/+/g, "/");
  dialog.warning({
    title: "确认删除",
    content: `确定删除${row.is_dir ? "文件夹" : "文件"}「${row.name}」吗？${
      row.is_dir ? "文件夹内的内容也会被删除。" : ""
    }`,
    positiveText: "删除",
    negativeText: "取消",
    onPositiveClick: async () => {
      try {
        await ftpDelete(activeProfile.value!, path, row.is_dir);
        message.success("已删除");
        await loadDir();
      } catch (e) {
        message.error(`删除失败：${e}`);
      }
    },
  });
}

// ---- 上传/下载（本地路径选择 + 进度） ----
const showLocalPicker = ref(false);
const pickerMode = ref<"upload" | "uploadDir" | "download">("upload");
const pickerFileMode = computed(() => pickerMode.value !== "download");
const pendingRemotePath = ref<string>("");

/** 进度状态：active 为 false 时不显示进度条 */
const progress = ref({
  active: false,
  file: "",
  transferred: 0,
  total: 0,
});
const progressPct = computed(() =>
  progress.value.total > 0
    ? Math.min(100, Math.round((progress.value.transferred / progress.value.total) * 100))
    : 0,
);
let unlistenProgress: (() => void) | null = null;

onUnmounted(() => {
  unlistenProgress?.();
});

function openUpload() {
  pickerMode.value = "upload";
  showLocalPicker.value = true;
}

function openUploadDir() {
  pickerMode.value = "uploadDir";
  showLocalPicker.value = true;
}

function onPickLocal(localPath: string) {
  showLocalPicker.value = false;
  if (!activeProfile.value) return;
  const base = currentPath.value.endsWith("/")
    ? currentPath.value
    : currentPath.value + "/";
  if (pickerMode.value === "upload") {
    const fileName = localPath.split(/[\\/]/).pop() ?? "upload.bin";
    const remotePath = (base + fileName).replace(/\/+/g, "/");
    void doUpload(localPath, remotePath);
  } else if (pickerMode.value === "uploadDir") {
    const dirName = localPath.split(/[\\/]/).pop() ?? "upload_dir";
    const remoteDir = (base + dirName).replace(/\/+/g, "/");
    void doUploadDir(localPath, remoteDir);
  } else {
    void doDownload(pendingRemotePath.value, localPath);
  }
}

async function startProgressListen(event: "upload" | "download") {
  unlistenProgress?.();
  unlistenProgress = await onFtpProgress(event, (p) => {
    progress.value = {
      active: true,
      file: p.file,
      transferred: p.transferred,
      total: p.total,
    };
  });
}

function stopProgressListen() {
  unlistenProgress?.();
  unlistenProgress = null;
  progress.value.active = false;
}

async function doUpload(localPath: string, remotePath: string) {
  if (!activeProfile.value) return;
  progress.value = { active: true, file: "", transferred: 0, total: 0 };
  await startProgressListen("upload");
  try {
    await ftpUpload(activeProfile.value, localPath, remotePath);
    message.success("上传成功");
    await loadDir();
  } catch (e) {
    message.error(`上传失败：${e}`);
  } finally {
    stopProgressListen();
  }
}

async function doUploadDir(localDir: string, remoteDir: string) {
  if (!activeProfile.value) return;
  progress.value = { active: true, file: "", transferred: 0, total: 0 };
  await startProgressListen("upload");
  try {
    await ftpUploadDir(activeProfile.value, localDir, remoteDir);
    message.success("文件夹上传完成");
    await loadDir();
  } catch (e) {
    message.error(`上传失败：${e}`);
  } finally {
    stopProgressListen();
  }
}

function startDownload(row: FtpEntry) {
  if (!activeProfile.value) return;
  const base = currentPath.value.endsWith("/")
    ? currentPath.value
    : currentPath.value + "/";
  pendingRemotePath.value = (base + row.name).replace(/\/+/g, "/");
  pickerMode.value = "download";
  showLocalPicker.value = true;
}

async function doDownload(remotePath: string, localPath: string) {
  if (!activeProfile.value) return;
  progress.value = { active: true, file: "", transferred: 0, total: 0 };
  await startProgressListen("download");
  try {
    const fileName = remotePath.split("/").pop() ?? "download.bin";
    const localFile = localPath.endsWith("\\") || localPath.endsWith("/")
      ? localPath + fileName
      : localPath + "\\" + fileName;
    await ftpDownload(activeProfile.value, remotePath, localFile);
    message.success("下载成功");
  } catch (e) {
    message.error(`下载失败：${e}`);
  } finally {
    stopProgressListen();
  }
}
</script>

<template>
  <n-space vertical size="large">
    <n-card title="FTP 文件管理">
      <!-- 顶部工具栏 -->
      <n-space align="center" style="margin-bottom: 12px">
        <n-select
          v-model:value="activeProfile"
          :options="
            profiles.map((p) => ({
              label: `${p.name} (${p.protocol.toUpperCase()})`,
              value: p,
            }))
          "
          style="width: 280px"
          placeholder="选择连接"
          @update:value="(p: FtpProfile) => selectProfile(p)"
        />
        <n-button type="primary" @click="openAdd">+ 添加</n-button>
        <n-button
          v-if="activeProfile"
          :loading="testingId === activeProfile.id"
          @click="testActive"
        >
          测试连接
        </n-button>
        <n-button
          v-if="activeProfile"
          @click="openEdit(activeProfile)"
        >
          编辑连接
        </n-button>
        <n-button
          v-if="activeProfile"
          type="error"
          @click="removeProfile(activeProfile)"
        >
          删除连接
        </n-button>
      </n-space>

      <!-- 文件浏览器 -->
      <template v-if="activeProfile">
        <!-- 路径面包屑 + 操作按钮 -->
        <n-space align="center" justify="space-between" style="margin-bottom: 8px">
          <n-space align="center" :wrap="false">
            <n-button size="small" @click="goUp">↑ 上级</n-button>
            <n-breadcrumb>
              <n-breadcrumb-item @click="jumpTo('/')">/</n-breadcrumb-item>
              <n-breadcrumb-item
                v-for="crumb in breadcrumbs"
                :key="crumb.path"
                @click="jumpTo(crumb.path)"
              >
                {{ crumb.name }}
              </n-breadcrumb-item>
            </n-breadcrumb>
          </n-space>
          <n-space align="center">
            <n-button size="small" @click="loadDir">刷新</n-button>
            <n-button size="small" type="primary" @click="openUpload">
              上传
            </n-button>
            <n-button size="small" @click="openUploadDir">
              上传文件夹
            </n-button>
            <n-button size="small" @click="openMkdir">新建文件夹</n-button>
          </n-space>
        </n-space>

        <!-- 上传/下载进度条 -->
        <div v-if="progress.active" style="margin-bottom: 8px">
          <n-space align="center" :wrap="false">
            <n-text depth="3" style="font-size: 12px; white-space: nowrap">
              {{ progress.file || "传输中…" }}
            </n-text>
            <n-progress
              type="line"
              :percentage="progressPct"
              :show-indicator="progress.total > 0"
              style="min-width: 200px"
            />
          </n-space>
        </div>

        <!-- 文件列表 -->
        <n-data-table
          :columns="[
            {
              title: '名称',
              key: 'name',
              render: (row: FtpEntry) =>
                h(
                  'span',
                  { style: 'cursor: pointer', onClick: () => row.is_dir && enterDir(row.name) },
                  [
                    h('span', { style: 'margin-right: 6px' }, row.is_dir ? '📁' : '📄'),
                    row.name,
                  ],
                ),
            },
            { title: '大小', key: 'size', width: 100, render: (row: FtpEntry) => fmtSize(row.size) },
            { title: '修改时间', key: 'modified', width: 180, render: (row: FtpEntry) => fmtTime(row.modified) },
            {
              title: '操作',
              key: 'actions',
              width: 200,
              render: (row: FtpEntry) =>
                h('div', { style: 'display:flex; gap:6px' }, [
                  !row.is_dir
                    ? h(
                        NButton,
                        { size: 'small', onClick: () => startDownload(row) } as Record<string, unknown>,
                        () => '下载',
                      )
                    : null,
                  h(
                    NButton,
                    { size: 'small', onClick: () => promptRename(row) } as Record<string, unknown>,
                    () => '重命名',
                  ),
                  h(
                    NButton,
                    {
                      size: 'small',
                      type: 'error',
                      onClick: () => deleteEntry(row),
                    } as Record<string, unknown>,
                    () => '删除',
                  ),
                ]),
            },
          ]"
          :data="entries"
          :bordered="false"
          :pagination="false"
          :loading="loadingDir"
          :row-props="(row: FtpEntry) => ({
            style: 'cursor: pointer',
            onDblclick: () => row.is_dir && enterDir(row.name),
          })"
        />
      </template>

      <n-empty v-else description="选择或添加一个连接以开始管理文件" />
    </n-card>

    <!-- 添加/编辑连接弹窗 -->
    <n-modal v-model:show="showAdd" preset="card" :title="isEdit ? '编辑 FTP 连接' : '添加 FTP 连接'" style="width: 520px">
      <n-form label-placement="left" label-width="80">
        <n-form-item label="名称">
          <n-input v-model:value="addForm.name" placeholder="如：生产服务器" />
        </n-form-item>
        <n-form-item label="协议">
          <n-select
            v-model:value="addForm.protocol"
            :options="protocolOptions"
            @update:value="onProtocolChange"
          />
        </n-form-item>
        <n-form-item label="主机">
          <n-input v-model:value="addForm.host" placeholder="如：ftp.example.com" />
        </n-form-item>
        <n-form-item label="端口">
          <n-input-number v-model:value="addForm.port" :min="1" :max="65535" />
        </n-form-item>
        <n-form-item label="用户名">
          <n-input v-model:value="addForm.username" placeholder="如：root" />
        </n-form-item>
        <n-form-item label="密码">
          <n-input
            v-model:value="addForm.password"
            type="password"
            show-password-on="click"
            placeholder="FTP/FTPS/SFTP 密码认证"
          />
        </n-form-item>
        <n-form-item v-if="addForm.protocol === 'sftp'" label="密钥路径">
          <n-input
            v-model:value="addForm.ssh_key"
            placeholder="SSH 私钥路径（与密码二选一，可留空）"
          />
        </n-form-item>
      </n-form>
      <template #footer>
        <n-space justify="end">
          <n-button @click="showAdd = false">取消</n-button>
          <n-button type="primary" @click="submitProfile">{{ isEdit ? "保存" : "添加" }}</n-button>
        </n-space>
      </template>
    </n-modal>

    <!-- 新建文件夹弹窗 -->
    <n-modal v-model:show="showMkdir" preset="card" title="新建文件夹" style="width: 400px">
      <n-input v-model:value="mkdirName" placeholder="文件夹名称" @keyup.enter="submitMkdir" />
      <template #footer>
        <n-space justify="end">
          <n-button @click="showMkdir = false">取消</n-button>
          <n-button type="primary" @click="submitMkdir">创建</n-button>
        </n-space>
      </template>
    </n-modal>

    <!-- 本地目录选择器（上传源/下载目标） -->
    <DirectoryPicker
      v-model:show="showLocalPicker"
      :mode="pickerFileMode ? 'file' : 'dir'"
      @select="onPickLocal"
    />
  </n-space>
</template>
