<script lang="ts">
  import { api, reveal } from "./api";
  import Icon from "./Icon.svelte";
  import { app } from "./store.svelte";

  let dialog: HTMLDialogElement;
  let userInput: HTMLInputElement;
  let labelInput: HTMLInputElement;
  let provider = $state("");
  let user = $state("");
  let domain = $state("");
  let label = $state("");
  let domains = $state<{ provider: string; list: string[] | null; error: string | null } | null>(null);

  const picked = $derived(provider ? app.provider(provider) : undefined);
  const canUser = $derived(!picked || picked.caps.custom_user);
  const canDomain = $derived(!!picked?.caps.choose_domain);

  $effect(() => {
    if (app.newInboxOpen && !dialog.open) {
      user = "";
      domain = "";
      label = "";
      app.createError = null;
      dialog.showModal();
    } else if (!app.newInboxOpen && dialog.open) {
      dialog.close();
    }
  });

  // The domain list of the picked provider, fetched when it's picked.
  $effect(() => {
    const id = provider;
    domain = "";
    if (!id || !canDomain) {
      domains = null;
      return;
    }
    domains = { provider: id, list: null, error: null };
    api.domains(id).then(
      (list) => {
        if (domains?.provider === id) domains = { provider: id, list, error: null };
      },
      (e) => {
        if (domains?.provider === id) domains = { provider: id, list: null, error: String(e) };
      },
    );
  });

  // Not once the picked provider chooses the username itself: it isn't sent.
  const userError = $derived(canUser && app.createError?.field === "user" ? app.createError.text : null);
  const labelError = $derived(app.createError?.field === "label" ? app.createError.text : null);

  // Put the cursor in the refused field.
  $effect(() => {
    if (userError) userInput?.focus();
    else if (labelError) labelInput?.focus();
  });

  // The reason under a refused field scrolls into view with it: on a short
  // window (1280×800 with a panel) the label's line sat below the visible
  // part of the dialog's body. Run when the line appears.
  const shown = (line: HTMLElement) => {
    requestAnimationFrame(() => reveal(body, line));
  };
  let body = $state<HTMLElement>();

  // Editing the refused field clears its message.
  function edited(field: "user" | "label") {
    if (app.createError?.field === field) app.createError = null;
  }

  async function submit(e: SubmitEvent) {
    e.preventDefault();
    await app.create({
      provider: provider || null,
      user: canUser ? user : null,
      domain: canDomain ? domain : null,
      label,
    });
  }
</script>

<dialog
  bind:this={dialog}
  class="w-[min(34rem,calc(100vw-2rem))]"
  aria-labelledby="new-inbox-title"
  onclose={() => (app.newInboxOpen = false)}
>
  <form class="flex min-h-0 flex-col" onsubmit={submit}>
    <header class="flex shrink-0 items-start justify-between gap-4 px-6 pt-5">
      <div>
        <h2 id="new-inbox-title" class="text-lg font-semibold tracking-title">New inbox</h2>
        <p class="mt-0.5 text-xs text-muted">Pick a provider, or let tmpmail use the first one that answers.</p>
      </div>
      <button type="button" class="btn btn-ghost btn-icon -mr-2" aria-label="Close" onclick={() => (app.newInboxOpen = false)}>
        <Icon name="x" />
      </button>
    </header>

    <div bind:this={body} class="min-h-0 flex-1 space-y-4 overflow-y-auto px-6 py-4">
      <fieldset>
        <legend class="mb-2 text-xs font-semibold">Provider</legend>
        <div class="divide-y divide-rule overflow-hidden rounded-lg border border-rule">
          {#each [{ id: "", name: "First that works", caps: null }, ...(app.init?.providers ?? [])] as p (p.id)}
            <label
              class={[
                "flex items-center gap-3 px-3 py-2 transition-colors",
                provider === p.id ? "bg-hi-soft" : "hover:bg-hover",
              ]}
            >
              <!-- svelte-ignore a11y_autofocus -->
              <input
                type="radio"
                name="provider"
                value={p.id}
                bind:group={provider}
                autofocus={provider === p.id}
                class="accent-[var(--color-ink)]"
              />
              <span class="min-w-0 flex-1">
                <span class="block text-xs font-semibold">{p.name}</span>
                <span class="block truncate text-2xs text-muted">
                  {p.caps ? p.caps.lifetime : "Tries each provider in your config's order"}
                </span>
              </span>
              {#if p.caps}
                <span class="flex shrink-0 gap-1 text-[11px] text-muted">
                  {#if p.caps.custom_user}<span class="rounded border border-rule px-1.5" title="You can pick the username">name</span>{/if}
                  {#if p.caps.choose_domain}<span class="rounded border border-rule px-1.5" title="You can pick the domain">domains</span>{/if}
                  {#if !p.caps.private}<span class="rounded bg-danger-soft px-1.5 text-danger" title="Anyone who knows the address can read it">public</span>{/if}
                </span>
              {/if}
            </label>
          {/each}
        </div>
      </fieldset>

      <div class="grid grid-cols-2 gap-3">
        <label class="block">
          <span class="mb-1.5 block text-xs font-semibold">Username <span class="font-normal text-muted">optional</span></span>
          <input
            class="field"
            bind:this={userInput}
            bind:value={user}
            oninput={() => edited("user")}
            aria-invalid={userError ? "true" : undefined}
            aria-describedby={userError ? "new-inbox-user-error" : undefined}
            disabled={!canUser}
            placeholder={canUser ? "random" : `${picked?.name} picks it`}
            autocomplete="off"
            spellcheck="false"
            maxlength="64"
            pattern="[A-Za-z0-9._\-]*"
            title="Letters, digits, dots, dashes and underscores"
          />
        </label>
        <label class="block">
          <span class="mb-1.5 block text-xs font-semibold">Domain</span>
          {#if canDomain}
            <select class="field" bind:value={domain} disabled={!domains?.list}>
              <option value="">{domains?.list ? "Any domain" : domains?.error ? "Any domain (list unavailable)" : "Loading…"}</option>
              {#each domains?.list ?? [] as d (d)}
                <option value={d}>{d}</option>
              {/each}
            </select>
          {:else}
            <input class="field" disabled placeholder={picked ? `${picked.name} has one` : "Pick a provider first"} />
          {/if}
        </label>
      </div>
      {#if userError}
        <p {@attach shown} id="new-inbox-user-error" class="-mt-3 text-2xs text-danger" role="alert">{userError}</p>
      {/if}
      {#if user && !provider}
        <p class="-mt-3 text-2xs text-muted">Only providers that let you pick a username are tried.</p>
      {/if}

      <label class="block">
        <span class="mb-1.5 block text-xs font-semibold">Label <span class="font-normal text-muted">optional</span></span>
        <input
          class="field"
          bind:this={labelInput}
          bind:value={label}
          oninput={() => edited("label")}
          aria-invalid={labelError ? "true" : undefined}
          aria-describedby={labelError ? "new-inbox-label-error" : undefined}
          placeholder="github, shop, newsletter…"
          maxlength="64"
          autocomplete="off"
        />
      </label>
      {#if labelError}
        <p {@attach shown} id="new-inbox-label-error" class="-mt-3 text-2xs text-danger" role="alert">{labelError}</p>
      {/if}
    </div>

    <footer class="flex shrink-0 items-center justify-end gap-2 border-t border-rule px-6 py-3.5">
      <button type="button" class="btn" onclick={() => (app.newInboxOpen = false)}>Cancel</button>
      <button type="submit" class="btn btn-primary min-w-28" disabled={app.creating}>
        {app.creating ? "Creating…" : "Create inbox"}
      </button>
    </footer>
  </form>
</dialog>
