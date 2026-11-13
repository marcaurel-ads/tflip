import { createRouter, createWebHistory } from "vue-router";
import ConnectPage from "./pages/ConnectPage.vue";
import ConnectionPage from "./pages/ConnectionPage.vue";
import SqlPage from "./pages/SqlPage.vue";

export const router = createRouter({
  history: createWebHistory(),
  routes: [
    { path: "/", component: ConnectPage },
    // props: the page gets `id` as a prop, not by reading the route itself.
    { path: "/connections/:id", component: ConnectionPage, props: true },
    { path: "/connections/:id/sql", component: SqlPage, props: true },
  ],
});
