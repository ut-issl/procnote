# procnote

> [!WARNING]
> Procnote is in early development. The CLI interface, template grammar, event log schema, and storage layout are all subject to change without notice.

A procedure execution tool for tracking step-by-step procedures with checkboxes, data inputs, attachments, and notes. Built as an event-sourced Tauri 2 desktop app (Rust backend + Svelte 5 frontend).

Procedures are written as Markdown templates with YAML frontmatter. Each execution replays an append-only event log, ensuring crash safety and full auditability.

![Procnote execution page showing procedure steps, checkboxes, inputs, notes, and attachment upload controls](docs/assets/procnote-execution-screenshot.png)

_The screenshot is based on the example procedure in [`procedures/example-tvt/template.md`](procedures/example-tvt/template.md)._

## Template Syntax

A procedure template is a Markdown file with YAML frontmatter followed by steps defined as `##` headings.

### Frontmatter

```yaml
---
id: TVT-001
title: "Thermal Vacuum Test - Reaction Wheel Unit"
version: "1.0"
author: "Nomura" # optional
equipment: # optional
  - id: CHAMBER-A
    name: "Thermal Vacuum Chamber A"
requirement_traces: # optional
  - REQ-RWU-TEMP-001
---
```

Required fields: `id`, `title`, `version`.

### Steps

Each `##` heading defines a step. Content within a step can include prose, checkboxes, and input blocks in any order. The order in the template is preserved in the UI.

```markdown
## Step 1: Power On Sequence

Connect PSU to DUT J1 connector. Set voltage to 5.0V.
```

### Checkboxes

Use Markdown task list syntax for interactive checkboxes:

```markdown
- [ ] Chamber pressure < 1e-5 Pa
- [ ] DUT temperature stabilized
- [x] Pre-checked item
```

**Constraint:** A list must contain _only_ checkbox items to be recognized as interactive checkboxes. If a list mixes regular bullet items with checkbox items, the entire list is treated as prose (rendered as Markdown text, not interactive checkboxes).

```markdown
<!-- All checkboxes - rendered as interactive checkboxes -->

- [ ] First check
- [ ] Second check

<!-- Mixed list - rendered as prose, NOT interactive checkboxes -->

- A regular bullet point
- [ ] A checkbox item
```

### Input Blocks

Define data-entry fields using a fenced code block with the `inputs` language tag. The block body is YAML:

````markdown
```inputs
- id: current-draw
  label: "Measure current draw"
  type: measurement
  unit: "mA"
  expected:
    min: 100
    max: 150
- id: selftest-result
  label: "Self-test response"
  type: selection
  options: ["PASS", "FAIL", "TIMEOUT"]
  expected: "PASS"
- id: log-file
  label: "Attach log file"
  type: attachment
```
````

Input types:

| Type          | Description                                                  |
| ------------- | ------------------------------------------------------------ |
| `measurement` | Numeric value with optional `unit` and `expected` range      |
| `text`        | Free-form text                                               |
| `selection`   | Dropdown from `options` list, with optional `expected` value |
| `attachment`  | File upload (stored with SHA-256 hash)                       |

### Prose

Any other Markdown content (paragraphs, bullet lists, sub-headings, code blocks, links, etc.) is rendered as-is. Standard Markdown formatting is supported.

### Full Example

````markdown
---
id: TVT-001
title: "Thermal Vacuum Test"
version: "1.0"
---

## Preconditions

- [ ] Chamber pressure < 1e-5 Pa
- [ ] DUT temperature stabilized at 25 deg C

## Step 1: Power On Sequence

Connect PSU to DUT J1 connector. Set voltage to 5.0V. Enable output.

- [ ] Confirm voltage stable

```inputs
- id: current-draw
  label: "Measure current draw"
  type: measurement
  unit: "mA"
  expected:
    min: 100
    max: 150
```

## Postconditions

- [ ] DUT powered off
- [ ] Chamber returned to ambient
````

## Installation

### macOS (Homebrew)

```sh
brew install --cask ut-issl/tap/procnote
```

Homebrew installs the app to `/Applications/` and links its terminal launcher into your PATH. If you install the DMG manually, create the link yourself:

```sh
sudo ln -sf "/Applications/procnote.app/Contents/Resources/bin/procnote" /usr/local/bin/procnote
```

> [!NOTE]
> The macOS builds are not currently code-signed or notarized. After installing, you need to remove the quarantine attribute:
>
> ```sh
> xattr -cr /Applications/procnote.app
> ```
>
> Without this, macOS will show a "damaged and can't be opened" error.

### Windows

Download the NSIS `.exe` installer from the [Releases page](https://github.com/ut-issl/procnote/releases). It installs the terminal launcher and adds its directory to your user PATH. Open a new terminal after installation.

The `.msi` installer also contains the launcher, but its `bin` directory must currently be added to PATH manually.

### Linux

The `.deb` package installs both the desktop application and `/usr/bin/procnote`. AppImage users must install their own PATH entry or detached launcher for the downloaded AppImage.

### Launch from a terminal

```sh
procnote .
procnote /path/to/workspace
procnote --help
procnote --version
```

Workspace commands start the desktop application and immediately return control to the terminal. Help, version, and argument errors are printed synchronously without starting the GUI.

## Development

Requires [Rust](https://rustup.rs/), [Node.js](https://nodejs.org/), [pnpm](https://pnpm.io/), and [just](https://github.com/casey/just).

```sh
# Start development server
just dev

# Run all checks
just check-all

# Run Rust tests
just test

# Generate TypeScript type bindings from Rust
just generate-types

# Format Rust code
just fmt
```

## Architecture

Three layers with strict dependency direction:

1. **`crates/procnote-core/`** -- Pure Rust domain logic (events, state machine, template parser). No Tauri dependency.
2. **`src-tauri/`** -- Tauri shell. Bridges core to desktop via IPC commands. Owns serialization DTOs and filesystem I/O.
3. **`src/`** -- SvelteKit + Svelte 5 frontend.

**`crates/procnote-launcher/`** is a separate, Tauri-free console adapter. It handles the public terminal interface and starts the packaged GUI; it does not participate in the domain dependency layers above.

Executions are stored as append-only JSONL event logs under `.executions/`.
