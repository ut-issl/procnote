#!/bin/sh
set -eu

if [ "$#" -ne 1 ]; then
  printf 'usage: %s <procnote.deb>\n' "$0" >&2
  exit 2
fi

deb=$1
source_launcher=src-tauri/launchers/bin/procnote-launcher
extract_dir=$(mktemp -d)
trap 'rm -rf "$extract_dir"' EXIT HUP INT TERM

dpkg-deb --extract "$deb" "$extract_dir"

gui=$extract_dir/usr/bin/procnote-gui
launcher=$extract_dir/usr/bin/procnote

if [ ! -x "$gui" ]; then
  printf 'missing executable GUI in Debian package: %s\n' "$gui" >&2
  exit 1
fi
if [ ! -x "$launcher" ]; then
  printf 'missing executable launcher in Debian package: %s\n' "$launcher" >&2
  exit 1
fi

cmp "$source_launcher" "$launcher"
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

if ldd "$gui" | grep -F 'not found' >/dev/null; then
  printf 'GUI has unresolved shared-library dependencies:\n' >&2
  ldd "$gui" >&2
  exit 1
fi
if ldd "$launcher" | grep -F 'not found' >/dev/null; then
  printf 'launcher has unresolved shared-library dependencies:\n' >&2
  ldd "$launcher" >&2
  exit 1
fi
if ldd "$launcher" | grep -F 'libwebkit' >/dev/null || ldd "$launcher" | grep -F 'libgtk' >/dev/null; then
  printf 'terminal launcher unexpectedly links GUI libraries\n' >&2
  exit 1
fi

desktop_file=$(find "$extract_dir/usr/share/applications" -type f -name '*.desktop' -print -quit)
if [ -z "$desktop_file" ] || ! grep -F 'Exec=procnote-gui' "$desktop_file" >/dev/null; then
  printf 'desktop entry does not launch procnote-gui\n' >&2
  exit 1
fi

printf 'Validated Linux package: %s\n' "$deb"
