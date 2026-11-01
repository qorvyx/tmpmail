<script lang="ts">
  import { ago, reveal, senderName, until } from "./api";
  import Icon from "./Icon.svelte";
  import { app } from "./store.svelte";

  let { search = $bindable() }: { search?: HTMLInputElement } = $props();

  const inbox = $derived(app.current);
  const m = $derived(app.currentMail);
  const caps = $derived(inbox ? app.provider(inbox.provider)?.caps : undefined);
  const expired = $derived(inbox ? app.isExpired(inbox) : false);
  const left = $derived(inbox ? until(inbox.expires_at, app.now) : null);

  // The open message stays in view when j/k (or anything else) opens
  // another; filtering or new mail doesn't move the list.
  let rows = $state<HTMLUListElement>();
  $effect(() => {
    if (app.openId) reveal(rows, rows?.querySelector('[aria-current="true"]'));
  });
</script>

<section class="flex h-full min-h-0 flex-col bg-raised" aria-label="Messages">
  {#if inbox}
    <header class="border-b border-rule px-4 pt-4 pb-3">
      <button
        class="group flex w-full min-w-0 items-center gap-2 rounded-md text-left"
        title="{inbox.address} (click to copy)"
        onclick={() => app.copyAddress(inbox.address)}
      >
        <span class="selectable min-w-0 truncate text-lg leading-tight font-semibold tracking-title">
          {inbox.address}
        </span>
        <Icon name="copy" size={15} class="text-faint group-hover:text-ink" />
      </button>
      <div class="mt-1.5 flex flex-wrap items-center gap-x-3 gap-y-1 text-2xs text-muted">
        <span>{app.providerName(inbox.provider)}</span>
        {#if expired}
          <span class="text-danger">Expired</span>
        {:else if m?.live}
          <span class="inline-flex items-center gap-1.5 text-live"><span class="live-dot"></span>Live</span>
        {/if}
        {#if !expired && left}
          <span class="inline-flex items-center gap-1"><Icon name="clock" size={12} />Expires {left}</span>
        {:else if !expired && caps}
          <span class="inline-flex min-w-0 items-center gap-1" title="How long {app.providerName(inbox.provider)} keeps it">
            <Icon name="clock" size={12} /><span class="truncate">{caps.lifetime}</span>
          </span>
        {/if}
        {#if !inbox.private}
          <span class="inline-flex items-center gap-1 text-danger"><Icon name="globe" size={12} />Public: anyone can read it</span>
        {/if}
      </div>
      <div class="mt-3 flex items-center gap-1.5">
        <button class="btn" onclick={() => app.copyAddress(inbox.address)} title="Copy the address (c)">
          <Icon name="copy" size={14} />Copy
        </button>
        {#if inbox.active}
          <span class="inline-flex h-[30px] items-center gap-1.5 px-2 text-xs font-semibold text-muted" title="The CLI uses this inbox when you don't name one">
            <Icon name="star" size={14} class="fill-current text-hi-edge" />Active
          </span>
        {:else}
          <button class="btn" onclick={() => app.makeActive(inbox.address)} title="Make it the CLI's default inbox (a)">
            <Icon name="star" size={14} />Make active
          </button>
        {/if}
        <span class="flex-1"></span>
        <button class="btn btn-ghost btn-icon" onclick={() => app.refresh()} title="Check now (r)" aria-label="Check now">
          <Icon name="refresh" size={15} />
        </button>
        <button
          class="btn btn-ghost btn-icon hover:!text-danger"
          onclick={() => (app.confirm = { kind: "inbox", address: inbox.address })}
          title="Delete inbox (D)"
          aria-label="Delete inbox"
        >
          <Icon name="trash" size={15} />
        </button>
      </div>
    </header>

    {#if m?.messages.length}
      <div class="border-b border-rule px-3 py-2">
        <label class="relative block">
          <span class="sr-only">Filter messages</span>
          <Icon name="search" size={14} class="pointer-events-none absolute top-1/2 left-2.5 -translate-y-1/2 text-faint" />
          <input
            bind:this={search}
            class="field !h-8 !pl-8 text-xs"
            type="search"
            placeholder="Filter by sender or subject  /"
            bind:value={app.query}
            onkeydown={(e) => {
              if (e.key === "Escape") {
                app.query = "";
                e.currentTarget.blur();
              }
            }}
          />
        </label>
      </div>
    {/if}

    {#if m?.error}
      <div class="mx-3 mt-3 flex items-start gap-2.5 rounded-lg bg-danger-soft px-3 py-2.5 text-xs text-danger" role="alert">
        <Icon name="alert" size={15} class="mt-0.5" />
        <div class="min-w-0 flex-1">
          <p class="font-semibold">{app.providerName(inbox.provider)} isn't answering</p>
          <p class="selectable mt-0.5 break-words opacity-90">{m.error}</p>
          <p class="mt-1 opacity-90">tmpmail keeps trying on its own.</p>
        </div>
        <button class="btn !h-7 shrink-0" onclick={() => app.refresh()}>Retry</button>
      </div>
    {/if}

    <ul bind:this={rows} class="min-h-0 flex-1 overflow-y-auto" aria-label="Messages in {inbox.address}">
      {#each app.visible as s (s.id)}
        {@const open = s.id === app.openId}
        {@const fresh = app.isFresh(inbox.address, s.id)}
        <li class="border-b border-rule">
          <button
            class={[
              "relative flex w-full flex-col gap-0.5 py-2.5 pr-4 pl-6 text-left transition-colors",
              open ? "bg-hi-soft" : "hover:bg-hover",
              fresh && !open && "[animation:arrive_2.4s_var(--ease-out)]",
            ]}
            aria-current={open}
            onclick={() => app.open(s)}
          >
            {#if !s.seen || fresh}
              <span class="absolute top-[17px] left-2.5 h-2 w-2 rounded-full bg-hi-edge" title="Unread"></span>
            {/if}
            <span class="flex w-full items-baseline gap-2">
              <span class={["min-w-0 flex-1 truncate text-xs", !s.seen || fresh ? "font-bold" : "font-medium"]}>
                {senderName(s.from)}
              </span>
              {#if fresh}
                <span class="rounded bg-hi px-1.5 text-[10.5px] leading-4 font-bold text-hi-ink">New</span>
              {/if}
              <span class="shrink-0 text-2xs text-faint">{ago(s.date, app.now)}</span>
            </span>
            <span class="flex w-full items-center gap-1.5">
              <span class={["min-w-0 flex-1 truncate text-xs", s.seen && !fresh ? "text-muted" : "text-ink"]}>
                {s.subject || "(no subject)"}
              </span>
              {#if s.has_attachments}
                <span title="Has attachments" class="text-faint"><Icon name="paperclip" size={13} /></span>
              {/if}
            </span>
            {#if s.intro}
              <span class="w-full truncate text-2xs text-faint">{s.intro}</span>
            {/if}
          </button>
        </li>
      {:else}
        <li class="px-6 py-10">
          {#if expired}
            <p class="text-md font-semibold">This inbox has expired</p>
            <p class="mt-1 text-xs text-muted">No more mail will arrive. Delete it, or make a new one.</p>
            <div class="mt-4 flex gap-2">
              <button class="btn" onclick={() => (app.confirm = { kind: "inbox", address: inbox.address })}>
                <Icon name="trash" size={14} />Delete
              </button>
              <button class="btn btn-primary" onclick={() => (app.newInboxOpen = true)}><Icon name="plus" size={14} />New inbox</button>
            </div>
          {:else if app.query && m?.messages.length}
            <p class="text-xs text-muted">Nothing from or about “{app.query}”.</p>
          {:else if m?.loaded && !m.error}
            <div class="flex items-center gap-2.5">
              <span class="live-dot"></span>
              <p class="text-md font-semibold">Waiting for mail</p>
            </div>
            <p class="mt-1.5 text-xs text-muted">
              Use this address anywhere. New mail shows up here by itself{m.live ? ", the moment it lands" : ""}.
            </p>
            <button class="btn btn-primary mt-4" onclick={() => app.copyAddress(inbox.address)}>
              <Icon name="copy" size={14} />Copy address
            </button>
          {:else if !m?.error}
            <div class="space-y-3" aria-label="Checking for mail">
              {#each [0, 1, 2] as i (i)}
                <div class="space-y-1.5 opacity-70">
                  <div class="h-3 w-2/5 rounded bg-sunken"></div>
                  <div class="h-3 w-4/5 rounded bg-sunken"></div>
                </div>
              {/each}
            </div>
          {/if}
        </li>
      {/each}
    </ul>
  {:else if app.init}
    <div class="grid h-full place-items-center p-8 text-center">
      <div>
        <p class="text-lg font-semibold tracking-title">Make your first address</p>
        <p class="mt-1 text-xs text-muted">A throwaway inbox that catches verification codes.</p>
        <button class="btn btn-primary mt-4" onclick={() => (app.newInboxOpen = true)}><Icon name="plus" size={14} />New inbox</button>
      </div>
    </div>
  {/if}
</section>
