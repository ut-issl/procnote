---
icon: material/file-document
---

# Templates

A **procedure template** is a Markdown file with YAML frontmatter that defines the structure of a procedure. Templates are the starting point for every execution.

## Directory Layout

Procnote operates on a single **workspace directory** passed as a positional argument:

```bash
procnote /path/to/my-workspace
```

If omitted, Procnote uses the current working directory. Inside the workspace, each subdirectory that contains a `template.md` file is treated as a procedure. Executions are stored under a `.executions/` subdirectory within each procedure.

```text
my-workspace/                       # ← workspace directory
├── thermal-vacuum-test/
│   ├── template.md                 # ← procedure template (discovered automatically)
│   └── .executions/
│       └── 20260402T033055-edb182e8/
│           ├── events.jsonl
│           ├── template.md         # snapshot
│           └── attachments/
└── pcb-inspection/
    ├── template.md
    └── .executions/
        └── ...
```

### How Procnote discovers templates

On startup, Procnote scans the workspace directory for **immediate subdirectories** that contain a file named `template.md`. Each valid template is listed on the home screen. Subdirectories without a `template.md` are silently skipped.

To add a new procedure, create a subdirectory and place a `template.md` file in it.

### How Procnote discovers executions

When listing past executions, Procnote scans each procedure's `.executions/` subdirectory. Each child directory that contains an `events.jsonl` file is loaded as an execution by replaying its event log.

## Frontmatter

The YAML frontmatter defines procedure metadata:

| Field                | Required | Description                              |
| -------------------- | -------- | ---------------------------------------- |
| `id`                 | Yes      | Unique identifier (e.g., `TVT-001`)      |
| `title`              | Yes      | Human-readable name                      |
| `version`            | Yes      | Version string                           |
| `author`             | No       | Procedure author                         |
| `equipment`          | No       | List of equipment (`id` + `name`)        |
| `requirement_traces` | No       | Requirement IDs this procedure traces to |

## Steps

Each `##` heading creates a step. Steps execute sequentially. The heading text becomes the step's display name.

## Content Types

Steps contain three types of content:

### Prose

Regular Markdown (paragraphs, bullet lists, code blocks, subheadings) renders as read-only instructions for the operator.

### Checkboxes

Task lists (`- [ ]` items) create interactive checkboxes. The operator toggles these during execution, and each toggle is recorded as an event.

A list must contain **only** checkbox items to be treated as interactive. Mixed lists render as prose.

### Input Blocks

Fenced code blocks with the `inputs` language tag define data-entry fields. Four input types are supported:

| Type          | Purpose           | Key fields                             |
| ------------- | ----------------- | -------------------------------------- |
| `measurement` | Numeric value     | `unit`, `expected.min`, `expected.max` |
| `text`        | Free-form text    | --                                     |
| `selection`   | Dropdown choice   | `options`, `expected`                  |
| `attachment`  | One or more files | --                                     |

Every input has an `id` (unique within the step) and a `label`.

## Template Snapshots

When an execution starts, Procnote copies the template into the execution directory. This snapshot ensures that the execution record is self-contained -- even if the template is later modified, the execution retains the exact procedure that was followed.
