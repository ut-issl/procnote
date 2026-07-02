use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

use super::types::{
    ExpectedValue, InputDefinition, InputType, ProcedureMetadata, ProcedureTemplate, Step,
    StepContent,
};

/// Errors that can occur during template parsing.
#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("missing YAML frontmatter (expected `---` delimiters)")]
    MissingFrontmatter,
    #[error("invalid YAML frontmatter: {0}")]
    InvalidYaml(#[from] serde_yaml_ng::Error),
    #[error("invalid inputs block: {0}")]
    InvalidInputsBlock(String),
    #[error("content before the first `##` step heading is not allowed")]
    ContentBeforeFirstStep,
    #[error("template body must contain at least one `##` step heading")]
    MissingStepHeading,
}

/// Parse a procedure Markdown file (with YAML frontmatter) into a `ProcedureTemplate`.
pub fn parse_template(source: &str) -> Result<ProcedureTemplate, ParseError> {
    let (frontmatter, body) = split_frontmatter(source)?;
    let metadata: ProcedureMetadata = serde_yaml_ng::from_str(frontmatter)?;
    let steps = parse_body(body)?;
    Ok(ProcedureTemplate { metadata, steps })
}

/// Split `---`-delimited YAML frontmatter from the Markdown body.
fn split_frontmatter(source: &str) -> Result<(&str, &str), ParseError> {
    let trimmed = source.trim_start();
    let leading_len = source.len() - trimmed.len();

    let Some(opener) = trimmed.split_inclusive('\n').next() else {
        return Err(ParseError::MissingFrontmatter);
    };
    if line_without_newline(opener) != "---" {
        return Err(ParseError::MissingFrontmatter);
    }

    let frontmatter_start = leading_len + opener.len();
    let mut cursor = frontmatter_start;
    for line in source[frontmatter_start..].split_inclusive('\n') {
        if line_without_newline(line) == "---" {
            let frontmatter = &source[frontmatter_start..cursor];
            let body_start = cursor + line.len();
            return Ok((frontmatter, &source[body_start..]));
        }
        cursor += line.len();
    }

    Err(ParseError::MissingFrontmatter)
}

fn line_without_newline(line: &str) -> &str {
    line.trim_end_matches(['\r', '\n'])
}

/// Parse the Markdown body into a list of steps, split on level-2 headings.
fn parse_body(body: &str) -> Result<Vec<Step>, ParseError> {
    let events = markdown_events(body);
    let headings = step_headings(&events);

    let Some(first_heading) = headings.first() else {
        return Err(ParseError::MissingStepHeading);
    };
    if !body[..first_heading.heading_start].trim().is_empty() {
        return Err(ParseError::ContentBeforeFirstStep);
    }

    headings
        .iter()
        .enumerate()
        .map(|(index, heading)| {
            let next_heading_start = headings
                .get(index + 1)
                .map_or(body.len(), |next| next.heading_start);
            let content = parse_step_content(&body[heading.content_start..next_heading_start])?;
            Ok(Step {
                id: None,
                heading: heading.text.clone(),
                content,
            })
        })
        .collect()
}

type MarkdownEvent<'a> = (Event<'a>, std::ops::Range<usize>);

#[derive(Debug)]
struct StepHeading {
    text: String,
    heading_start: usize,
    content_start: usize,
}

#[derive(Debug)]
struct ContentSegment {
    range: std::ops::Range<usize>,
    content: Vec<StepContent>,
}

fn markdown_events(source: &str) -> Vec<MarkdownEvent<'_>> {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TASKLISTS);
    Parser::new_ext(source, options)
        .into_offset_iter()
        .collect()
}

fn step_headings(events: &[MarkdownEvent<'_>]) -> Vec<StepHeading> {
    let mut headings = Vec::new();
    let mut index = 0;
    while index < events.len() {
        match &events[index].0 {
            Event::Start(Tag::Heading { level, .. })
                if *level == pulldown_cmark::HeadingLevel::H2 =>
            {
                let heading_start = events[index].1.start;
                let (text, next_index, content_start) = collect_heading_text(events, index);
                headings.push(StepHeading {
                    text,
                    heading_start,
                    content_start,
                });
                index = next_index;
            }
            _ => index += 1,
        }
    }
    headings
}

/// Collect the text content of a level-2 heading.
fn collect_heading_text(
    events: &[MarkdownEvent<'_>],
    start_index: usize,
) -> (String, usize, usize) {
    let mut text = String::new();
    let mut index = start_index + 1;
    while index < events.len() {
        match &events[index].0 {
            Event::End(TagEnd::Heading(pulldown_cmark::HeadingLevel::H2)) => {
                return (text, index + 1, events[index].1.end);
            }
            Event::Text(t) | Event::Code(t) => {
                text.push_str(t);
                index += 1;
            }
            Event::SoftBreak | Event::HardBreak => {
                text.push(' ');
                index += 1;
            }
            _ => index += 1,
        }
    }
    (text, index, events[start_index].1.end)
}

fn parse_step_content(section: &str) -> Result<Vec<StepContent>, ParseError> {
    let events = markdown_events(section);
    let segments = content_segments(&events)?;
    let mut content = Vec::new();
    let mut cursor = 0;

    for segment in segments {
        push_prose(section, cursor, segment.range.start, &mut content);
        content.extend(segment.content);
        cursor = segment.range.end;
    }
    push_prose(section, cursor, section.len(), &mut content);

    Ok(content)
}

fn push_prose(section: &str, start: usize, end: usize, content: &mut Vec<StepContent>) {
    let raw = section[start..end].trim();
    if !raw.is_empty() {
        content.push(StepContent::Prose {
            text: raw.to_string(),
        });
    }
}

fn content_segments(events: &[MarkdownEvent<'_>]) -> Result<Vec<ContentSegment>, ParseError> {
    let mut segments = Vec::new();
    let mut index = 0;

    while index < events.len() {
        match &events[index].0 {
            Event::Start(Tag::List(_)) => {
                let Some(end_index) = matching_list_end(events, index) else {
                    index += 1;
                    continue;
                };
                if is_pure_task_list(events, index, end_index) {
                    segments.push(ContentSegment {
                        range: events[index].1.start..events[end_index].1.end,
                        content: collect_task_checkboxes(events, index, end_index),
                    });
                }
                index = end_index + 1;
            }
            Event::Start(Tag::CodeBlock(pulldown_cmark::CodeBlockKind::Fenced(lang)))
                if lang.as_ref() == "inputs" =>
            {
                let Some(end_index) = matching_code_block_end(events, index) else {
                    index += 1;
                    continue;
                };
                let code = collect_code_block(events, index, end_index);
                segments.push(ContentSegment {
                    range: events[index].1.start..events[end_index].1.end,
                    content: vec![StepContent::InputBlock {
                        inputs: parse_inputs_block(&code)?,
                    }],
                });
                index = end_index + 1;
            }
            _ => index += 1,
        }
    }

    Ok(segments)
}

fn matching_list_end(events: &[MarkdownEvent<'_>], list_start_index: usize) -> Option<usize> {
    let mut depth = 0usize;
    events
        .iter()
        .enumerate()
        .skip(list_start_index)
        .find_map(|(index, (event, _))| match event {
            Event::Start(Tag::List(_)) => {
                depth += 1;
                None
            }
            Event::End(TagEnd::List(_)) => {
                depth = depth.saturating_sub(1);
                (depth == 0).then_some(index)
            }
            _ => None,
        })
}

fn matching_code_block_end(events: &[MarkdownEvent<'_>], start_index: usize) -> Option<usize> {
    events
        .iter()
        .enumerate()
        .skip(start_index + 1)
        .find_map(|(index, (event, _))| {
            matches!(event, Event::End(TagEnd::CodeBlock)).then_some(index)
        })
}

/// Check whether the list is a pure task list.
///
/// Every item at every nesting level must have its own `TaskListMarker`; if not,
/// the whole list is prose so no nested checkboxes are ripped out of context.
fn is_pure_task_list(
    events: &[MarkdownEvent<'_>],
    list_start_index: usize,
    list_end_index: usize,
) -> bool {
    let item_indices: Vec<usize> = events
        .iter()
        .enumerate()
        .take(list_end_index)
        .skip(list_start_index + 1)
        .filter_map(|(index, (event, _))| matches!(event, Event::Start(Tag::Item)).then_some(index))
        .collect();

    !item_indices.is_empty()
        && item_indices
            .into_iter()
            .all(|item_index| direct_item_has_marker(events, item_index))
}

fn direct_item_has_marker(events: &[MarkdownEvent<'_>], item_start_index: usize) -> bool {
    let mut item_depth = 1usize;
    let mut index = item_start_index + 1;
    while index < events.len() {
        match &events[index].0 {
            Event::TaskListMarker(_) if item_depth == 1 => return true,
            Event::Start(Tag::Item) => item_depth += 1,
            Event::End(TagEnd::Item) => {
                item_depth = item_depth.saturating_sub(1);
                if item_depth == 0 {
                    return false;
                }
            }
            _ => {}
        }
        index += 1;
    }
    false
}

fn collect_task_checkboxes(
    events: &[MarkdownEvent<'_>],
    list_start_index: usize,
    list_end_index: usize,
) -> Vec<StepContent> {
    events
        .iter()
        .enumerate()
        .take(list_end_index)
        .skip(list_start_index + 1)
        .filter_map(|(index, (event, _))| match event {
            Event::TaskListMarker(checked) => Some(StepContent::Checkbox {
                id: None,
                text: collect_task_text(events, index),
                checked: *checked,
            }),
            _ => None,
        })
        .collect()
}

/// Collect the text of one task item, excluding nested lists from the parent text.
fn collect_task_text(events: &[MarkdownEvent<'_>], marker_index: usize) -> String {
    let mut text = String::new();
    let mut item_depth = 1usize;
    let mut nested_list_depth = 0usize;
    let mut index = marker_index + 1;

    while index < events.len() {
        match &events[index].0 {
            Event::End(TagEnd::Item) if item_depth == 1 => break,
            Event::End(TagEnd::Item) => {
                item_depth = item_depth.saturating_sub(1);
            }
            Event::Start(Tag::Item) => {
                item_depth += 1;
            }
            Event::Start(Tag::List(_)) => {
                nested_list_depth += 1;
            }
            Event::End(TagEnd::List(_)) => {
                nested_list_depth = nested_list_depth.saturating_sub(1);
            }
            Event::Text(t) | Event::Code(t) if item_depth == 1 && nested_list_depth == 0 => {
                text.push_str(t);
            }
            Event::SoftBreak | Event::HardBreak if item_depth == 1 && nested_list_depth == 0 => {
                text.push(' ');
            }
            _ => {}
        }
        index += 1;
    }

    text.trim().to_string()
}

/// Collect content of a fenced code block.
fn collect_code_block(
    events: &[MarkdownEvent<'_>],
    start_index: usize,
    end_index: usize,
) -> String {
    events
        .iter()
        .take(end_index)
        .skip(start_index + 1)
        .filter_map(|(event, _)| match event {
            Event::Text(t) => Some(t.as_ref()),
            _ => None,
        })
        .collect()
}

/// Parse a YAML inputs block into a list of `InputDefinition`s.
fn parse_inputs_block(code: &str) -> Result<Vec<InputDefinition>, ParseError> {
    // The inputs block is a YAML list of input definitions.
    // We need to handle the `expected` field specially since it can be a range or exact value.
    let raw: Vec<RawInputDefinition> =
        serde_yaml_ng::from_str(code).map_err(|e| ParseError::InvalidInputsBlock(e.to_string()))?;

    raw.into_iter()
        .map(std::convert::TryInto::try_into)
        .collect()
}

/// Intermediate representation for deserializing input definitions with flexible `expected`.
#[derive(Debug, Deserialize)]
struct RawInputDefinition {
    id: String,
    label: String,
    #[serde(rename = "type")]
    input_type: InputType,
    #[serde(default)]
    unit: Option<String>,
    #[serde(default)]
    options: Vec<String>,
    #[serde(default)]
    expected: Option<serde_yaml_ng::Value>,
}

use serde::Deserialize;

impl TryFrom<RawInputDefinition> for InputDefinition {
    type Error = ParseError;

    fn try_from(raw: RawInputDefinition) -> Result<Self, Self::Error> {
        let expected = match raw.expected {
            None => None,
            Some(serde_yaml_ng::Value::Mapping(map)) => {
                let min = map
                    .get(serde_yaml_ng::Value::String("min".to_string()))
                    .and_then(serde_yaml_ng::Value::as_f64);
                let max = map
                    .get(serde_yaml_ng::Value::String("max".to_string()))
                    .and_then(serde_yaml_ng::Value::as_f64);
                match (min, max) {
                    (Some(min), Some(max)) => Some(ExpectedValue::Range { min, max }),
                    _ => {
                        return Err(ParseError::InvalidInputsBlock(
                            "expected range must have both `min` and `max`".to_string(),
                        ));
                    }
                }
            }
            Some(serde_yaml_ng::Value::String(s)) => Some(ExpectedValue::Exact(s)),
            Some(_) => {
                return Err(ParseError::InvalidInputsBlock(
                    "`expected` must be a string or a {min, max} mapping".to_string(),
                ));
            }
        };

        Ok(Self {
            id: raw.id,
            label: raw.label,
            input_type: raw.input_type,
            unit: raw.unit,
            options: raw.options,
            expected,
        })
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "unwrap is acceptable in tests")]
mod tests {
    use super::*;

    const SAMPLE_TEMPLATE: &str = r#"---
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

```inputs
- id: current-draw
  label: "Measure current draw"
  type: measurement
  unit: "mA"
  expected:
    min: 100
    max: 150
```

## Step 2: Functional Check

Execute self-test command via EGSE.

```inputs
- id: selftest-result
  label: "Self-test response"
  type: selection
  options: ["PASS", "FAIL", "TIMEOUT"]
  expected: "PASS"
```

## Postconditions

- [ ] DUT powered off
- [ ] Chamber returned to ambient
"#;

    #[test]
    fn test_parse_frontmatter() {
        let (fm, body) = split_frontmatter(SAMPLE_TEMPLATE).unwrap();
        assert!(fm.contains("TVT-001"));
        assert!(body.trim_start().starts_with("## Preconditions"));
    }

    #[test]
    fn test_parse_metadata() {
        let template = parse_template(SAMPLE_TEMPLATE).unwrap();
        assert_eq!(template.metadata.id, "TVT-001");
        assert_eq!(
            template.metadata.title,
            "Thermal Vacuum Test - Reaction Wheel Unit"
        );
        assert_eq!(template.metadata.version, "1.0");
        assert_eq!(template.metadata.author, Some("Nomura".to_string()));
        assert_eq!(template.metadata.equipment.len(), 1);
        assert_eq!(template.metadata.equipment[0].id, "CHAMBER-A");
        assert_eq!(template.metadata.requirement_traces.len(), 1);
    }

    #[test]
    fn test_parse_steps() {
        let template = parse_template(SAMPLE_TEMPLATE).unwrap();
        assert_eq!(template.steps.len(), 4);

        assert_eq!(template.steps[0].heading, "Preconditions");
        assert_eq!(template.steps[1].heading, "Step 1: Power On Sequence");
        assert_eq!(template.steps[2].heading, "Step 2: Functional Check");
        assert_eq!(template.steps[3].heading, "Postconditions");
    }

    #[test]
    fn test_parse_checkboxes() {
        let template = parse_template(SAMPLE_TEMPLATE).unwrap();
        let preconditions = &template.steps[0];

        let checkboxes: Vec<_> = preconditions
            .content
            .iter()
            .filter_map(|c| match c {
                StepContent::Checkbox { text, checked, .. } => Some((text.clone(), *checked)),
                _ => None,
            })
            .collect();

        assert_eq!(checkboxes.len(), 3);
        assert_eq!(checkboxes[0].0, "Chamber pressure < 1e-5 Pa");
        assert!(!checkboxes[0].1);
        assert_eq!(
            checkboxes[1].0,
            "DUT temperature stabilized at 25 deg C +/- 2 deg C"
        );
        assert!(!checkboxes[1].1);
    }

    #[test]
    fn test_parse_prose() {
        let template = parse_template(SAMPLE_TEMPLATE).unwrap();
        let step1 = &template.steps[1];

        let prose: Vec<_> = step1
            .content
            .iter()
            .filter_map(|c| match c {
                StepContent::Prose { text } => Some(text.clone()),
                _ => None,
            })
            .collect();

        assert_eq!(prose.len(), 1);
        assert!(prose[0].contains("Connect PSU to DUT J1 connector"));
    }

    #[test]
    fn test_parse_measurement_input() {
        let template = parse_template(SAMPLE_TEMPLATE).unwrap();
        let step1 = &template.steps[1];

        let inputs: Vec<_> = step1
            .content
            .iter()
            .filter_map(|c| match c {
                StepContent::InputBlock { inputs } => Some(inputs.clone()),
                _ => None,
            })
            .collect();

        assert_eq!(inputs.len(), 1);
        assert_eq!(inputs[0].len(), 1);

        let input = &inputs[0][0];
        assert_eq!(input.id, "current-draw");
        assert_eq!(input.label, "Measure current draw");
        assert_eq!(input.input_type, InputType::Measurement);
        assert_eq!(input.unit, Some("mA".to_string()));
        assert_eq!(
            input.expected,
            Some(ExpectedValue::Range {
                min: 100.0,
                max: 150.0
            })
        );
    }

    #[test]
    fn test_parse_selection_input() {
        let template = parse_template(SAMPLE_TEMPLATE).unwrap();
        let step2 = &template.steps[2];

        let inputs: Vec<_> = step2
            .content
            .iter()
            .filter_map(|c| match c {
                StepContent::InputBlock { inputs } => Some(inputs.clone()),
                _ => None,
            })
            .collect();

        assert_eq!(inputs.len(), 1);
        let input = &inputs[0][0];
        assert_eq!(input.id, "selftest-result");
        assert_eq!(input.input_type, InputType::Selection);
        assert_eq!(input.options, vec!["PASS", "FAIL", "TIMEOUT"]);
        assert_eq!(
            input.expected,
            Some(ExpectedValue::Exact("PASS".to_string()))
        );
    }

    #[test]
    fn test_missing_frontmatter() {
        let result = parse_template("# No frontmatter\nSome text.");
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ParseError::MissingFrontmatter
        ));
    }

    #[test]
    fn test_frontmatter_delimiters_must_be_exact_lines() {
        let result = split_frontmatter("----\nid: BAD\n---\n");
        assert!(matches!(result, Err(ParseError::MissingFrontmatter)));

        let source = "---\nid: GOOD\ntext: |\n  --- not a delimiter\n---\n## Step\n";
        let (frontmatter, body) = split_frontmatter(source).unwrap();
        assert!(frontmatter.contains("--- not a delimiter"));
        assert!(body.starts_with("## Step"));
    }

    #[test]
    fn test_content_before_first_step_heading_errors() {
        let source = r#"---
id: INTRO-001
title: "Intro"
version: "0.1"
---

Intro paragraph.

## Step

Body.
"#;
        let result = parse_template(source);
        assert!(matches!(
            result.unwrap_err(),
            ParseError::ContentBeforeFirstStep
        ));
    }

    #[test]
    fn test_missing_step_heading_errors() {
        let source = r#"---
id: NO-STEPS
title: "No steps"
version: "0.1"
---

Only prose.
"#;
        let result = parse_template(source);
        assert!(matches!(
            result.unwrap_err(),
            ParseError::MissingStepHeading
        ));
    }

    #[test]
    fn test_minimal_template() {
        let source = r#"---
id: MIN-001
title: "Minimal"
version: "0.1"
---

## Only Step

Just some text.
"#;
        let template = parse_template(source).unwrap();
        assert_eq!(template.metadata.id, "MIN-001");
        assert_eq!(template.steps.len(), 1);
        assert_eq!(template.steps[0].heading, "Only Step");
    }

    #[test]
    fn test_prose_preserves_markdown() {
        let source = r#"---
id: MD-001
title: "Markdown Test"
version: "0.1"
---

## Step with rich prose

Here is a paragraph with **bold** and *italic* text.

- bullet point 1
- bullet point 2

### A sub-heading

```python
print("hello")
```

Some trailing text.

```inputs
- id: val
  label: "Value"
  type: measurement
  unit: "V"
```
"#;
        let template = parse_template(source).unwrap();
        assert_eq!(template.steps.len(), 1);

        let prose_parts: Vec<_> = template.steps[0]
            .content
            .iter()
            .filter_map(|c| match c {
                StepContent::Prose { text } => Some(text.clone()),
                _ => None,
            })
            .collect();

        assert_eq!(prose_parts.len(), 1);
        let prose = &prose_parts[0];
        // All Markdown elements should be preserved as raw text.
        assert!(prose.contains("**bold**"), "bold not preserved: {prose}");
        assert!(prose.contains("*italic*"), "italic not preserved: {prose}");
        assert!(
            prose.contains("- bullet point 1"),
            "bullet list not preserved: {prose}"
        );
        assert!(
            prose.contains("### A sub-heading"),
            "sub-heading not preserved: {prose}"
        );
        assert!(
            prose.contains("```python"),
            "code block not preserved: {prose}"
        );
        assert!(
            prose.contains("Some trailing text."),
            "trailing text not preserved: {prose}"
        );
    }

    #[test]
    fn test_prose_between_checkboxes_and_inputs() {
        let source = r#"---
id: MIX-001
title: "Mixed Content"
version: "0.1"
---

## Mixed Step

- [ ] First check
- [ ] Second check

Some prose between checkboxes and inputs.

```inputs
- id: val
  label: "Value"
  type: measurement
  unit: "V"
```
"#;
        let template = parse_template(source).unwrap();
        assert_eq!(template.steps.len(), 1);

        let content = &template.steps[0].content;
        // Should have: Checkbox, Checkbox, Prose, InputBlock
        assert_eq!(content.len(), 4, "expected 4 content items: {content:?}");
        assert!(matches!(content[0], StepContent::Checkbox { .. }));
        assert!(matches!(content[1], StepContent::Checkbox { .. }));
        assert!(matches!(content[2], StepContent::Prose { .. }));
        assert!(matches!(content[3], StepContent::InputBlock { .. }));

        if let StepContent::Prose { text } = &content[2] {
            assert!(text.contains("Some prose between checkboxes and inputs"));
        }
    }

    #[test]
    fn test_mixed_regular_and_checkbox_items_as_prose() {
        let source = r#"---
id: MIX-002
title: "Mixed List Test"
version: "0.1"
---

## Step with mixed list

- bullet point 1
- [ ] a checkbox item
  - [ ] a nested checkbox item
"#;
        let template = parse_template(source).unwrap();
        assert_eq!(template.steps.len(), 1);

        let content = &template.steps[0].content;
        // Mixed list should be captured entirely as prose (not interactive checkboxes).
        assert_eq!(
            content.len(),
            1,
            "expected 1 content item (prose): {content:?}"
        );
        assert!(matches!(content[0], StepContent::Prose { .. }));

        if let StepContent::Prose { text } = &content[0] {
            assert!(
                text.contains("bullet point 1"),
                "should contain bullet: {text}"
            );
            assert!(
                text.contains("[ ] a checkbox item"),
                "should contain checkbox text: {text}"
            );
        }
    }

    #[test]
    fn test_regular_bullet_with_nested_task_list_stays_prose() {
        let source = r#"---
id: NEST-MIX-001
title: "Nested Mixed List Test"
version: "0.1"
---

## Step with nested task list

- prose item
  - [ ] nested checkbox
"#;
        let template = parse_template(source).unwrap();
        assert_eq!(template.steps.len(), 1);
        assert_eq!(template.steps[0].content.len(), 1);
        let StepContent::Prose { text } = &template.steps[0].content[0] else {
            panic!("expected prose: {:?}", template.steps[0].content);
        };
        assert!(text.contains("- prose item"));
        assert!(text.contains("[ ] nested checkbox"));
    }

    #[test]
    fn test_nested_pure_checkboxes() {
        let source = r#"---
id: NEST-001
title: "Nested Checkbox Test"
version: "0.1"
---

## Step with nested checkboxes

- [ ] parent checkbox
  - [ ] nested checkbox
"#;
        let template = parse_template(source).unwrap();
        assert_eq!(template.steps.len(), 1);

        let checkboxes: Vec<_> = template.steps[0]
            .content
            .iter()
            .filter_map(|c| match c {
                StepContent::Checkbox { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect();

        // Both should be captured as independent checkboxes with no duplicated prose.
        assert_eq!(
            checkboxes,
            vec!["parent checkbox".to_string(), "nested checkbox".to_string()]
        );
        assert_eq!(
            template.steps[0]
                .content
                .iter()
                .filter(|content| matches!(content, StepContent::Prose { .. }))
                .count(),
            0
        );
    }

    #[test]
    fn test_regular_bullet_list_as_prose() {
        let source = r#"---
id: BULLET-001
title: "Bullet List Test"
version: "0.1"
---

## Step with bullets

- item 1
- item 2
- item 3
"#;
        let template = parse_template(source).unwrap();
        assert_eq!(template.steps.len(), 1);

        let content = &template.steps[0].content;
        // Pure bullet list (no checkboxes) should be prose.
        assert_eq!(content.len(), 1, "expected 1 content item: {content:?}");
        assert!(matches!(content[0], StepContent::Prose { .. }));

        if let StepContent::Prose { text } = &content[0] {
            assert!(
                text.contains("- item 1"),
                "should contain bullet items: {text}"
            );
            assert!(
                text.contains("- item 3"),
                "should contain all items: {text}"
            );
        }
    }

    #[test]
    fn test_pure_task_list_then_regular_list() {
        let source = r#"---
id: SEQ-001
title: "Sequential Lists"
version: "0.1"
---

## Step with sequential lists

- [ ] check 1
- [ ] check 2

Some text between.

- bullet A
- bullet B
"#;
        let template = parse_template(source).unwrap();
        assert_eq!(template.steps.len(), 1);

        let content = &template.steps[0].content;
        // Should have: Checkbox, Checkbox, Prose (text + bullet list)
        let checkbox_count = content
            .iter()
            .filter(|c| matches!(c, StepContent::Checkbox { .. }))
            .count();
        let prose_count = content
            .iter()
            .filter(|c| matches!(c, StepContent::Prose { .. }))
            .count();

        assert_eq!(checkbox_count, 2, "expected 2 checkboxes: {content:?}");
        assert!(
            prose_count >= 1,
            "expected at least 1 prose block: {content:?}"
        );
    }

    #[test]
    fn test_same_marker_adjacent_lists_merged_as_prose() {
        // pulldown-cmark merges adjacent lists using the same bullet marker into
        // a single list. When that list contains both task items and regular items,
        // the whole list becomes prose.
        let source = r#"---
id: MERGE-001
title: "Merged List"
version: "0.1"
---

## Step

- [ ] check 1
- [ ] check 2

- bullet A
- bullet B
"#;
        let template = parse_template(source).unwrap();
        assert_eq!(template.steps.len(), 1);

        let content = &template.steps[0].content;
        // pulldown-cmark treats all 4 items as one list → mixed → prose
        assert_eq!(content.len(), 1, "expected 1 prose block: {content:?}");
        assert!(matches!(content[0], StepContent::Prose { .. }));
    }
}
