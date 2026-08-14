/**
 * API 适配层：桌面端走 Tauri invoke，Web 面板走 fetch。
 * 运行模式由构建时注入：VITE_RUNPHP_MODE = "desktop" | "panel"
 */

const mode = import.meta.env.VITE_RUNPHP_MODE ?? "desktop";

/** 判断当前是否运行在 Tauri 桌面壳中。 */
export const isDesktop = mode === "desktop";

/** 调用后端命令。桌面端走 invoke，面板端走 REST。 */
export async function call<T>(cmd: string, args: Record<string, unknown> = {}): Promise<T> {
  if (isDesktop) {
    const { invoke } = await import("@tauri-apps/api/core");
    return invoke<T>(cmd, args);
  }
  const res = await fetch(`/api/${cmd}`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(args),
  });
  if (!res.ok) {
    throw new Error(await res.text());
  }
  return res.json();
}

// ---- 具体接口（M1 骨架，后续随 core 能力扩展） ----

/** 示例：问候（M1 验证用）。 */
export const greet = (name: string) => call<string>("greet", { name });

/** 获取数据目录。 */
export const getDataDir = () => call<string>("data_dir");
