---
icon: material/file-document-edit
---

# Writing a Template

A procedure template is a Markdown file with YAML frontmatter. Place it as `template.md` inside a procedure directory:

```text
procedures/
└── my-procedure/
    └── template.md
```

## Frontmatter

Every template starts with YAML frontmatter between `---` fences:

```yaml
---
id: TVT-001
title: "Thermal Vacuum Test - Reaction Wheel Unit"
version: "1.0"
author: "Nomura"
equipment:
  - id: CHAMBER-A
    name: "Thermal Vacuum Chamber A"
requirement_traces:
  - REQ-RWU-TEMP-001
---
```

| Field                | Required | Description                                      |
| -------------------- | -------- | ------------------------------------------------ |
| `id`                 | Yes      | Unique procedure identifier                      |
| `title`              | Yes      | Human-readable procedure name                    |
| `version`            | Yes      | Procedure version string                         |
| `author`             | No       | Procedure author                                 |
| `equipment`          | No       | List of equipment with `id` and `name`           |
| `requirement_traces` | No       | List of requirement IDs this procedure traces to |

## Steps

Each `##` heading defines a step. Steps are executed in order.

```markdown
## Preconditions

Instructions go here.

## Step 1: Power On

More instructions.
```

## Content Types

Within a step, you can use three types of content.

### Prose

Any Markdown content (paragraphs, lists, code blocks, subheadings) renders as read-only instructions.

```markdown
## Step 1: Power On

Connect PSU to DUT J1 connector. Set voltage to 5.0V.

### Substep details

Additional context with a code example:

​`python
print("example")
​`
```

### Checkboxes

Task lists create interactive checkboxes that the operator toggles during execution:

```markdown
## Preconditions

- [ ] Chamber pressure < 1e-5 Pa
- [ ] DUT temperature stabilized
- [ ] EGSE connected and nominal
```

!!! warning "Pure checkbox lists only"

    A list must contain **only** checkbox items (`- [ ]` or `- [x]`) to be recognized as interactive. Mixed lists (regular bullets alongside checkboxes) render as prose.

### Input Blocks

Fenced code blocks with the `inputs` language tag define data-entry fields:

````markdown
```inputs
- id: current-draw
  label: "Measure current draw"
  type: measurement
  unit: "mA"
  expected:
    min: 100
    max: 150
```
````

#### Input Types

**`measurement`** -- Numeric value with optional unit and expected range.

```yaml
- id: current-draw
  label: "Measure current draw"
  type: measurement
  unit: "mA"
  expected:
    min: 100
    max: 150
```

**`text`** -- Free-form text input.

```yaml
- id: notes
  label: "Inspector notes"
  type: text
```

**`selection`** -- Dropdown from a predefined list of options.

```yaml
- id: selftest-result
  label: "Self-test response"
  type: selection
  options: ["PASS", "FAIL", "TIMEOUT"]
  expected: "PASS"
```

**`attachment`** -- File upload. The file is stored with a SHA-256 hash prefix for integrity.

```yaml
- id: log-file
  label: "Attach log file"
  type: attachment
```

## Complete Example

````markdown
---
id: TVT-001
title: "Thermal Vacuum Test - Reaction Wheel Unit"
version: "1.0"
author: "Nomura"
equipment:
  - id: CHAMBER-A
    name: "Thermal Vacuum Chamber A"
---

## Preconditions

- [ ] Chamber pressure < 1e-5 Pa
- [ ] DUT temperature stabilized at 25 deg C +/- 2 deg C
- [ ] EGSE connected and nominal

## Step 1: Power On Sequence

Connect PSU to DUT J1 connector. Set voltage to 5.0V. Enable output.

- [ ] PSU output enabled

​```inputs

- id: current-draw
  label: "Measure current draw"
  type: measurement
  unit: "mA"
  expected:
  min: 100
  max: 150
  ​```

## Step 2: Functional Check

Execute self-test command via EGSE.

​```inputs

- id: selftest-result
  label: "Self-test response"
  type: selection
  options: ["PASS", "FAIL", "TIMEOUT"]
  expected: "PASS"
- id: selftest-log
  label: "Attach self-test log file"
  type: attachment
  ​```

## Postconditions

- [ ] DUT powered off
- [ ] Chamber returned to ambient
````
