<script setup lang="ts">
import { ref, watch } from "vue";
import { fsBrowse, type DirListing } from "../api";

const props = withDefaults(
  defineProps<{ mode?: "dir" | "file"; show?: boolean }>(),
  { mode: "dir", show: false },
);
const show = defineModel<boolean>("show", { default: false });
const emit = defineEmits<{ (e: "select", path: string): void }>();

const listing = ref<DirListing | null>(null);
const loading = ref(false);
const manual = ref("");

watch(
  () => show.value,
  (v) => {
    if (v) void browse(null);
  },
);

async function browse(path: string | null) {
  loading.value = true;
  try {
    listing.value = await fsBrowse(path ?? undefined);
    manual.value = listing.value.current;
  } finally {
    loading.value = false;
  }
}

function goUp() {
  const p = listing.value?.parent;
  if (p != null) void browse(p);
}

function jump() {
  const p = manual.value.trim();
  if (p) void browse(p);
}

/** 选文件模式下：点击文件直接选中；选目录模式下：选当前目录 */
function pickFile(file: { name: string; path: string }) {
  emit("select", file.path);
  show.value = false;
}

function pick() {
  if (listing.value?.current) {
    emit("select", listing.value.current);
    show.value = false;
  }
}
</script>

<template>
  <n-modal
    v-model:show="show"
    preset="card"
    :title="props.mode === 'file' ? '选择文件' : '选择目录'"
    style="width: 560px"
  >
    <n-space vertical>
      <n-space :wrap="false">
        <n-button size="small" :disabled="!listing?.parent" @click="goUp">
          上级
        </n-button>
        <n-input
          v-model:value="manual"
          size="small"
          placeholder="输入路径跳转"
          @keyup.enter="jump"
        />
        <n-button size="small" @click="jump">跳转</n-button>
      </n-space>

      <n-spin :show="loading">
        <div
          style="
            max-height: 320px;
            overflow: auto;
            border: 1px solid rgba(128, 128, 128, 0.2);
            border-radius: 4px;
          "
        >
          <div
            v-for="d in listing?.dirs ?? []"
            :key="d.path"
            class="entry-row"
            @click="browse(d.path)"
          >
            📁 {{ d.name }}
          </div>
          <template v-if="props.mode === 'file'">
            <div
              v-for="f in listing?.files ?? []"
              :key="f.path"
              class="entry-row file-row"
              @click="pickFile(f)"
            >
              📄 {{ f.name }}
            </div>
          </template>
          <n-text
            v-if="
              !loading &&
              (listing?.dirs.length ?? 0) === 0 &&
              (listing?.files.length ?? 0) === 0
            "
            depth="3"
            style="display: block; padding: 12px"
          >
            （空目录）
          </n-text>
        </div>
      </n-spin>

      <n-text depth="3" style="font-size: 12px">
        当前：{{ listing?.current || "计算机" }}
      </n-text>
    </n-space>

    <template #footer>
      <n-space justify="end">
        <n-button @click="show = false">取消</n-button>
        <n-button
          v-if="props.mode === 'dir'"
          type="primary"
          :disabled="!listing?.current"
          @click="pick"
        >
          选择当前目录
        </n-button>
      </n-space>
    </template>
  </n-modal>
</template>

<style scoped>
.entry-row {
  padding: 6px 12px;
  cursor: pointer;
}
.entry-row:hover {
  background: rgba(128, 128, 128, 0.12);
}
.file-row:hover {
  background: rgba(24, 160, 88, 0.12);
}
</style>
