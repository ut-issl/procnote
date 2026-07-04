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

## Preconditions

- [ ] Chamber pressure < 1e-5 Pa
- [ ] DUT temperature stabilized at 25 deg C +/- 2 deg C
- [ ] EGSE connected and nominal

## Step 1: Power On Sequence

Connect PSU to DUT J1 connector. Set voltage to 5.0V. Enable output.

- [ ] a checkbox item
  - [ ] a nested checkbox item

### Some heading

text here

```python
def example_function():
    print("This is an example code block inside the procedure.")
```

```inputs
- id: current-draw
  label: "Measure current draw"
  type: measurement
  unit: "mA"
  expected:
    min: 100
    max: 150
```

text also here

## Step 2: Functional Check

Execute self-test command via EGSE.

```inputs
- id: selftest-result
  label: "Self-test response"
  type: selection
  options: ["PASS", "FAIL", "TIMEOUT"]
  expected: "PASS"
- id: selftest-log
  label: "Attach self-test log file"
  type: attachment
```

## Postconditions

- [ ] DUT powered off
- [ ] Chamber returned to ambient
