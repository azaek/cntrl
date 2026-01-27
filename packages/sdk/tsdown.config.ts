import { defineConfig } from "tsdown";

export default defineConfig({
  entry: ["src/index.ts", "src/client.ts", "src/types.ts"],
  format: "esm",
  dts: true,
  clean: true,
});
