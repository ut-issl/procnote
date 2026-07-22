import { spawnSync } from "node:child_process";
import { createRequire } from "node:module";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const require = createRequire(import.meta.url);
const tauriCli = require.resolve("@tauri-apps/cli/tauri.js");

function requestedTarget(args) {
  const separatorIndex = args.indexOf("--");
  const commandArguments = separatorIndex === -1 ? args : args.slice(0, separatorIndex);
  const targets = commandArguments.flatMap((argument, index) => {
    if (argument === "--target" || argument === "-t") {
      const value = commandArguments[index + 1];
      if (value === undefined) {
        throw new Error(`${argument} requires a target triple`);
      }
      return [value];
    }
    if (argument.startsWith("--target=")) {
      return [argument.slice("--target=".length)];
    }
    return [];
  });

  if (targets.length > 1) {
    throw new Error("the Tauri target may only be specified once");
  }
  return targets[0];
}

function targetPlatform(target) {
  if (target === undefined) {
    return process.platform;
  }

  const components = target.split("-");
  if (components.includes("windows")) {
    return "win32";
  }
  if (components.includes("darwin")) {
    return "darwin";
  }
  if (components.includes("linux")) {
    return "linux";
  }
  throw new Error(`unsupported desktop target: ${target}`);
}

function bundleConfiguration(platform) {
  const filename = (() => {
    switch (platform) {
      case "darwin":
        return "tauri.bundle.macos.conf.json";
      case "linux":
        return "tauri.bundle.linux.conf.json";
      case "win32":
        return "tauri.bundle.windows.conf.json";
      default:
        throw new Error(`unsupported desktop platform: ${platform}`);
    }
  })();

  return join("src-tauri", filename);
}

const args = process.argv.slice(2);
const forwardedArgs =
  args[0] === "build"
    ? [
        "build",
        "--config",
        bundleConfiguration(targetPlatform(requestedTarget(args))),
        ...args.slice(1),
      ]
    : args;
const result = spawnSync(process.execPath, [tauriCli, ...forwardedArgs], {
  cwd: repositoryRoot,
  stdio: "inherit",
});
if (result.error) {
  throw result.error;
}
if (result.status === null) {
  throw new Error(`Tauri CLI terminated by signal ${result.signal}`);
}
process.exitCode = result.status;
