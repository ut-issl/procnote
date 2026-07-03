---
icon: material/home
---

# Welcome to Procnote

Procnote is an **event-sourced desktop app** for executing and auditing step-by-step procedures. It replaces paper checklists and ad-hoc spreadsheets with a structured, crash-safe execution engine backed by append-only event logs.

![Procnote execution page showing procedure steps, checkboxes, inputs, notes, and attachment upload controls](assets/procnote-execution-screenshot.png)

_The screenshot is based on the example procedure in [`procedures/example-tvt/template.md`](https://github.com/shunichironomura/procnote/blob/main/procedures/example-tvt/template.md)._

## What Does Procnote Do?

1. **Define procedures** as Markdown templates with checkboxes, data inputs, and prose instructions.
2. **Execute procedures** step by step in a desktop UI, recording every action as an immutable event.
3. **Audit and review** completed executions with full event history and revert capabilities.

## Quick Example

A procedure template is a Markdown file with YAML frontmatter:

````markdown
---
id: INSP-001
title: "Visual Inspection - PCB Assembly"
version: "1.0"
---

## Prepare workspace

- [ ] Wear ESD wrist strap
- [ ] Clean inspection surface

## Inspect solder joints

- [ ] No cold solder joints
- [ ] No solder bridges

## Record results

​```inputs

- id: result
  type: selection
  label: "Inspection result"
  options: ["Pass", "Fail", "Conditional pass"]
- id: notes
  type: text
  label: "Inspector notes"
  ​```
````

Launch Procnote with a workspace directory, just like `code <path>` for VS Code:

```bash
procnote /path/to/my-workspace
```

Procnote discovers all `template.md` files in the workspace, guides the operator through each step, captures checkbox states and input values, and writes every action to an append-only event log.

## Getting Started

<div class="grid cards" markdown>

- :material-school:{ .lg .middle } **Guide**

  ***

  Learn how to write templates and run executions.

  [:octicons-arrow-right-24: Start the guide](guide/index.md)

- :material-book-open-variant:{ .lg .middle } **Concepts**

  ***

  Understand templates, event sourcing, and crash safety.

  [:octicons-arrow-right-24: Key concepts](concepts/index.md)

- :material-code-braces:{ .lg .middle } **Development**

  ***

  Set up the development environment and contribute.

  [:octicons-arrow-right-24: Development](development.md)

</div>
