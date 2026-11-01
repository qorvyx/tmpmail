// Typed access to the Rust side. Every command and event the window uses is
// here; the `main` capability allows exactly these commands.
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export interface Caps {
  private: boolean;
  custom_user: boolean;
  choose_domain: boolean;
  attachments: boolean;
  raw: boolean;
  delete_message: boolean;
  delete_inbox: boolean;
  login: boolean;
  lifetime: string;
}

export interface ProviderInfo {
  id: string;
  name: string;
  caps: Caps;
}

export interface ProbePlan {
  real: { name: string; key: number; images: boolean; theme: "light" | "dark" }[];
  controls: { name: string; url: string; sandbox: string | null }[];
  wait_secs: number;
}

export interface Init {
  origin: string;
  providers: ProviderInfo[];
  probe: ProbePlan | null;
  /** Whether attachments can be dragged out on this system. */
  drag: boolean;
  /** A notification click started the app (Windows): open this once the inboxes are in. */
  reveal: { address: string; id: string | null } | null;
}

export interface GuiSettings {
  notifications: boolean;
  tray: boolean;
  start_minimized: boolean;
}

export interface SettingsView {
  config_path: string | null;
  config_exists: boolean;
  /** Enabled providers in fallback order, then the others. */
  providers: { id: string; name: string; enabled: boolean }[];
  poll_interval: number;
  push: boolean;
  http_timeout: number;
  gui: GuiSettings;
  /** Environment variables that are set and win over the file. */
  overrides: string[];
  /** Whether a tray icon can be shown (Linux: a StatusNotifier host runs). */
  tray_host: boolean;
  os: string;
}

export interface SettingsInput extends GuiSettings {
  /** The enabled providers, in fallback order. */
  providers: string[];
  poll_interval: number;
  push: boolean;
  http_timeout: number;
}

export interface InboxView {
  address: string;
  provider: string;
  label: string | null;
  created_at: string;
  expires_at: string | null;
  expired: boolean;
  active: boolean;
  private: boolean;
}

export interface Summary {
  id: string;
  from: string;
  subject: string;
  intro: string;
  date: string | null;
  seen: boolean;
  has_attachments: boolean;
}

export interface AttachmentInfo {
  filename: string;
  content_type: string;
  size: number | null;
}

export type MessagesEvent =
  | { address: string; live: boolean; messages: Summary[] }
  | { address: string; live: boolean; error: string }
  /** The inbox has expired and is no longer checked. */
  | { address: string; expired: true };

export interface OpenedMessage {
  address: string;
  id: string;
  key: number;
  from: string;
  to: string[];
  subject: string;
  date: string | null;
  attachments: AttachmentInfo[];
  /** The verification code found in the message, if any. */
  code: string | null;
  /** The sender's bare address (what "copy sender" copies). */
  sender: string;
  /** http(s) links, likely verification links first. */
  links: string[];
  /** Whether the message has an HTML body. */
  html: boolean;
}

export type OpenedEvent = OpenedMessage | { address: string; id: string; error: string };

export interface Notice {
  kind: "error" | "info" | "success";
  text: string;
}

export interface Events {
  inboxes: { inboxes: InboxView[] };
  messages: MessagesEvent;
  opened: OpenedEvent;
  /** `from_tray`: made from the tray menu, not the window's own request. */
  created: { address: string; from_tray: boolean };
  /** Creating an inbox failed: why, and what to try. `field` names the
   * dialog field it is about (a taken label or username), shown there. */
  create_failed: { text: string; field?: "label" | "user" };
  deleted: { address: string; id: string };
  notice: Notice;
  /** A notification or the tray brought the window back: open this. */
  reveal: { address: string; id: string | null };
}

export function on<K extends keyof Events>(
  name: K,
  handler: (payload: Events[K]) => void,
): Promise<UnlistenFn> {
  return listen<Events[K]>(name, (e) => handler(e.payload));
}

export interface CreateArgs {
  provider: string | null;
  user: string | null;
  domain: string | null;
  label: string | null;
}

export const api = {
  init: () => invoke<Init>("init"),
  domains: (provider: string) => invoke<string[]>("domains", { provider }),
  createInbox: (args: CreateArgs) => invoke<void>("create_inbox", { ...args }),
  selectInbox: (address: string) => invoke<void>("select_inbox", { address }),
  useInbox: (address: string) => invoke<void>("use_inbox", { address }),
  deleteInbox: (address: string) => invoke<void>("delete_inbox", { address }),
  openMessage: (address: string, id: string) => invoke<void>("open_message", { address, id }),
  deleteMessage: (address: string, id: string) => invoke<void>("delete_message", { address, id }),
  copyAddress: (address: string) => invoke<void>("copy_address", { address }),
  copyFromMessage: (key: number, part: "code" | "sender" | "link", index?: number) =>
    invoke<void>("copy_from_message", { key, part, index: index ?? null }),
  openLink: (key: number, index: number) => invoke<void>("open_link", { key, index }),
  saveAttachment: (key: number, index: number) => invoke<string | null>("save_attachment", { key, index }),
  settings: () => invoke<SettingsView>("settings"),
  saveSettings: (input: SettingsInput) => invoke<void>("save_settings", { input }),
  revealConfig: () => invoke<void>("reveal_config"),
  prepareAttachment: (key: number, index: number) => invoke<void>("prepare_attachment", { key, index }),
  dragAttachment: (key: number, index: number) => invoke<void>("drag_attachment", { key, index }),
  probeFinish: () => invoke<void>("probe_finish"),
};

/// "just now", "5m ago", "3h ago", "2d ago".
export function ago(iso: string | null, now = Date.now()): string {
  if (!iso) return "";
  const secs = Math.round((now - Date.parse(iso)) / 1000);
  if (secs < 60) return "just now";
  if (secs < 3600) return `${Math.floor(secs / 60)}m ago`;
  if (secs < 86400) return `${Math.floor(secs / 3600)}h ago`;
  return `${Math.floor(secs / 86400)}d ago`;
}

/// "in 42m", "in 3h", "in 2d", or null once past.
export function until(iso: string | null, now = Date.now()): string | null {
  if (!iso) return null;
  const secs = Math.round((Date.parse(iso) - now) / 1000);
  if (secs <= 0) return null;
  if (secs < 3600) return `in ${Math.max(1, Math.floor(secs / 60))}m`;
  if (secs < 86400) return `in ${Math.floor(secs / 3600)}h`;
  return `in ${Math.floor(secs / 86400)}d`;
}

/// The display name of "Name <addr>", else the address.
export function senderName(from: string): string {
  const m = /^\s*"?([^"<]*?)"?\s*<[^>]+>\s*$/.exec(from);
  return m?.[1] ? m[1] : from;
}

export function fileSize(bytes: number | null): string {
  if (bytes == null) return "";
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(bytes < 10240 ? 1 : 0)} KB`;
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}

/// A short type name for a MIME type: "PDF", "PNG image", "ZIP".
export function fileKind(contentType: string, filename: string): string {
  const ct = (contentType.split(";")[0] ?? "").trim().toLowerCase();
  const ext = filename.includes(".") ? filename.split(".").pop()!.toUpperCase() : "";
  if (ct.startsWith("image/")) return `${ext || ct.slice(6).toUpperCase()} image`;
  if (ct === "application/pdf") return "PDF";
  if (ext && ext.length <= 5) return ext;
  return ct || "File";
}

/**
 * Scrolls `scroller` (only it, never the page) just enough to show `row`:
 * the selected message follows j/k, the selected inbox [/], a field's error
 * shows in a dialog's body. `scrollIntoView` would scroll every scrolling
 * ancestor, the page too, which here has no scrollbar to undo it.
 */
export function reveal(scroller: Element | null | undefined, row: Element | null | undefined) {
  if (!scroller || !row) return;
  const s = scroller.getBoundingClientRect();
  const r = row.getBoundingClientRect();
  if (r.top < s.top) scroller.scrollTop -= s.top - r.top;
  else if (r.bottom > s.bottom) scroller.scrollTop += Math.min(r.bottom - s.bottom, r.top - s.top);
}
