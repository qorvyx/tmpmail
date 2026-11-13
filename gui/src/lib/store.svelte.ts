// The window's state and every action on it. Components read from `app` and
// call its methods; the Rust side answers with events, handled in `listen`.
import { SvelteSet } from "svelte/reactivity";
import {
  api,
  on,
  senderName,
  type CreateArgs,
  type InboxView,
  type Init,
  type OpenedEvent,
  type ProviderInfo,
  type Summary,
} from "./api";

export interface MailState {
  messages: Summary[];
  loaded: boolean;
  live: boolean;
  error: string | null;
  expired: boolean;
}

export interface Toast {
  id: number;
  kind: "error" | "info" | "success" | "mail";
  text: string;
  action?: { label: string; run: () => void };
  /** The message a "New mail" toast's Open opens, and whether it names only that one. */
  opens?: { address: string; id: string; only: boolean };
}

export type Confirm =
  | { kind: "message"; address: string; id: string; subject: string }
  | { kind: "inbox"; address: string };

/// Ids of messages that arrived while the window was open: "address\nid".
const freshKey = (address: string, id: string) => `${address}\n${id}`;

function errorText(e: unknown): string {
  return typeof e === "string" ? e : e instanceof Error ? e.message : String(e);
}

class AppState {
  init = $state<Init | null>(null);
  startError = $state<string | null>(null);
  inboxes = $state<InboxView[]>([]);
  mail = $state<Record<string, MailState>>({});
  selected = $state<string | null>(null);
  openId = $state<string | null>(null);
  opened = $state<OpenedEvent | null>(null);
  creating = $state(false);
  /** Why the new inbox dialog's label or username was refused. */
  createError = $state<{ field: "label" | "user"; text: string } | null>(null);
  query = $state("");
  fresh = new SvelteSet<string>();
  toasts = $state<Toast[]>([]);
  now = $state(Date.now());

  // Dialogs.
  newInboxOpen = $state(false);
  helpOpen = $state(false);
  settingsOpen = $state(false);
  confirm = $state<Confirm | null>(null);

  // The open message's view.
  images = $state(false);
  original = $state(false);
  source = $state(false);

  /** Attachments ready to drag out, by "key/index"; false while downloading. */
  dragReady = $state<Record<string, boolean>>({});

  /** Message ids each inbox had at its first listing, to spot new mail. */
  #known = new Map<string, Set<string>>();
  /** What a notification click that started the app opens, once the inboxes are in. */
  #pendingReveal: { address: string; id: string | null } | null = null;
  #toastId = 0;

  current = $derived(this.inboxes.find((i) => i.address === this.selected) ?? null);
  currentMail = $derived(this.selected ? this.mail[this.selected] : undefined);
  visible = $derived.by(() => {
    const list = this.currentMail?.messages ?? [];
    const q = this.query.trim().toLowerCase();
    if (!q) return list;
    return list.filter((s) => s.from.toLowerCase().includes(q) || s.subject.toLowerCase().includes(q));
  });
  openedMessage = $derived(this.opened && "key" in this.opened ? this.opened : null);

  provider(id: string): ProviderInfo | undefined {
    return this.init?.providers.find((p) => p.id === id);
  }

  providerName(id: string): string {
    return this.provider(id)?.name ?? id;
  }

  /** The worker's latest word wins over the inbox list's (it may be older). */
  isExpired(inbox: InboxView): boolean {
    return this.mail[inbox.address]?.expired ?? inbox.expired;
  }

  isFresh(address: string, id: string): boolean {
    return this.fresh.has(freshKey(address, id));
  }

  // ---------- toasts ----------

  toast(kind: Toast["kind"], text: string, action?: Toast["action"], opens?: Toast["opens"]) {
    const id = ++this.#toastId;
    this.toasts = [...this.toasts.slice(-3), { id, kind, text, action, opens }];
    // A long notice or error (one naming a config file's path) stays long
    // enough to read: about 80 ms a character, up to 30 s; the × closes it
    // sooner. "New mail" keeps its 7 s, however long the subject.
    const base = kind === "error" ? 9000 : kind === "mail" ? 7000 : 3500;
    const ms = kind === "mail" ? base : Math.min(30000, Math.max(base, text.length * 80));
    window.setTimeout(() => this.dismiss(id), ms);
  }

  dismiss(id: number) {
    this.toasts = this.toasts.filter((t) => t.id !== id);
  }

  /**
   * Drops the "New mail" toast about this one message: it is open now (a
   * notification click, say). One that names other new mail too stays.
   */
  #dropMailToasts(address: string, id: string) {
    const about = (t: Toast) => t.opens?.only && t.opens.address === address && t.opens.id === id;
    if (this.toasts.some(about)) this.toasts = this.toasts.filter((t) => !about(t));
  }

  /** Runs a command, turning a failure into an error toast. */
  async #run<T>(p: Promise<T>): Promise<T | undefined> {
    try {
      return await p;
    } catch (e) {
      this.toast("error", errorText(e));
      return undefined;
    }
  }

  // ---------- inboxes ----------

  select(address: string) {
    if (this.selected === address) return;
    this.selected = address;
    this.query = "";
    this.closeMessage();
    void this.#run(api.selectInbox(address));
  }

  step(inboxDelta: number) {
    const n = this.inboxes.findIndex((i) => i.address === this.selected);
    const next = this.inboxes[Math.min(this.inboxes.length - 1, Math.max(0, n + inboxDelta))];
    if (next) this.select(next.address);
  }

  refresh() {
    if (!this.selected) return;
    void this.#run(api.selectInbox(this.selected).then(() => true)).then((ok) => {
      if (ok) this.toast("info", "Checking for new mail");
    });
  }

  async create(args: CreateArgs): Promise<boolean> {
    this.creating = true;
    this.createError = null;
    try {
      await api.createInbox(args);
      return true;
    } catch (e) {
      this.creating = false;
      this.toast("error", errorText(e));
      return false;
    }
  }

  async copyAddress(address = this.selected) {
    if (!address) return;
    const ok = await this.#run(api.copyAddress(address).then(() => true));
    if (ok) this.toast("success", "Address copied");
  }

  makeActive(address = this.selected) {
    if (!address) return;
    void this.#run(api.useInbox(address));
  }

  deleteInbox(address: string) {
    void this.#run(api.deleteInbox(address));
  }

  // ---------- messages ----------

  open(s: Summary) {
    if (this.selected) this.#openId(this.selected, s.id);
  }

  /** Opens a message of `address`, the selected inbox, listed or not yet (a notification click). */
  #openId(address: string, id: string) {
    // Open already; a failed open can be tried again.
    if (this.openId === id && this.opened && !("error" in this.opened)) return;
    this.openId = id;
    this.opened = null;
    this.images = false;
    this.original = false;
    this.source = false;
    this.fresh.delete(freshKey(address, id));
    this.#dropMailToasts(address, id);
    void this.#run(api.openMessage(address, id));
  }

  closeMessage() {
    this.openId = null;
    this.opened = null;
  }

  /** Moves through the visible messages and opens the one it lands on. */
  move(delta: number) {
    const list = this.visible;
    if (!list.length) return;
    const n = list.findIndex((s) => s.id === this.openId);
    const next = n < 0 ? (delta > 0 ? 0 : list.length - 1) : Math.min(list.length - 1, Math.max(0, n + delta));
    const s = list[next];
    if (s) this.open(s);
  }

  deleteMessage(address: string, id: string) {
    void this.#run(api.deleteMessage(address, id));
  }

  async copyFromMessage(part: "code" | "sender" | "link", index?: number): Promise<boolean> {
    const m = this.openedMessage;
    if (!m) return false;
    const ok = await this.#run(api.copyFromMessage(m.key, part, index).then(() => true));
    if (ok) this.toast("success", part === "code" ? "Code copied" : part === "sender" ? "Sender copied" : "Link copied");
    return !!ok;
  }

  openLink(index: number) {
    const m = this.openedMessage;
    if (m) void this.#run(api.openLink(m.key, index));
  }

  async saveAttachment(index: number) {
    const m = this.openedMessage;
    if (!m) return;
    const name = await this.#run(api.saveAttachment(m.key, index));
    if (name) this.toast("success", `Saved ${name}`);
  }

  /** Downloads an attachment for dragging, when the pointer reaches it. */
  prepareDrag(index: number) {
    const m = this.openedMessage;
    if (!m || !this.init?.drag) return;
    const k = `${m.key}/${index}`;
    if (k in this.dragReady) return;
    this.dragReady[k] = false;
    api.prepareAttachment(m.key, index).then(
      () => (this.dragReady[k] = true),
      (e) => {
        delete this.dragReady[k];
        this.toast("error", errorText(e));
      },
    );
  }

  dragOut(index: number) {
    const m = this.openedMessage;
    if (!m) return;
    api.dragAttachment(m.key, index).catch((e) => {
      // Its file may have been cleaned up: prepare it again next time.
      delete this.dragReady[`${m.key}/${index}`];
      this.toast("error", errorText(e));
    });
  }

  /** Opens an inbox and, if given, one of its messages (a notification was clicked). */
  reveal(address: string, id: string | null) {
    if (!this.inboxes.some((i) => i.address === address)) return;
    this.select(address);
    // Listed here or not yet: opened by id.
    if (id) this.#openId(address, id);
  }

  /** Opens what a notification click asked for before the inboxes were in. Returns whether it did. */
  #revealPending(): boolean {
    const r = this.#pendingReveal;
    if (!r || !this.inboxes.length || document.querySelector("dialog[open]")) return false;
    this.#pendingReveal = null;
    if (!this.inboxes.some((i) => i.address === r.address)) return false;
    this.reveal(r.address, r.id);
    return true;
  }

  // ---------- events ----------

  #onMessages(address: string, list: Summary[]) {
    const known = this.#known.get(address);
    if (known) {
      const arrived = list.filter((s) => !known.has(s.id));
      // Neither marked New nor in a toast if opened already (a notification click came first).
      const unopened = arrived.filter((s) => !s.seen && (address !== this.selected || s.id !== this.openId));
      for (const s of unopened) this.fresh.add(freshKey(address, s.id));
      const newest = unopened[0];
      if (newest) {
        const more = unopened.length > 1 ? ` and ${unopened.length - 1} more` : "";
        const text = `${senderName(newest.from)}: ${newest.subject || "(no subject)"}${more}`;
        const open = {
          label: "Open",
          run: () => {
            this.select(address);
            this.open(newest);
          },
        };
        this.toast("mail", text, open, { address, id: newest.id, only: unopened.length === 1 });
      }
    }
    this.#known.set(address, new Set(list.map((s) => s.id)));
  }

  listen(): () => void {
    const unlisten = [
      on("inboxes", (p) => {
        this.inboxes = p.inboxes;
        // A notification click's inbox first, so only that one is listed now.
        if (this.#revealPending()) return;
        if (!this.selected || !this.inboxes.some((i) => i.address === this.selected)) {
          const first = this.inboxes.find((i) => i.active) ?? this.inboxes[0];
          this.selected = null;
          this.closeMessage();
          if (first) this.select(first.address);
        }
      }),
      on("messages", (p) => {
        const old = this.mail[p.address];
        const messages = old?.messages ?? [];
        if ("messages" in p) {
          this.#onMessages(p.address, p.messages);
          this.mail[p.address] = { messages: p.messages, loaded: true, live: p.live, error: null, expired: false };
        } else if ("error" in p) {
          this.mail[p.address] = { messages, loaded: true, live: p.live, error: p.error, expired: false };
        } else {
          this.mail[p.address] = { messages, loaded: true, live: false, error: null, expired: true };
        }
      }),
      on("opened", (p) => {
        // Opened means read: the Rust side saves it for the CLI and TUI too.
        if ("key" in p) {
          const s = this.mail[p.address]?.messages.find((m) => m.id === p.id);
          if (s) s.seen = true;
        }
        if (p.address === this.selected && p.id === this.openId) this.opened = p;
      }),
      on("created", (p) => {
        if (p.from_tray) {
          // The window's own create (if any) is still running: leave it be.
          if (!document.querySelector("dialog[open]")) this.select(p.address);
          return;
        }
        this.creating = false;
        this.newInboxOpen = false;
        this.selected = null;
        this.select(p.address);
        this.toast("success", `Inbox ready: ${p.address}`, {
          label: "Copy",
          run: () => void this.copyAddress(p.address),
        });
      }),
      on("deleted", (p) => {
        const m = this.mail[p.address];
        if (m) m.messages = m.messages.filter((s) => s.id !== p.id);
        this.#known.get(p.address)?.delete(p.id);
        if (p.address === this.selected && p.id === this.openId) this.closeMessage();
      }),
      on("create_failed", (p) => {
        this.creating = false;
        // A refused field is shown by it in the dialog, if that is open.
        if (p.field && this.newInboxOpen) this.createError = { field: p.field, text: p.text };
        else this.toast("error", p.text);
      }),
      on("notice", (p) => this.toast(p.kind, p.text)),
      on("reveal", (p) => {
        if (document.querySelector("dialog[open]")) return;
        // Before the inboxes are in: once they are.
        if (!this.inboxes.length) this.#pendingReveal = p;
        else this.reveal(p.address, p.id);
      }),
    ];
    // Listen first, then ask for the data, so the first events aren't lost.
    void Promise.all(unlisten)
      .then(() => api.init())
      .then((i) => {
        this.init = i;
        if (i.reveal) this.#pendingReveal = i.reveal;
        this.#revealPending();
      })
      .catch((e) => (this.startError = errorText(e)));
    const tick = window.setInterval(() => (this.now = Date.now()), 30000);
    return () => {
      window.clearInterval(tick);
      // The page may not unlisten in Rust (see capabilities/main.json):
      // this only drops the handlers here, and the refused call is ignored.
      void Promise.all(unlisten).then((fns) =>
        fns.forEach((f) => void Promise.resolve(f() as unknown).catch(() => {})),
      );
    };
  }
}

export const app = new AppState();
