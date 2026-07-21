# Terminal launchers

`procnote` has one compiled desktop executable. These scripts expose a
VS Code-style `procnote <workspace>` command without building a second copy of
the Tauri application.

- Windows bundles `windows/procnote.cmd` as `bin/procnote.cmd`; the NSIS hook
  uses `nsis/update-user-path.ps1` to add that directory to the user's PATH
  without NSIS's string-length limit.
- macOS bundles `macos/procnote` inside the app. The Homebrew cask links this
  script, and DMG users can link it manually.
- Linux packages rename the bundled GUI to `procnote-gui` and install
  `linux/procnote` as `/usr/bin/procnote`.

Each launcher preserves the caller's working directory and environment,
forwards arguments unchanged, detaches the GUI process, and returns control to
the terminal. Do not replace these launchers with a separately built Tauri
binary: Cargo build-script resource and linker settings are package-scoped, so
a second package can silently lose the GUI manifest or runtime configuration.
