#!/bin/sh
set -eu

if [ "$#" -ne 2 ]; then
  printf 'usage: %s <procnote.dmg> <arm64|x86_64>\n' "$0" >&2
  exit 2
fi

dmg=$1
expected_arch=$2
case $expected_arch in
  arm64 | x86_64) ;;
  *)
    printf 'unsupported expected architecture: %s\n' "$expected_arch" >&2
    exit 2
    ;;
esac
source_launcher=src-tauri/launchers/bin/procnote-launcher
mount_dir=$(mktemp -d)
attached=false

cleanup() {
  if [ "$attached" = true ]; then
    hdiutil detach "$mount_dir" -quiet || true
  fi
  rm -rf "$mount_dir"
}
trap cleanup EXIT HUP INT TERM

hdiutil attach "$dmg" -nobrowse -readonly -mountpoint "$mount_dir" -quiet
attached=true

app=$mount_dir/procnote.app
gui=$app/Contents/MacOS/procnote
launcher=$app/Contents/Resources/bin/procnote

if [ ! -x "$gui" ]; then
  printf 'missing executable GUI in DMG: %s\n' "$gui" >&2
  exit 1
fi
if [ ! -x "$launcher" ]; then
  printf 'missing executable launcher in DMG: %s\n' "$launcher" >&2
  exit 1
fi

cmp "$source_launcher" "$launcher"
gui_arch=$(lipo -archs "$gui")
launcher_arch=$(lipo -archs "$launcher")
if [ "$gui_arch" != "$expected_arch" ]; then
  printf 'unexpected GUI architecture: expected %s, got %s\n' "$expected_arch" "$gui_arch" >&2
  exit 1
fi
if [ "$launcher_arch" != "$expected_arch" ]; then
  printf 'unexpected launcher architecture: expected %s, got %s\n' "$expected_arch" "$launcher_arch" >&2
  exit 1
fi

if otool -L "$launcher" | grep -F 'WebKit' >/dev/null; then
  printf 'terminal launcher unexpectedly links WebKit\n' >&2
  exit 1
fi

if [ "$(uname -m)" = "$expected_arch" ]; then
  launcher_version=$("$launcher" --version)
  gui_version=$("$gui" --version)
  if [ "$launcher_version" != "$gui_version" ]; then
    printf 'launcher and GUI versions differ: %s != %s\n' "$launcher_version" "$gui_version" >&2
    exit 1
  fi
  printf '%s\n' "$launcher_version" | grep -F 'procnote ' >/dev/null

  launcher_help=$("$launcher" --help)
  printf '%s\n' "$launcher_help" | grep -F 'Usage: procnote [WORKSPACE]' >/dev/null
  printf '%s\n' "$launcher_help" | grep -F -- '--version' >/dev/null
fi

if [ -e "$app/Contents/Resources/cli/procnote" ] || [ -e "$app/Contents/Resources/cli/procnote.exe" ]; then
  printf 'legacy CLI executable is still packaged in the DMG\n' >&2
  exit 1
fi

printf 'Validated macOS package: %s\n' "$dmg"
