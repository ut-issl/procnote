---
id: TABLE-001
title: "Markdown Table Rendering Check"
version: "1.0"
author: "Nomura"
---

## Table rendering smoke test

Use this procedure to verify that Markdown tables render correctly in procedure steps.

| Parameter   | Expected value | Actual value        | Status |
| ----------- | -------------: | :------------------ | :----: |
| Voltage     |          5.0 V | `input:voltage`     |  [ ]   |
| Current     |         120 mA | `input:current`     |  [ ]   |
| Temperature |          25 °C | `input:temperature` |  [ ]   |

```inputs
- id: voltage
  type: measurement
  label: "Voltage"
  unit: V
  expected:
    min: 4.75
    max: 5.25
- id: current
  type: measurement
  label: "Current"
  unit: mA
  expected:
    min: 100
    max: 140
- id: temperature
  type: measurement
  label: "Temperature"
  unit: °C
```

A table without input or checkbox cells is shown as it is:

| Signal | Connector | Pin |
| ------ | :-------- | --: |
| VCC    | J1        |   1 |
| GND    | J1        |   2 |

## Checklist

- [ ] Header row is visually distinct
- [ ] Numeric column alignment is preserved
- [ ] Center-aligned status column is readable
- [ ] Unicode symbols render correctly
- [ ] Values and checkboxes can be filled in table cells

```inputs
- id: table-rendering-notes
  type: text
  label: "Rendering notes"
```
