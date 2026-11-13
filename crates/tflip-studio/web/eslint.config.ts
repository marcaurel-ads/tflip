import {
  defineConfigWithVueTs,
  vueTsConfigs,
} from "@vue/eslint-config-typescript";
import skipFormatting from "@vue/eslint-config-prettier/skip-formatting";
import pluginVue from "eslint-plugin-vue";
import { globalIgnores } from "eslint/config";

export default defineConfigWithVueTs(
  {
    name: "app/files-to-lint",
    files: ["**/*.{ts,mts,tsx,vue}"],
  },

  globalIgnores(["**/dist/**", "src/bindings/**"]),

  pluginVue.configs["flat/recommended"],
  // Type-aware, so it catches what plain syntax rules cannot, such as a
  // promise that is never awaited.
  vueTsConfigs.recommendedTypeChecked,

  // Last, so Prettier alone decides formatting.
  skipFormatting,
);
