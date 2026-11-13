<script setup lang="ts">
import { ref, watch } from "vue";
import { useRouter } from "vue-router";
import { AE_INVALID_HANDLE, ApiFailure, del, errorText, get } from "../api";
import type { ConnectionView } from "../bindings/ConnectionView";

const props = defineProps<{ id: string }>();
const router = useRouter();
const connection = ref<ConnectionView>();
const error = ref<string>();

async function load() {
  connection.value = undefined;
  error.value = undefined;
  try {
    connection.value = await get<ConnectionView>(
      `/api/connections/${props.id}`,
    );
  } catch (e) {
    if (e instanceof ApiFailure && e.error.code === AE_INVALID_HANDLE) {
      // replace: Back must not lead to the dead connection again.
      await router.replace("/");
      return;
    }
    error.value = errorText(e);
  }
}

async function disconnect() {
  try {
    await del(`/api/connections/${props.id}`);
    await router.push("/");
  } catch (e) {
    error.value = errorText(e);
  }
}

// Vue reuses the page when only the ID changes, so onMounted alone would
// miss the switch from one connection to another.
watch(() => props.id, load, { immediate: true });
</script>

<template>
  <template v-if="connection">
    <h1>{{ connection.path }}</h1>
    <p>
      {{ connection.dictionary ? "Dictionary" : "Free tables" }}
      <template v-if="connection.user"> as {{ connection.user }}</template>
    </p>
    <RouterLink :to="`/connections/${id}/sql`">SQL</RouterLink>
    <button @click="disconnect">Disconnect</button>
  </template>
  <p v-if="error" role="alert">{{ error }}</p>
</template>
