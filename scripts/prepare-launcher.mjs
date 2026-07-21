import { chmodSync, copyFileSync, existsSync, mkdirSync, rmSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const stagedDirectory = join(repositoryRoot, "src-tauri", "launchers", "bin");

function run(command, args, options = {}) {
  const result = spawnSync(command, args, {
    cwd: repositoryRoot,
    encoding: "utf8",
    ...options,
  });

  if (result.error) {
    throw result.error;
  }
  if (result.status === null) {
    throw new Error(`${command} terminated by signal ${result.signal}`);
  }
  if (result.status !== 0) {
    throw new Error(`${command} ${args.join(" ")} failed with exit code ${result.status}`);
  }

  return result.stdout?.trim() ?? "";
}

function normalizeArchitecture(architecture) {
  switch (architecture) {
    case "arm64":
      return "aarch64";
    case "x64":
      return "x86_64";
    default:
      return architecture;
  }
}

function targetFromTauriEnvironment() {
  const platform = process.env.TAURI_ENV_PLATFORM;
  const architecture = process.env.TAURI_ENV_ARCH;

  if (platform === undefined && architecture === undefined) {
    return undefined;
  }
  if (platform === undefined || architecture === undefined) {
    throw new Error(
      "TAURI_ENV_PLATFORM and TAURI_ENV_ARCH must either both be set or both be unset",
    );
  }

  const rustArchitecture = normalizeArchitecture(architecture);
  switch (platform) {
    case "darwin":
    case "macos":
      return { triple: `${rustArchitecture}-apple-darwin`, executableSuffix: "" };
    case "linux":
      return { triple: `${rustArchitecture}-unknown-linux-gnu`, executableSuffix: "" };
    case "windows":
      return { triple: `${rustArchitecture}-pc-windows-msvc`, executableSuffix: ".exe" };
    default:
      throw new Error(`unsupported Tauri target platform: ${platform}`);
  }
}

function hostTarget() {
  const triple = run("rustc", ["--print", "host-tuple"]);
  if (triple.length === 0) {
    throw new Error("rustc returned an empty host target triple");
  }

  return {
    triple,
    executableSuffix: process.platform === "win32" ? ".exe" : "",
  };
}

function buildProfile() {
  switch (process.env.TAURI_ENV_DEBUG) {
    case "true":
      return "debug";
    case undefined:
    case "":
    case "false":
      return "release";
    default:
      throw new Error(`invalid TAURI_ENV_DEBUG value: ${process.env.TAURI_ENV_DEBUG}`);
  }
}

const target = targetFromTauriEnvironment() ?? hostTarget();
const profile = buildProfile();
const cargoArguments = ["build", "--package", "procnote-launcher", "--target", target.triple];
if (profile === "release") {
  cargoArguments.push("--release");
}

run("cargo", cargoArguments, { stdio: "inherit" });

const metadata = JSON.parse(run("cargo", ["metadata", "--format-version", "1", "--no-deps"]));
const executableName = `procnote-launcher${target.executableSuffix}`;
const builtLauncher = join(metadata.target_directory, target.triple, profile, executableName);
if (!existsSync(builtLauncher)) {
  throw new Error(`Cargo did not produce the launcher at ${builtLauncher}`);
}

rmSync(stagedDirectory, { recursive: true, force: true });
mkdirSync(stagedDirectory, { recursive: true });
const stagedLauncher = join(stagedDirectory, executableName);
copyFileSync(builtLauncher, stagedLauncher);
if (target.executableSuffix.length === 0) {
  chmodSync(stagedLauncher, 0o755);
}

process.stdout.write(`Staged ${target.triple} launcher at ${stagedLauncher}\n`);
