<script lang="ts">
  import { fileKind, fileSize, senderName } from "./api";
  import CodeBanner from "./CodeBanner.svelte";
  import Icon from "./Icon.svelte";
  import Preview from "./Preview.svelte";
  import { app } from "./store.svelte";
  import { theme } from "./theme.svelte";

  const m = $derived(app.openedMessage);
  const inbox = $derived(app.current);
  const caps = $derived(inbox ? app.provider(inbox.provider)?.caps : undefined);
  let allLinks = $state(false);
  const links = $derived(m ? (allLinks ? m.links : m.links.slice(0, 1)) : []);

  $effect(() => {
    void m?.key;
    allLinks = false;
  });

  function parts(link: string): { host: string; rest: string } {
    try {
      const u = new URL(link);
      return { host: u.host, rest: `${u.pathname}${u.search}`.replace(/^\/$/, "") };
    } catch {
      return { host: link, rest: "" };
    }
  }

  function when(iso: string | null): string {
    if (!iso) return "";
    return new Date(iso).toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" });
  }
</script>

<section class="@container flex h-full min-h-0 min-w-0 flex-col bg-raised" aria-label="Message">
  {#if m && app.init}
    <div class="max-h-[58%] shrink-0 overflow-y-auto border-b border-rule">
      <header class="px-6 pt-5 pb-3">
        <h2 class="selectable text-xl leading-tight font-semibold tracking-[-0.02em] break-words">
          {m.subject || "(no subject)"}
        </h2>
        <div class="mt-3 flex flex-wrap items-center gap-x-3 @sm:flex-nowrap">
          <span
            class="grid h-9 w-9 shrink-0 place-items-center rounded-full bg-sunken text-sm font-bold text-muted"
            aria-hidden="true">{senderName(m.from).trim().charAt(0).toUpperCase() || "?"}</span
          >
          <div class="min-w-0 flex-1">
            <!-- A narrow pane: the address (with its copy button) wraps
                 under the name instead of both being cut to a few letters. -->
            <p class="flex min-w-0 flex-wrap items-baseline gap-x-2 @sm:flex-nowrap">
              <span class="selectable max-w-full truncate text-sm font-semibold" title={m.from}>{senderName(m.from)}</span>
              <span class="flex min-w-0 max-w-full items-baseline gap-2">
                {#if m.sender !== senderName(m.from)}
                  <span class="selectable truncate text-xs text-muted">{m.sender}</span>
                {/if}
                <button
                  class="shrink-0 self-center text-faint hover:text-ink"
                  title="Copy the sender's address"
                  aria-label="Copy the sender's address"
                  onclick={() => app.copyFromMessage("sender")}><Icon name="copy" size={13} /></button
                >
              </span>
            </p>
            <p class="truncate text-2xs text-muted" title={m.to.join(", ")}>
              To {m.to.join(", ") || inbox?.address}
            </p>
          </div>
          <!-- A narrow pane: the date gets a line of its own, under the
               sender, instead of squeezing the sender's line. -->
          <time
            class="basis-full truncate pl-12 text-2xs text-muted @sm:basis-auto @sm:shrink-0 @sm:self-start @sm:pl-0"
            datetime={m.date ?? undefined}>{when(m.date)}</time
          >
        </div>

        <div class="mt-3 -ml-2.5 flex flex-wrap items-center gap-1" role="toolbar" aria-label="Message view">
          {#if m.html}
            <button
              class="btn btn-ghost"
              aria-pressed={app.images}
              disabled={app.source}
              onclick={() => (app.images = !app.images)}
              title="Load remote images, over https only. The sender can see that you opened the mail."
            >
              <Icon name="image" size={14} />{app.images ? "Images on (https)" : "Load images (https)"}
            </button>
            {#if theme.resolved === "dark"}
              <button
                class="btn btn-ghost"
                aria-pressed={app.original}
                disabled={app.source}
                onclick={() => (app.original = !app.original)}
                title="Show the mail in the sender's own colors"
              >
                <Icon name="contrast" size={14} />Original colors
              </button>
            {/if}
          {/if}
          {#if caps?.raw || m.html}
            <button
              class="btn btn-ghost"
              aria-pressed={app.source}
              onclick={() => (app.source = !app.source)}
              title={caps?.raw ? "Show the raw message source" : "Show the HTML source"}
            >
              <Icon name="code" size={14} />Source
            </button>
          {/if}
          <span class="flex-1"></span>
          {#if caps?.delete_message}
            <button
              class="btn btn-ghost hover:!text-danger"
              onclick={() => (app.confirm = { kind: "message", address: m.address, id: m.id, subject: m.subject })}
              title="Delete the message (d)"
            >
              <Icon name="trash" size={14} />Delete
            </button>
          {/if}
        </div>
      </header>

      {#if m.code}
        <div class="px-6 pb-4">
          <CodeBanner code={m.code} onCopy={() => app.copyFromMessage("code")} />
        </div>
      {/if}

      {#if m.links.length}
        <div class="px-6 pb-4">
          <h3 class="mb-1.5 text-2xs font-semibold text-muted">
            {m.links.length === 1 ? "Link" : "Links, the likeliest verification link first"}
          </h3>
          <ul class="divide-y divide-rule overflow-hidden rounded-lg border border-rule">
            {#each links as link, i (link)}
              {@const p = parts(link)}
              <!-- A narrow pane: the address takes the whole first line (two
                   at most, broken anywhere) and the buttons go under it. -->
              <li class={["flex flex-wrap items-center justify-end gap-x-3 gap-y-1.5 px-3 py-2 @sm:flex-nowrap", i === 0 && "bg-sunken"]}>
                <span class="flex min-w-0 basis-full items-start gap-3 @sm:flex-1 @sm:basis-auto">
                  <Icon name="link" size={14} class="mt-0.5 shrink-0 text-faint" />
                  <span class="selectable line-clamp-2 min-w-0 flex-1 text-xs break-all @sm:line-clamp-1" title={link}>
                    <span class="font-semibold">{p.host}</span><span class="text-muted">{p.rest}</span>
                  </span>
                </span>
                <button class={["btn !h-7", i === 0 && "btn-primary"]} onclick={() => app.openLink(i)} title="Open in your browser">
                  <Icon name="external" size={13} />Open
                </button>
                <button class="btn btn-ghost btn-icon !h-7" onclick={() => app.copyFromMessage("link", i)} title="Copy link" aria-label="Copy link">
                  <Icon name="copy" size={13} />
                </button>
              </li>
            {/each}
          </ul>
          {#if m.links.length > 1}
            <button class="mt-1.5 text-2xs font-semibold text-muted hover:text-ink" onclick={() => (allLinks = !allLinks)}>
              {allLinks ? "Show only the first link" : `Show ${m.links.length - 1} more ${m.links.length === 2 ? "link" : "links"}`}
            </button>
          {/if}
        </div>
      {/if}

      {#if caps?.attachments && m.attachments.length}
        <div class="px-6 pb-4">
          <h3 class="mb-1.5 text-2xs font-semibold text-muted">Attachments</h3>
          <ul class="flex flex-wrap gap-2">
            {#each m.attachments as a, i (i)}
              {@const ready = app.dragReady[`${m.key}/${i}`]}
              <li class="flex max-w-full items-center gap-2.5 rounded-lg border border-rule py-1.5 pr-1.5 pl-3">
                {#if app.init.drag}
                  <!-- Dragging this starts a drag of the file itself; Rust
                       downloads it first, when the pointer gets here. -->
                  <span
                    class={["-ml-1.5 grid h-7 w-5 place-items-center rounded text-faint", ready ? "cursor-grab hover:bg-hover hover:text-ink" : "cursor-progress"]}
                    draggable="true"
                    role="img"
                    aria-label="Drag {a.filename} out"
                    title={ready ? "Drag the file to a folder or another app" : ready === false ? "Getting the file ready to drag…" : "Drag the file out"}
                    onpointerenter={() => app.prepareDrag(i)}
                    ondragstart={(e) => {
                      e.preventDefault();
                      if (ready) app.dragOut(i);
                      else app.prepareDrag(i);
                    }}
                  >
                    <Icon name="grip" size={14} />
                  </span>
                {:else}
                  <Icon name="paperclip" size={14} class="text-faint" />
                {/if}
                <span class="min-w-0">
                  <span class="block max-w-[16rem] truncate text-xs font-semibold" title={a.filename}>{a.filename}</span>
                  <span class="block text-2xs text-muted">{fileKind(a.content_type, a.filename)}{a.size != null ? `, ${fileSize(a.size)}` : ""}</span>
                </span>
                <button class="btn btn-ghost btn-icon !h-7" onclick={() => app.saveAttachment(i)} title="Save…" aria-label="Save {a.filename}">
                  <Icon name="download" size={14} />
                </button>
              </li>
            {/each}
          </ul>
        </div>
      {/if}
    </div>
    <div class="min-h-0 flex-1">
      <Preview
        origin={app.init.origin}
        mailKey={m.key}
        images={app.images}
        theme={theme.resolved}
        original={app.original}
        source={app.source}
        title="Message body"
      />
    </div>
  {:else if app.opened && "error" in app.opened}
    <div class="m-6 flex items-start gap-2.5 rounded-lg bg-danger-soft px-4 py-3 text-xs text-danger" role="alert">
      <Icon name="alert" size={15} class="mt-0.5" />
      <div>
        <p class="font-semibold">Couldn't open this message</p>
        <p class="selectable mt-0.5">{app.opened.error}</p>
        <p class="mt-1">Refresh the inbox and try again.</p>
      </div>
    </div>
  {:else if app.openId}
    <div class="space-y-4 px-6 pt-6" aria-label="Opening the message">
      <div class="h-6 w-3/5 rounded bg-sunken"></div>
      <div class="flex items-center gap-3">
        <div class="h-9 w-9 rounded-full bg-sunken"></div>
        <div class="h-3 w-48 rounded bg-sunken"></div>
      </div>
      <div class="h-24 rounded-xl bg-sunken"></div>
    </div>
  {:else}
    <div class="grid h-full place-items-center p-8 text-center text-muted">
      <div>
        <Icon name="mail" size={28} class="mx-auto text-rule-strong" />
        <p class="mt-3 text-xs">
          {app.currentMail?.messages.length ? "Pick a message to read it." : "Messages you open show up here."}
        </p>
        {#if app.currentMail?.messages.length}
          <p class="mt-1 text-2xs">Or press <kbd>j</kbd> for the newest.</p>
        {/if}
      </div>
    </div>
  {/if}
</section>
