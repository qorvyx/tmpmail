<script lang="ts">
  import { onMount } from "svelte";
  import logo from "./assets/logo.svg";
  import ConfirmDialog from "./lib/ConfirmDialog.svelte";
  import HelpDialog from "./lib/HelpDialog.svelte";
  import MessageList from "./lib/MessageList.svelte";
  import MessageView from "./lib/MessageView.svelte";
  import NewInboxDialog from "./lib/NewInboxDialog.svelte";
  import ProbeView from "./lib/ProbeView.svelte";
  import Resizer from "./lib/Resizer.svelte";
  import SettingsDialog from "./lib/SettingsDialog.svelte";
  import Sidebar from "./lib/Sidebar.svelte";
  import Toasts from "./lib/Toasts.svelte";
  import Welcome from "./lib/Welcome.svelte";
  import { app } from "./lib/store.svelte";

  const WIDTHS = "tmpmail.panes";
  function savedWidths(): [number, number] {
    try {
      const v = JSON.parse(localStorage.getItem(WIDTHS) ?? "null");
      if (Array.isArray(v) && v.length === 2 && v.every((n) => typeof n === "number")) return [v[0] as number, v[1] as number];
    } catch {
      // Defaults below.
    }
    return [256, 360];
  }
  const [w1, w2] = savedWidths();
  let sidebar = $state(w1);
  let list = $state(w2);
  $effect(() => {
    try {
      localStorage.setItem(WIDTHS, JSON.stringify([sidebar, list]));
    } catch {
      // Not remembered.
    }
  });

  // In a narrow window the message list is drawn narrower than its saved
  // width (not below LIST_MIN), so the open message keeps MESSAGE_MIN; the
  // saved width comes back when the window grows.
  const LIST_MIN = 280;
  const LIST_MAX = 560;
  const MESSAGE_MIN = 340;
  let windowWidth = $state(window.innerWidth);
  const room = $derived(windowWidth - sidebar - MESSAGE_MIN);
  const listShown = $derived(Math.max(LIST_MIN, Math.min(list, room)));

  let search = $state<HTMLInputElement>();

  onMount(() => app.listen());

  /** The TUI's keys. Off while typing in a field or while a dialog is open. */
  function onkey(e: KeyboardEvent) {
    // Ctrl+, (Cmd+, on macOS) opens the settings, from anywhere.
    if (e.key === "," && (e.ctrlKey || e.metaKey) && !e.altKey && app.init && !app.init.probe) {
      e.preventDefault();
      if (!document.querySelector("dialog[open]")) app.settingsOpen = true;
      return;
    }
    if (e.ctrlKey || e.metaKey || e.altKey || e.defaultPrevented) return;
    const t = e.target as HTMLElement | null;
    if (t?.closest("input, textarea, select, [contenteditable]")) return;
    if (document.querySelector("dialog[open]")) return;
    if (!app.init) return;
    const m = app.openedMessage;
    const inbox = app.current;
    const caps = inbox ? app.provider(inbox.provider)?.caps : undefined;
    const handled = (() => {
      switch (e.key) {
        case "j":
        case "ArrowDown":
          app.move(1);
          return true;
        case "k":
        case "ArrowUp":
          app.move(-1);
          return true;
        case "[":
          app.step(-1);
          return true;
        case "]":
          app.step(1);
          return true;
        case "n":
          if (!app.creating) void app.create({ provider: null, user: null, domain: null, label: null });
          return true;
        case "N":
          app.newInboxOpen = true;
          return true;
        case "y":
          if (m?.code) void app.copyFromMessage("code");
          else if (m) app.toast("info", "No code found in this message");
          return true;
        case "L":
          if (m?.links.length) void app.copyFromMessage("link", 0);
          return true;
        case "O":
          if (m?.links.length) app.openLink(0);
          return true;
        case "c":
          void app.copyAddress();
          return true;
        case "a":
          app.makeActive();
          return true;
        case "r":
          app.refresh();
          return true;
        case "d":
          if (m && caps?.delete_message) app.confirm = { kind: "message", address: m.address, id: m.id, subject: m.subject };
          else if (m) app.toast("info", `${app.providerName(inbox?.provider ?? "")} can't delete messages`);
          return true;
        case "D":
          if (inbox) app.confirm = { kind: "inbox", address: inbox.address };
          return true;
        case "/":
          search?.focus();
          return true;
        case "?":
          app.helpOpen = true;
          return true;
      }
      return false;
    })();
    if (handled) e.preventDefault();
  }
</script>

<svelte:window onkeydown={onkey} bind:innerWidth={windowWidth} />

{#if app.startError}
  <main class="grid h-full place-items-center p-8">
    <div class="max-w-md text-center">
      <img src={logo} alt="" width="48" height="48" class="mx-auto rounded-xl" />
      <h1 class="mt-4 text-lg font-semibold tracking-title">tmpmail couldn't start</h1>
      <p class="selectable mt-2 rounded-lg bg-danger-soft px-4 py-3 text-xs break-words text-danger">{app.startError}</p>
      <p class="mt-3 text-xs text-muted">
        Fix or move the file it names (the config is read from <code class="font-mono">TMPMAIL_CONFIG</code> or
        <code class="font-mono">~/.config/tmpmail/config.toml</code>), then open tmpmail again.
      </p>
    </div>
  </main>
{:else if app.init?.probe}
  <ProbeView origin={app.init.origin} plan={app.init.probe} />
{:else}
  <!-- The message list gives way (down to its minimum) so the open message
       keeps MESSAGE_MIN where it can in a narrow window. -->
  <div class="grid h-full" style:grid-template-columns="{sidebar}px {listShown}px minmax(0,1fr)">
    <div class="relative min-h-0">
      <Sidebar />
      <Resizer bind:width={sidebar} min={200} max={360} label="Resize the inbox list" />
    </div>
    {#if app.init && !app.inboxes.length}
      <Welcome />
    {:else}
      <div class="relative min-h-0">
        <MessageList bind:search />
        <Resizer
          bind:width={() => listShown, (w) => (list = w)}
          min={LIST_MIN}
          max={Math.max(LIST_MIN, Math.min(LIST_MAX, room))}
          label="Resize the message list"
        />
      </div>
      <MessageView />
    {/if}
  </div>
  <NewInboxDialog />
  <ConfirmDialog />
  <HelpDialog />
  <SettingsDialog />
{/if}

<Toasts />
