// Build crates/procnote-wasm for the browser into src/lib/wasm/pkg/:
// cargo builds the WebAssembly module, and wasm-bindgen (the CLI, the same
// version as the crate) generates its JavaScript bindings and types.
import { spawnSync } from "node:child_process";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");

function run(command, args) {
  const result = spawnSync(command, args, { cwd: repositoryRoot, stdio: "inherit" });
  if (result.error) throw result.error;
  if (result.status !== 0) {
    throw new Error(`${command} ${args.join(" ")} failed with exit code ${result.status}`);
  }
}

run("cargo", [
  "build",
  "--package",
  "procnote-wasm",
  "--target",
  "wasm32-unknown-unknown",
  "--release",
]);
run("wasm-bindgen", [
  join(repositoryRoot, "target", "wasm32-unknown-unknown", "release", "procnote_wasm.wasm"),
  "--target",
  "web",
  "--out-dir",
  join(repositoryRoot, "src", "lib", "wasm", "pkg"),
]);
