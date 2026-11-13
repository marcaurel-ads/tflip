<script setup lang="ts">
import { onBeforeUnmount, ref, watch } from "vue";
import { useRouter } from "vue-router";
import {
  AE_INVALID_HANDLE,
  ApiFailure,
  SqlSocket,
  errorText,
  get,
  post,
} from "../api";
import type { Column } from "../bindings/Column";
import type { ConnectionView } from "../bindings/ConnectionView";
import type { SqlRequest } from "../bindings/SqlRequest";

const PAGE = 100;

const props = defineProps<{ id: string }>();
const router = useRouter();
const path = ref<string>();
const sql = ref("");
const busy = ref(false);
const error = ref<string>();
const message = ref<string>();
const columns = ref<Column[]>([]);
const rows = ref<(string | null)[][]>([]);
const start = ref(0);
const end = ref(true);
// Not refs: nothing on the page shows them.
let socket: SqlSocket | undefined;
let cursor: number | undefined;

function clearResult() {
  cursor = undefined;
  columns.value = [];
  rows.value = [];
  start.value = 0;
  end.value = true;
}

async function run(work: () => Promise<void>) {
  busy.value = true;
  error.value = undefined;
  message.value = undefined;
  try {
    await work();
  } catch (e) {
    if (e instanceof ApiFailure && e.error.code === AE_INVALID_HANDLE) {
      // replace: Back must not lead to the dead connection again.
      await router.replace("/");
      return;
    }
    error.value = errorText(e);
  } finally {
    busy.value = false;
  }
}

/** Asks for the connection first: a socket to an unknown one only fails,
 * without saying why, so the page could not return to Connect. */
async function open() {
  socket?.close();
  socket = undefined;
  clearResult();
  path.value = (await get<ConnectionView>(`/api/connections/${props.id}`)).path;
  socket = new SqlSocket(props.id);
}

async function page(first: number) {
  if (socket === undefined || cursor === undefined) {
    return;
  }
  const answer = await socket.rows(cursor, first, PAGE);
  start.value = first;
  rows.value = answer.rows;
  end.value = answer.end;
}

function execute() {
  return run(async () => {
    if (socket === undefined) {
      return;
    }
    if (cursor !== undefined) {
      const old = cursor;
      clearResult();
      await socket.closeCursor(old);
    }
    const answer = await socket.execute(sql.value);
    if (answer.type === "affected") {
      message.value =
        answer.rows === null
          ? "Done."
          : `${answer.rows} ${answer.rows === 1 ? "row" : "rows"} affected`;
      return;
    }
    cursor = answer.cursor;
    columns.value = answer.columns;
    await page(0);
  });
}

function verify() {
  const request: SqlRequest = { sql: sql.value };
  return run(async () => {
    await post<void>(`/api/connections/${props.id}/verify`, request);
    message.value = "The SQL is valid.";
  });
}

function turn(first: number) {
  return run(() => page(first));
}

// Vue reuses the page when only the ID changes, so onMounted alone would
// miss the switch from one connection to another.
watch(
  () => props.id,
  () => run(open),
  { immediate: true },
);
onBeforeUnmount(() => socket?.close());
</script>

<template>
  <div class="sql">
    <h1>SQL</h1>
    <p v-if="path">
      <RouterLink :to="`/connections/${id}`">{{ path }}</RouterLink>
    </p>
    <form @submit.prevent="execute">
      <textarea
        v-model="sql"
        rows="8"
        spellcheck="false"
        @keydown.ctrl.enter.prevent="execute"
      />
      <button :disabled="busy">Execute</button>
      <button type="button" :disabled="busy" @click="verify">Verify</button>
    </form>
    <p v-if="message" role="status">{{ message }}</p>
    <p v-if="error" role="alert">{{ error }}</p>
    <template v-if="columns.length">
      <div class="result">
        <table>
          <thead>
            <tr>
              <th
                v-for="(column, c) in columns"
                :key="c"
                :title="column.field_type"
              >
                {{ column.name }}
              </th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="(row, r) in rows" :key="start + r">
              <td
                v-for="(value, c) in row"
                :key="c"
                :class="{ null: value === null }"
              >
                {{ value ?? "NULL" }}
              </td>
            </tr>
          </tbody>
        </table>
      </div>
      <nav>
        <button
          :disabled="busy || start === 0"
          @click="turn(Math.max(0, start - PAGE))"
        >
          Previous
        </button>
        <span v-if="rows.length">
          Rows {{ start + 1 }}–{{ start + rows.length }}
        </span>
        <span v-else>No rows</span>
        <button :disabled="busy || end" @click="turn(start + PAGE)">
          Next
        </button>
      </nav>
    </template>
  </div>
</template>

<style scoped>
/* #app centres its text, which suits no editor or grid. */
.sql {
  text-align: left;
}

textarea {
  box-sizing: border-box;
  width: 100%;
  font-family: var(--mono);
}

.result {
  overflow-x: auto;
}

table {
  border-collapse: collapse;
  font-family: var(--mono);
  font-size: 14px;
}

th,
td {
  border: 1px solid var(--border);
  padding: 2px 6px;
  /* Field text keeps its padding, as ARC's grid shows it. */
  white-space: pre;
}

td.null {
  font-style: italic;
  opacity: 0.6;
}
</style>
