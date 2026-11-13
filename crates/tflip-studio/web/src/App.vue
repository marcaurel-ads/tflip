<script setup lang="ts">
import { onMounted, ref } from "vue";
import { get } from "./api";
import type { Version } from "./bindings/Version";

const version = ref<string>();

onMounted(async () => {
  version.value = (await get<Version>("/api/version")).version;
});
</script>

<template>
  <header><RouterLink to="/">tflip-studio</RouterLink> {{ version }}</header>
  <main>
    <RouterView />
  </main>
</template>
