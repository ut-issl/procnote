# Terminal launcher

`procnote` ships one Tauri desktop executable and one small, Tauri-free console
launcher. The launcher owns the public `procnote [WORKSPACE]` interface:

- `--help`, `--version`, and argument errors are handled synchronously in the
  terminal by `crates/procnote-launcher`.
- A valid workspace starts the GUI in a detached process session/group with
  null standard streams, then immediately returns control to the terminal.
- The GUI inherits the caller's working directory and environment, and receives
  the workspace as one OS-native argument without shell interpolation.

The launcher resolves symlinks and locates the GUI from the package layout:

- Windows installs it as `bin/procnote.exe` beside the root `procnote.exe` GUI.
  The NSIS hook uses `nsis/update-user-path.ps1` to add `bin` to the user's PATH
  without NSIS's string-length limit.
- macOS bundles it as `Contents/Resources/bin/procnote`; the Homebrew cask links
  this executable, and DMG users can link it manually.
- Linux packages name the GUI `procnote-gui` and install the launcher as
  `/usr/bin/procnote`.

`scripts/prepare-launcher.mjs` builds the launcher for Tauri's target and stages
it under the ignored `src-tauri/launchers/bin/` directory before Tauri compiles
and bundles the app. `scripts/tauri.mjs` injects the matching
`tauri.bundle.*.conf.json` extension for desktop builds, which places the staged
binary at the paths above. Keeping generated-resource paths out of Tauri's
automatically loaded platform configuration allows direct `cargo check` and
`cargo test` runs to work from a clean checkout.

The launcher must never depend on `procnote-tauri`, Tauri, or GUI/dialog
libraries. This keeps it a normal console executable and ensures the package has
exactly one Tauri application receiving Tauri's package-scoped manifest,
resource, linker, and runtime settings.
