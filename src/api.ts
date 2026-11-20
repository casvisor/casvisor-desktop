import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useEffect, useState } from "react";

export interface Status {
  state: "unconfigured" | "connecting" | "online" | "offline" | "error";
  message: string;
  server: string;
}

export interface SessionInfo {
  id: number;
  session: string;
  user: string;
  userDisplayName: string;
  clientIp: string;
  permissions: string[];
  accepted: boolean;
}

export interface ConfigView {
  server: string;
  fingerprint: string;
  hasToken: boolean;
  unattended: boolean;
}

export interface Snapshot {
  status: Status;
  session: SessionInfo | null;
  config: ConfigView;
  service: boolean;
  version: string;
}

export interface Settings {
  server: string;
  fingerprint: string;
  token: string;
  unattended: boolean;
}

export const api = {
  snapshot: () => invoke<Snapshot>("get_snapshot"),
  saveSettings: (settings: Settings) => invoke<void>("save_settings", { settings }),
  answerConsent: (id: number, accepted: boolean) => invoke<void>("answer_consent", { id, accepted }),
  disconnect: () => invoke<void>("disconnect_session"),
  installService: () => invoke<void>("install_service"),
  uninstallService: () => invoke<void>("uninstall_service"),
  showMain: () => invoke<void>("show_main"),
};

export function useSnapshot() {
  const [snapshot, setSnapshot] = useState<Snapshot | null>(null);

  useEffect(() => {
    let alive = true;
    const refresh = () =>
      api.snapshot().then((s) => {
        if (alive) {
          setSnapshot(s);
        }
      });
    refresh();
    const unlisten = [
      listen<Status>("status", (e) => setSnapshot((s) => (s ? { ...s, status: e.payload } : s))),
      listen<SessionInfo | null>("session", (e) => setSnapshot((s) => (s ? { ...s, session: e.payload } : s))),
      listen<ConfigView>("config", (e) => setSnapshot((s) => (s ? { ...s, config: e.payload } : s))),
    ];
    return () => {
      alive = false;
      unlisten.forEach((p) => p.then((f) => f()));
    };
  }, []);

  return [snapshot, setSnapshot] as const;
}

export function displayName(session: SessionInfo) {
  return session.userDisplayName || session.user;
}

const zh = navigator.language.toLowerCase().startsWith("zh");

const messages: Record<string, [string, string]> = {
  online: ["Connected to {server}", "已连接到 {server}"],
  connecting: ["Connecting to {server}…", "正在连接 {server}…"],
  offline: ["Not connected", "没有连接"],
  error: ["Can't connect", "连不上"],
  unconfigured: ["Not set up yet", "还没有设置"],
  controlling: ["{name} is controlling this computer", "{name} 正在控制这台电脑"],
  disconnect: ["Disconnect", "断开"],
  request: ["{name} wants to control this computer", "{name} 请求控制这台电脑"],
  requestDetail: ["Account {user}, from {ip}", "账号 {user}，来自 {ip}"],
  permissions: ["Requested", "申请的权限"],
  allow: ["Allow", "允许"],
  deny: ["Deny ({seconds})", "拒绝（{seconds}）"],
  settings: ["Settings", "设置"],
  server: ["Casvisor address", "Casvisor 地址"],
  serverHint: ["e.g. https://casvisor.example.com", "例如 https://casvisor.example.com"],
  token: ["Device token", "设备令牌"],
  tokenSaved: ["Saved, leave empty to keep it", "已保存，不改就留空"],
  fingerprint: ["Casvisor's host key fingerprint", "Casvisor 主机密钥指纹"],
  unattended: ["Unattended: allow sessions without asking", "无人值守：不询问，直接允许远程控制"],
  save: ["Save", "保存"],
  saved: ["Saved", "已保存"],
  service: ["Windows service", "Windows 服务"],
  serviceOn: [
    "Casvisor Desktop runs as a service, the lock screen, the sign-in screen and UAC prompts can be controlled too. Changing the settings needs an administrator.",
    "Casvisor Desktop 以服务方式运行，锁屏、登录界面和 UAC 弹窗也能控制。修改设置需要管理员权限。",
  ],
  serviceOff: [
    "Install the service so the lock screen, the sign-in screen and UAC prompts can be controlled too, and the computer is reachable before anyone signs in. Needs an administrator.",
    "安装服务后，锁屏、登录界面和 UAC 弹窗也能控制，没人登录时也能连上。需要管理员权限。",
  ],
  installService: ["Install service", "安装服务"],
  uninstallService: ["Uninstall service", "卸载服务"],
  working: ["Waiting for the administrator prompt…", "等待管理员确认…"],
  Connect: ["View and control", "查看和操作"],
  Copy: ["Copy from this computer", "从这台电脑复制"],
  Paste: ["Paste to this computer", "粘贴到这台电脑"],
  Upload: ["Upload files", "上传文件"],
  Download: ["Download files", "下载文件"],
};

export function t(key: string, vars: Record<string, string | number> = {}) {
  const pair = messages[key];
  let text = pair ? pair[zh ? 1 : 0] : key;
  for (const [k, v] of Object.entries(vars)) {
    text = text.replace(`{${k}}`, String(v));
  }
  return text;
}
