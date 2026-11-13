<script setup lang="ts">
import { onMounted, ref, useTemplateRef } from "vue";
import { useRouter } from "vue-router";
import {
  AE_INVALID_HANDLE,
  ApiFailure,
  VaultLocked,
  del,
  errorText,
  get,
  post,
} from "../api";
import type { ConnectRequest } from "../bindings/ConnectRequest";
import type { Connected } from "../bindings/Connected";
import type { RepositoryEntry } from "../bindings/RepositoryEntry";
import type { UnlockRequest } from "../bindings/UnlockRequest";
import type { VaultState } from "../bindings/VaultState";

const router = useRouter();
const entries = ref<RepositoryEntry[]>([]);
const path = ref("");
const user = ref("");
const password = ref("");
const savePassword = ref(false);
const passwordInput = useTemplateRef("password-input");
const busy = ref(false);
const error = ref<string>();

const masterDialog = useTemplateRef("master-dialog");
const vaultExists = ref(false);
const master = ref("");
const masterAgain = ref("");
const masterError = ref<string>();
// Settled when the dialog closes: true once the vault is unlocked.
let unlocked: ((done: boolean) => void) | undefined;

async function run(work: () => Promise<void>) {
  busy.value = true;
  error.value = undefined;
  try {
    await work();
  } catch (e) {
    error.value = errorText(e);
  } finally {
    busy.value = false;
  }
}

/** Shows the dialog; false if it was cancelled. */
async function askMaster(): Promise<boolean> {
  const state = await get<VaultState>("/api/vault");
  vaultExists.value = state.exists;
  master.value = "";
  masterAgain.value = "";
  masterError.value = undefined;
  masterDialog.value?.showModal();
  return new Promise((resolve) => {
    unlocked = resolve;
  });
}

async function submitMaster() {
  // A typo here would lock the saved passwords away for good.
  if (!vaultExists.value && master.value !== masterAgain.value) {
    masterError.value = "The two passwords differ.";
    return;
  }
  const request: UnlockRequest = { password: master.value };
  try {
    await post<void>("/api/vault/unlock", request);
  } catch (e) {
    masterError.value = errorText(e);
    return;
  }
  unlocked?.(true);
  unlocked = undefined;
  masterDialog.value?.close();
}

// Escape and Cancel close it too, without unlocking.
function masterClosed() {
  unlocked?.(false);
  unlocked = undefined;
}

/** Runs `work`, and once more after the master password if the vault was
 * locked. */
async function withVault(work: () => Promise<void>) {
  try {
    await work();
  } catch (e) {
    if (!(e instanceof VaultLocked)) {
      throw e;
    }
    if (await askMaster()) {
      await work();
    }
  }
}

function fill(entry: RepositoryEntry) {
  path.value = entry.path;
  user.value = entry.user ?? "";
  password.value = "";
  savePassword.value = false;
}

function connect() {
  // An empty field means "no login", which the engine expects as null.
  const request: ConnectRequest = {
    path: path.value,
    user: user.value || null,
    password: password.value || null,
    save_password: savePassword.value && password.value !== "",
  };
  return run(() =>
    withVault(async () => {
      const { id } = await post<Connected>("/api/connections", request);
      await router.push(`/connections/${id}`);
    }),
  );
}

/** One without a user, or with its password saved, connects at once;
 * another needs its password typed in. */
function choose(entry: RepositoryEntry) {
  if (entry.user !== null && !entry.saved) {
    fill(entry);
    passwordInput.value?.focus();
    return;
  }
  return run(() =>
    withVault(async () => {
      const { id } = await post<Connected>(
        `/api/repository/${entry.id}/connect`,
        undefined,
      );
      await router.push(`/connections/${id}`);
    }),
  );
}

function forget(entry: RepositoryEntry) {
  return run(async () => {
    try {
      await del(`/api/repository/${entry.id}`);
    } catch (e) {
      // Forgotten already, by another tab or studio: what was asked holds.
      if (!(e instanceof ApiFailure && e.error.code === AE_INVALID_HANDLE)) {
        throw e;
      }
    }
    entries.value = entries.value.filter((other) => other.id !== entry.id);
  });
}

onMounted(async () => {
  // Set inside run's closure, where TypeScript does not see it change.
  let reopen = "none" as "done" | "locked" | "none";
  await run(async () => {
    try {
      // 204 (undefined) after the first time since the studio started.
      const connected = await post<Connected | undefined>(
        "/api/repository/reopen",
        undefined,
      );
      if (connected !== undefined) {
        reopen = "done";
        // replace: Back from the connection would only reopen it again.
        await router.replace(`/connections/${connected.id}`);
      }
    } catch (e) {
      if (!(e instanceof VaultLocked)) {
        throw e;
      }
      reopen = "locked";
    }
  });
  if (reopen === "done") {
    return;
  }
  // Apart from the reopen, so a failed reopen still lists what can be
  // chosen instead, and keeps its error.
  const failed = error.value;
  await run(async () => {
    entries.value = await get<RepositoryEntry[]>("/api/repository");
    if (entries.value[0] !== undefined) {
      fill(entries.value[0]);
    }
  });
  error.value ??= failed;
  // The server counted its one reopen already; this one is the page's own.
  if (reopen === "locked" && entries.value[0] !== undefined) {
    await choose(entries.value[0]);
  }
});
</script>

<template>
  <h1>Connect</h1>
  <ul v-if="entries.length" class="entries">
    <li v-for="entry in entries" :key="entry.id">
      <button
        type="button"
        class="entry"
        :disabled="busy"
        @click="choose(entry)"
      >
        <strong>{{ entry.name }}</strong>
        <small>
          {{ entry.path }}
          <template v-if="entry.user"> as {{ entry.user }}</template>
          <template v-if="entry.saved"> · password saved</template>
        </small>
      </button>
      <button
        type="button"
        :disabled="busy"
        :aria-label="`Forget ${entry.name}`"
        @click="forget(entry)"
      >
        Forget
      </button>
    </li>
  </ul>
  <form @submit.prevent="connect">
    <label>
      Folder or dictionary (.add)
      <input v-model="path" required />
    </label>
    <label>
      User
      <input v-model="user" autocomplete="username" />
    </label>
    <label>
      Password
      <input
        ref="password-input"
        v-model="password"
        type="password"
        autocomplete="current-password"
      />
    </label>
    <label v-if="password">
      <input v-model="savePassword" type="checkbox" />
      Save password
    </label>
    <button :disabled="busy">Connect</button>
    <p v-if="error" role="alert">{{ error }}</p>
  </form>
  <dialog ref="master-dialog" @close="masterClosed">
    <form @submit.prevent="submitMaster">
      <h2>
        {{
          vaultExists ? "Enter the master password" : "Choose a master password"
        }}
      </h2>
      <p v-if="!vaultExists">
        It protects the passwords the studio saves. It cannot be recovered:
        without it, the saved passwords have to be entered again.
      </p>
      <label>
        Master password
        <input
          v-model="master"
          type="password"
          required
          autofocus
          :autocomplete="vaultExists ? 'current-password' : 'new-password'"
        />
      </label>
      <label v-if="!vaultExists">
        Again
        <input
          v-model="masterAgain"
          type="password"
          required
          autocomplete="new-password"
        />
      </label>
      <button>{{ vaultExists ? "Unlock" : "Set" }}</button>
      <button type="button" @click="masterDialog?.close()">Cancel</button>
      <p v-if="masterError" role="alert">{{ masterError }}</p>
    </form>
  </dialog>
</template>

<style scoped>
.entries {
  list-style: none;
  padding: 0;
}

.entries li {
  display: flex;
  gap: 8px;
  margin-bottom: 8px;
}

.entry {
  display: flex;
  flex: 1;
  flex-direction: column;
  align-items: flex-start;
  text-align: left;
}
</style>
