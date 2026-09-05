import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { check, type Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import {
  enable as enableAutostart,
  disable as disableAutostart,
  isEnabled as isAutostartEnabled,
} from "@tauri-apps/plugin-autostart";
import "./styles.css";

// ---- Backend DTOs (mirror src-tauri/src/status.rs, commands.rs, settings.rs) ----
type SyncState =
  | "signedOut"
  | "idle"
  | "syncing"
  | "paused"
  | "signInRequired"
  | "offline"
  | "error";
interface Failure {
  path: string;
  action: string;
  error: string;
}
interface Report {
  planned: number;
  downloaded: number;
  uploaded: number;
  trashedLocal: number;
  trashedRemote: number;
  conflicts: number;
  foldersCreated: number;
  failures: Failure[];
  aborted: string | null;
}
interface Status {
  state: SyncState;
  detail: string;
  signedIn: boolean;
  appleId: string | null;
  syncFolder: string;
  lastSync: string | null;
  lastReport: Report | null;
  progress: { done: number; total: number; current: string } | null;
}
interface Settings {
  syncFolder: string;
  pollIntervalSecs: number;
  closeToTray: boolean;
  notificationsEnabled: boolean;
  paused: boolean;
}
interface Phone {
  id: number;
  number: string;
}
type SignInResult = { step: "signedIn" } | { step: "needsCode"; phones: Phone[] };
interface AppInfo {
  version: string;
  logPath: string;
  dataDir: string;
}

const STATE_LABEL: Record<SyncState, string> = {
  signedOut: "Not signed in",
  idle: "Up to date",
  syncing: "Syncing",
  paused: "Paused",
  signInRequired: "Sign-in needed",
  offline: "Offline",
  error: "Attention needed",
};

// ---- Theme ----
const THEME_KEY = "wattdrive.theme";
type ThemePref = "system" | "business" | "corporate";
function applyTheme(pref: ThemePref): void {
  const dark = matchMedia("(prefers-color-scheme: dark)").matches;
  const theme = pref === "system" ? (dark ? "business" : "corporate") : pref;
  document.documentElement.dataset.theme = theme;
}
function themePref(): ThemePref {
  const raw = localStorage.getItem(THEME_KEY);
  return raw === "business" || raw === "corporate" ? raw : "system";
}
matchMedia("(prefers-color-scheme: dark)").addEventListener("change", () => applyTheme(themePref()));
applyTheme(themePref());

// ---- DOM ----
const app = document.getElementById("app")!;
app.innerHTML = `
<div class="shell">
  <div id="update-banner" class="update-banner hidden">
    <span id="update-text"></span>
    <span><button id="update-install" class="btn btn-xs btn-primary">Install and restart</button>
    <button id="update-later" class="btn btn-xs btn-ghost">Later</button></span>
  </div>
  <div class="topbar">
    <div class="brand"><img src="/icon.png" alt="" /> WattDrive</div>
    <div>
      <button id="btn-settings" class="btn btn-sm btn-ghost">Settings</button>
      <button id="btn-about" class="btn btn-sm btn-ghost">About</button>
    </div>
  </div>
  <div class="content">

    <section id="view-signin" class="centered hidden">
      <h1>Sign in to iCloud</h1>
      <p class="muted">WattDrive keeps a folder on this computer in sync with your iCloud Drive.
      Your password is used only to sign in to iCloud from here and is kept in your system keyring.</p>
      <div class="field"><label for="apple-id">Apple Account email</label>
        <input id="apple-id" class="input input-bordered input-sm" type="email" autocomplete="username" spellcheck="false" /></div>
      <div class="field"><label for="password">Password</label>
        <input id="password" class="input input-bordered input-sm" type="password" autocomplete="current-password" /></div>
      <p id="signin-error" class="error"></p>
      <div class="actions"><button id="btn-signin" class="btn btn-sm btn-primary">Sign in</button></div>
    </section>

    <section id="view-code" class="centered hidden">
      <h1>Enter the verification code</h1>
      <p class="muted" id="code-hint">Apple sent a six-digit code to your trusted devices. Approve the sign-in there, then type the code below.</p>
      <div class="field"><label for="code">Verification code</label>
        <input id="code" class="input input-bordered input-sm" inputmode="numeric" autocomplete="one-time-code" maxlength="7" /></div>
      <div id="sms-row" class="field hidden"><label for="sms-phone">No trusted device to hand? Text the code instead</label>
        <div style="display:flex;gap:8px"><select id="sms-phone" class="select select-bordered select-sm" style="flex:1"></select>
        <button id="btn-sms" class="btn btn-sm">Text me</button></div></div>
      <p id="code-progress" class="muted"></p>
      <p id="code-error" class="error"></p>
      <div class="actions"><button id="btn-code-back" class="btn btn-sm btn-ghost">Back</button>
        <button id="btn-code" class="btn btn-sm btn-primary">Verify</button></div>
    </section>

    <section id="view-main" class="hidden">
      <div class="panel">
        <div class="state-row">
          <span id="state-dot" class="state-dot"></span>
          <div style="flex:1">
            <div id="state-title" class="state-title"></div>
            <div id="state-detail" class="muted"></div>
          </div>
          <button id="btn-sync" class="btn btn-sm btn-primary">Sync now</button>
          <button id="btn-pause" class="btn btn-sm">Pause</button>
        </div>
        <div id="signin-needed" class="hidden" style="margin-top:12px">
          <button id="btn-resume" class="btn btn-sm btn-warning">Continue with saved password</button>
          <button id="btn-resignin" class="btn btn-sm btn-ghost">Sign in with a different password</button>
          <span id="resume-error" class="error" style="margin-left:8px"></span>
        </div>
        <progress id="progress" class="progress progress-info w-full hidden" style="margin-top:12px"></progress>
      </div>
      <div class="panel">
        <h2>Folder</h2>
        <div class="folder-row"><code id="folder-path"></code>
          <button id="btn-open-folder" class="btn btn-xs">Open</button>
          <button id="btn-open-trash" class="btn btn-xs btn-ghost">Local trash</button></div>
        <p class="muted" style="margin-top:8px">Files you delete here go to <code>.wattdrive-trash</code> inside the folder when iCloud removes them, and to iCloud's Recently Deleted when you remove them here. Nothing is ever hard-deleted.</p>
      </div>
      <div class="panel">
        <h2>Last sync <span id="last-sync" class="muted"></span></h2>
        <div id="stats" class="stats"></div>
        <ul id="failures" class="failures"></ul>
      </div>
    </section>

    <section id="view-settings" class="hidden">
      <div class="panel">
        <h2>Settings</h2>
        <div class="setting"><div><div>Sync folder</div><div class="desc">Absolute path. Changing it starts a fresh mirror in the new location.</div></div>
          <input id="set-folder" class="input input-bordered input-sm" style="width:300px" /></div>
        <div class="setting"><div><div>Check iCloud every</div><div class="desc">Local changes sync immediately; this is how often remote changes are picked up.</div></div>
          <select id="set-interval" class="select select-bordered select-sm">
            <option value="60">1 minute</option><option value="120">2 minutes</option><option value="300">5 minutes</option>
            <option value="900">15 minutes</option><option value="3600">1 hour</option></select></div>
        <div class="setting"><div>Start at login (in the tray)</div><input id="set-autostart" type="checkbox" class="toggle toggle-sm" /></div>
        <div class="setting"><div>Closing the window keeps WattDrive running in the tray</div><input id="set-tray" type="checkbox" class="toggle toggle-sm" /></div>
        <div class="setting"><div>Desktop notifications for conflicts and sign-in problems</div><input id="set-notify" type="checkbox" class="toggle toggle-sm" /></div>
        <div class="setting"><div>Theme</div>
          <select id="set-theme" class="select select-bordered select-sm">
            <option value="system">Follow system</option><option value="business">Dark</option><option value="corporate">Light</option></select></div>
        <p id="settings-error" class="error"></p>
        <div class="actions"><button id="btn-signout" class="btn btn-sm btn-ghost btn-error">Sign out</button>
          <span style="flex:1"></span>
          <button id="btn-settings-cancel" class="btn btn-sm btn-ghost">Cancel</button>
          <button id="btn-settings-save" class="btn btn-sm btn-primary">Save</button></div>
      </div>
    </section>

    <section id="view-about" class="hidden">
      <div class="panel">
        <h2>About WattDrive</h2>
        <p id="about-version" class="muted"></p>
        <p class="muted">Two-way sync between a local folder and iCloud Drive, for Linux. Talks to the same private
        endpoints icloud.com uses, so Apple can change them without notice.</p>
        <p class="muted">Log: <code id="about-log"></code></p>
        <p id="about-update" class="muted"></p>
        <div class="actions"><button id="btn-about-back" class="btn btn-sm btn-ghost">Back</button>
          <button id="btn-check-update" class="btn btn-sm">Check for updates</button></div>
      </div>
    </section>
  </div>
</div>`;

const $ = <T extends HTMLElement = HTMLElement>(id: string): T => document.getElementById(id) as T;
const views = ["view-signin", "view-code", "view-main", "view-settings", "view-about"] as const;
type View = (typeof views)[number];
let current: View = "view-main";
function show(view: View): void {
  current = view;
  for (const v of views) $(v).classList.toggle("hidden", v !== view);
}

// ---- Status rendering ----
let status: Status | null = null;
let phones: Phone[] = [];

function fmtTime(iso: string | null): string {
  if (!iso) return "never";
  const d = new Date(iso);
  return isNaN(d.getTime()) ? iso : d.toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" });
}

function render(s: Status): void {
  status = s;
  if (!s.signedIn) {
    if (current === "view-main") show("view-signin");
  } else if (current === "view-signin" || current === "view-code") {
    show("view-main");
  }
  $("state-dot").className = `state-dot ${s.state}`;
  $("state-title").textContent = STATE_LABEL[s.state] + (s.appleId ? ` · ${s.appleId}` : "");
  $("state-detail").textContent = s.detail;
  $("folder-path").textContent = s.syncFolder;
  $("btn-pause").textContent = s.state === "paused" ? "Resume" : "Pause";
  $<HTMLButtonElement>("btn-sync").disabled = s.state === "syncing";
  $("signin-needed").classList.toggle("hidden", s.state !== "signInRequired");
  const prog = $<HTMLProgressElement>("progress");
  if (s.progress && s.progress.total > 0) {
    prog.max = s.progress.total;
    prog.value = s.progress.done;
    prog.classList.remove("hidden");
  } else {
    prog.classList.add("hidden");
  }
  $("last-sync").textContent = fmtTime(s.lastSync);
  const r = s.lastReport;
  const stats = $("stats");
  if (!r) {
    stats.innerHTML = `<div class="muted">No sync has run yet.</div>`;
  } else {
    const cells: [number, string][] = [
      [r.downloaded, "downloaded"],
      [r.uploaded, "uploaded"],
      [r.trashedLocal + r.trashedRemote, "removed"],
      [r.conflicts, "conflicts"],
      [r.foldersCreated, "folders"],
    ];
    stats.innerHTML = cells
      .map(([n, l]) => `<div class="stat"><div class="n">${n}</div><div class="l">${l}</div></div>`)
      .join("");
  }
  const fails = $("failures");
  fails.innerHTML = "";
  for (const f of r?.failures ?? []) {
    const li = document.createElement("li");
    li.textContent = `${f.action} — ${f.path}: ${f.error}`;
    fails.appendChild(li);
  }
  if (r?.aborted) {
    const li = document.createElement("li");
    li.textContent = `Stopped early: ${r.aborted}`;
    fails.appendChild(li);
  }
}

// ---- Sign-in flow ----
function busy(btn: HTMLButtonElement, on: boolean, label: string): void {
  btn.disabled = on;
  btn.textContent = on ? "Please wait…" : label;
}

function afterSignIn(result: SignInResult): void {
  if (result.step === "needsCode") {
    phones = result.phones;
    const sel = $<HTMLSelectElement>("sms-phone");
    sel.innerHTML = phones.map((p) => `<option value="${p.id}">${p.number}</option>`).join("");
    $("sms-row").classList.toggle("hidden", phones.length === 0);
    $("code-error").textContent = "";
    $("code-progress").textContent = "";
    $<HTMLInputElement>("code").value = "";
    show("view-code");
    $("code").focus();
  } else {
    show("view-main");
  }
}

async function doResume(): Promise<void> {
  const btn = $<HTMLButtonElement>("btn-resume");
  $("resume-error").textContent = "";
  busy(btn, true, "Continue with saved password");
  try {
    afterSignIn(await invoke<SignInResult>("resume_sign_in"));
  } catch (e) {
    $("resume-error").textContent = String(e);
  } finally {
    busy(btn, false, "Continue with saved password");
  }
}

async function doSignIn(): Promise<void> {
  const btn = $<HTMLButtonElement>("btn-signin");
  $("signin-error").textContent = "";
  busy(btn, true, "Sign in");
  try {
    const result = await invoke<SignInResult>("sign_in", {
      appleId: $<HTMLInputElement>("apple-id").value,
      password: $<HTMLInputElement>("password").value,
    });
    $<HTMLInputElement>("password").value = "";
    afterSignIn(result);
  } catch (e) {
    $("signin-error").textContent = String(e);
  } finally {
    busy(btn, false, "Sign in");
  }
}

let smsPhoneId: number | null = null;
async function doVerify(): Promise<void> {
  const btn = $<HTMLButtonElement>("btn-code");
  $("code-error").textContent = "";
  busy(btn, true, "Verify");
  try {
    const code = $<HTMLInputElement>("code").value;
    if (smsPhoneId !== null) await invoke("submit_sms_code", { code, phoneId: smsPhoneId });
    else await invoke("submit_code", { code });
    smsPhoneId = null;
    show("view-main");
  } catch (e) {
    $("code-error").textContent = String(e);
  } finally {
    busy(btn, false, "Verify");
  }
}

async function doSms(): Promise<void> {
  const id = Number($<HTMLSelectElement>("sms-phone").value);
  if (!id) return;
  $("code-error").textContent = "";
  try {
    await invoke("request_sms", { phoneId: id });
    smsPhoneId = id;
    $("code-hint").textContent = "A text with the code is on its way. Type it below.";
  } catch (e) {
    $("code-error").textContent = String(e);
  }
}

// ---- Settings ----
async function openSettings(): Promise<void> {
  const s = await invoke<Settings>("get_settings");
  $<HTMLInputElement>("set-folder").value = s.syncFolder;
  $<HTMLSelectElement>("set-interval").value = String(s.pollIntervalSecs);
  $<HTMLInputElement>("set-tray").checked = s.closeToTray;
  $<HTMLInputElement>("set-notify").checked = s.notificationsEnabled;
  $<HTMLSelectElement>("set-theme").value = themePref();
  $<HTMLInputElement>("set-autostart").checked = await isAutostartEnabled().catch(() => false);
  $("settings-error").textContent = "";
  $("btn-signout").classList.toggle("hidden", !status?.signedIn);
  show("view-settings");
}

async function saveSettings(): Promise<void> {
  const before = await invoke<Settings>("get_settings");
  const next: Settings = {
    ...before,
    syncFolder: $<HTMLInputElement>("set-folder").value.trim(),
    pollIntervalSecs: Number($<HTMLSelectElement>("set-interval").value),
    closeToTray: $<HTMLInputElement>("set-tray").checked,
    notificationsEnabled: $<HTMLInputElement>("set-notify").checked,
  };
  try {
    await invoke("set_settings", { settings: next });
    const pref = $<HTMLSelectElement>("set-theme").value as ThemePref;
    localStorage.setItem(THEME_KEY, pref);
    applyTheme(pref);
    const wantAuto = $<HTMLInputElement>("set-autostart").checked;
    const haveAuto = await isAutostartEnabled().catch(() => false);
    if (wantAuto !== haveAuto) {
      if (wantAuto) await enableAutostart();
      else await disableAutostart();
    }
    show(status?.signedIn ? "view-main" : "view-signin");
  } catch (e) {
    $("settings-error").textContent = String(e);
  }
}

async function togglePause(): Promise<void> {
  const s = await invoke<Settings>("get_settings");
  await invoke("set_settings", { settings: { ...s, paused: !s.paused } });
}

// ---- Updates ----
let pendingUpdate: Update | null = null;
async function checkUpdates(interactive: boolean): Promise<void> {
  try {
    const upd = await check();
    if (upd) {
      pendingUpdate = upd;
      $("update-text").textContent = `WattDrive ${upd.version} is available.`;
      $("update-banner").classList.remove("hidden");
      if (interactive) $("about-update").textContent = `Version ${upd.version} is available — use the banner to install.`;
    } else if (interactive) {
      $("about-update").textContent = "You are on the latest version.";
    }
  } catch (e) {
    if (interactive) $("about-update").textContent = `Update check failed: ${e}`;
  }
}

// ---- Wiring ----
$("btn-signin").addEventListener("click", () => void doSignIn());
$("password").addEventListener("keydown", (e) => e.key === "Enter" && void doSignIn());
$("btn-code").addEventListener("click", () => void doVerify());
$("code").addEventListener("keydown", (e) => e.key === "Enter" && void doVerify());
$("btn-sms").addEventListener("click", () => void doSms());
$("btn-code-back").addEventListener("click", () => show("view-signin"));
$("btn-sync").addEventListener("click", () => void invoke("sync_now"));
$("btn-pause").addEventListener("click", () => void togglePause());
$("btn-resume").addEventListener("click", () => void doResume());
$("btn-resignin").addEventListener("click", () => {
  if (status?.appleId) $<HTMLInputElement>("apple-id").value = status.appleId;
  show("view-signin");
});
$("btn-open-folder").addEventListener("click", () => void invoke("open_sync_folder"));
$("btn-open-trash").addEventListener("click", () => void invoke("open_trash_folder"));
$("btn-settings").addEventListener("click", () => void openSettings());
$("btn-settings-cancel").addEventListener("click", () => show(status?.signedIn ? "view-main" : "view-signin"));
$("btn-settings-save").addEventListener("click", () => void saveSettings());
$("btn-signout").addEventListener("click", async () => {
  try {
    await invoke("sign_out");
    show("view-signin");
  } catch (e) {
    $("settings-error").textContent = String(e);
  }
});
$("btn-about").addEventListener("click", async () => {
  const info = await invoke<AppInfo>("app_info");
  $("about-version").textContent = `Version ${info.version}`;
  $("about-log").textContent = info.logPath;
  $("about-update").textContent = "";
  show("view-about");
});
$("btn-about-back").addEventListener("click", () => show(status?.signedIn ? "view-main" : "view-signin"));
$("btn-check-update").addEventListener("click", () => void checkUpdates(true));
$("update-install").addEventListener("click", async () => {
  if (!pendingUpdate) return;
  try {
    await pendingUpdate.downloadAndInstall();
    await relaunch();
  } catch (e) {
    $("update-text").textContent = `Update failed: ${e}`;
  }
});
$("update-later").addEventListener("click", () => $("update-banner").classList.add("hidden"));

void listen<Status>("sync-status", (e) => render(e.payload));
void listen<string>("auth-progress", (e) => {
  $("code-progress").textContent = e.payload;
});

async function boot(): Promise<void> {
  const initial = await invoke<Status>("get_status");
  // Every section starts hidden; pick the first view explicitly. render()
  // only switches views on the signed-in / signed-out transitions.
  show(initial.signedIn ? "view-main" : "view-signin");
  render(initial);
  const hidden = await invoke<boolean>("started_hidden");
  if (!hidden) await getCurrentWindow().show();
  setTimeout(() => void checkUpdates(false), 10_000);
}
void boot();
