// 自动更新模块 —— 包一层 tauri-plugin-updater，让上层只关心状态机
//
// 状态机：
//   idle → checking → available / up-to-date / error
//                          ↓
//                       downloading → ready → 重启
//
// 订阅：
//   onUpdateState(fn) → 返回取消订阅函数
//
// 注意事项：
// - 启动时的"静默检查"由 main.ts 调 checkForUpdate(true)，不打 UI
// - 设置面板的"手动检查"由 settings.ts 调 checkForUpdate(false)
// - 真正下载 + 安装由用户显式触发（避免突然弹窗/重启）
import { check, type Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";

export type UpdateStatus =
  | "idle"
  | "checking"
  | "available"
  | "up-to-date"
  | "downloading"
  | "ready"
  | "error"
  | "disabled";

export interface UpdateState {
  status: UpdateStatus;
  /** 新版本号（available / downloading / ready 时填） */
  version?: string;
  /** 更新说明（GitHub release body） */
  notes?: string;
  /** 下载进度 0-1（downloading 时填） */
  progress?: number;
  /** 错误信息（error 时填） */
  error?: string;
}

type Listener = (s: UpdateState) => void;

let currentUpdate: Update | null = null;
let listeners: Listener[] = [];

function emit(s: UpdateState) {
  for (const fn of listeners) fn(s);
}

export function onUpdateState(fn: Listener): () => void {
  listeners.push(fn);
  return () => {
    listeners = listeners.filter((l) => l !== fn);
  };
}

export function getCurrentUpdate(): Update | null {
  return currentUpdate;
}

/** 仅检查，不下载。silent=true 时不 emit "checking" 状态。 */
export async function checkForUpdate(silent = false): Promise<UpdateState> {
  if (!silent) emit({ status: "checking" });
  try {
    const update = await check();
    if (!update) {
      const s: UpdateState = { status: "up-to-date" };
      emit(s);
      return s;
    }
    currentUpdate = update;
    const s: UpdateState = {
      status: "available",
      version: update.version,
      notes: update.body ?? undefined,
    };
    emit(s);
    return s;
  } catch (e) {
    // 离线 / 网络错 / 签名失败 / pubkey 未配 —— 统一报 error
    const msg = e instanceof Error ? e.message : String(e);
    const s: UpdateState = { status: "error", error: msg };
    emit(s);
    return s;
  }
}

/** 下载并安装，然后重启 app。需用户显式触发。 */
export async function downloadAndInstall(): Promise<UpdateState> {
  if (!currentUpdate) {
    const s: UpdateState = { status: "error", error: "没有可用更新" };
    emit(s);
    return s;
  }
  const version = currentUpdate.version;
  emit({ status: "downloading", version });
  try {
    let total = 0;
    let downloaded = 0;
    await currentUpdate.downloadAndInstall((event) => {
      if (event.event === "Started") {
        total = event.data.contentLength ?? 0;
      } else if (event.event === "Progress") {
        downloaded += event.data.chunkLength;
        emit({
          status: "downloading",
          version,
          progress: total > 0 ? downloaded / total : undefined,
        });
      } else if (event.event === "Finished") {
        emit({ status: "ready", version });
      }
    });
    emit({ status: "ready", version });
    // 等用户重启提示确认
    return { status: "ready", version };
  } catch (e) {
    const msg = e instanceof Error ? e.message : String(e);
    const s: UpdateState = { status: "error", error: msg };
    emit(s);
    return s;
  }
}

/** 重启 app 完成安装（用户在 UI 上点"立即重启"后调用） */
export async function relaunchApp(): Promise<void> {
  await relaunch();
}
