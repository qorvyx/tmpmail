<script lang="ts">
  import logo from "../assets/logo.svg";
  import { reveal, until } from "./api";
  import Icon from "./Icon.svelte";
  import { app } from "./store.svelte";
  import { theme, type ThemePref } from "./theme.svelte";

  const mod = /Mac/.test(navigator.platform) ? "⌘" : "Ctrl";

  // The selected inbox stays in view when [/] (or anything else) picks it.
  let list = $state<HTMLUListElement>();
  $effect(() => {
    if (app.selected) reveal(list, list?.querySelector('[aria-current="true"]'));
  });

  const prefs: { value: ThemePref; icon: "monitor" | "sun" | "moon"; label: string }[] = [
    { value: "system", icon: "monitor", label: "System theme" },
    { value: "light", icon: "sun", label: "Light theme" },
    { value: "dark", icon: "moon", label: "Dark theme" },
  ];
</script>

<nav class="flex h-full min-h-0 flex-col bg-panel" aria-label="Inboxes">
  <header class="flex items-center gap-2.5 px-4 pt-4 pb-3">
    <img src={logo} alt="" width="28" height="28" class="rounded-[7px]" />
    <span class="text-lg leading-none font-bold tracking-[-0.025em]">tmpmail</span>
  </header>

  <div class="px-3 pb-3">
    <button class="btn btn-primary w-full" onclick={() => (app.newInboxOpen = true)} disabled={app.creating}>
      <Icon name="plus" />
      {app.creating ? "Creating…" : "New inbox"}
    </button>
  </div>

  <ul bind:this={list} class="min-h-0 flex-1 overflow-y-auto px-2 pb-2">
    {#each app.inboxes as inbox (inbox.address)}
      {@const m = app.mail[inbox.address]}
      {@const expired = app.isExpired(inbox)}
      {@const selected = inbox.address === app.selected}
      {@const unread = m?.messages.filter((s) => !s.seen || app.isFresh(inbox.address, s.id)).length ?? 0}
      {@const left = until(inbox.expires_at, app.now)}
      <li>
        <button
          class={[
            "group relative flex w-full flex-col gap-0.5 rounded-lg py-2 pr-2.5 pl-3.5 text-left transition-colors",
            selected ? "bg-hi-soft" : "hover:bg-hover",
            expired && "opacity-60",
          ]}
          aria-current={selected}
          title={inbox.address}
          onclick={() => app.select(inbox.address)}
        >
          {#if selected}
            <span class="absolute top-2 bottom-2 left-1 w-[3px] rounded-full bg-hi-edge" aria-hidden="true"></span>
          {/if}
          <span class="flex w-full items-center gap-1.5">
            <span class="min-w-0 flex-1 truncate text-xs font-semibold">{inbox.label ?? inbox.address}</span>
            {#if inbox.active}
              <span class="text-hi-edge" title="Active inbox: the CLI's default">
                <Icon name="star" size={13} class="fill-current" />
              </span>
            {/if}
            {#if unread}
              <span class="rounded-full bg-ink px-1.5 text-[11px] leading-[18px] font-semibold text-raised">{unread}</span>
            {/if}
          </span>
          {#if inbox.label}
            <span class="w-full truncate text-2xs text-muted">{inbox.address}</span>
          {/if}
          <span class="flex w-full items-center gap-1.5 text-2xs text-muted">
            <span class="truncate">{app.providerName(inbox.provider)}</span>
            {#if expired}
              <span class="text-danger">expired</span>
            {:else if m?.live}
              <span class="inline-flex items-center gap-1 text-live"><span class="h-1.5 w-1.5 rounded-full bg-live"></span>live</span>
            {:else if m?.error}
              <span class="text-danger">not answering</span>
            {:else if left}
              <span class="shrink-0">expires {left}</span>
            {/if}
          </span>
        </button>
      </li>
    {:else}
      {#if app.init}
        <li class="px-3 py-4 text-xs text-muted">No inboxes yet.</li>
      {/if}
    {/each}
  </ul>

  <footer class="flex items-center justify-between gap-2 border-t border-rule px-3 py-2.5">
    <div class="flex rounded-lg border border-rule bg-sunken p-0.5" role="radiogroup" aria-label="Theme">
      {#each prefs as p (p.value)}
        <button
          class={[
            "grid h-7 w-8 place-items-center rounded-md transition-colors",
            theme.pref === p.value ? "bg-raised text-ink shadow-sm" : "text-muted hover:text-ink",
          ]}
          role="radio"
          aria-checked={theme.pref === p.value}
          aria-label={p.label}
          title={p.label}
          onclick={() => theme.set(p.value)}
        >
          <Icon name={p.icon} size={15} />
        </button>
      {/each}
    </div>
    <span class="flex gap-0.5">
      <button class="btn btn-ghost btn-icon" onclick={() => (app.helpOpen = true)} title="Keyboard shortcuts (?)" aria-label="Keyboard shortcuts">
        <Icon name="keyboard" />
      </button>
      <button class="btn btn-ghost btn-icon" onclick={() => (app.settingsOpen = true)} title="Settings ({mod}+,)" aria-label="Settings">
        <Icon name="settings" />
      </button>
    </span>
  </footer>
</nav>
