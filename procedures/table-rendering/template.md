---
id: TABLE-001
title: "Markdown Table Rendering Check"
version: "1.0"
author: "Nomura"
---

## Table rendering smoke test

Use this procedure to verify that Markdown tables render correctly in procedure steps.

| Parameter   | Expected value | Actual value | Status |
| ----------- | -------------: | :----------- | :----: |
| Voltage     |          5.0 V | TBD          |   ☐    |
| Current     |         120 mA | TBD          |   ☐    |
| Temperature |          25 °C | TBD          |   ☐    |

## Checklist

- [ ] Header row is visually distinct
- [ ] Numeric column alignment is preserved
- [ ] Center-aligned status column is readable
- [ ] Unicode symbols render correctly

```inputs
- id: table-rendering-notes
  type: text
  label: "Rendering notes"
```
