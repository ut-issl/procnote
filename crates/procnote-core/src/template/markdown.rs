//! Writing a [`ProcedureTemplate`] as procnote Markdown, the format
//! [`parse_template`] reads.
//!
//! The output is checked by parsing it back: a template whose content the
//! format can't hold (for example prose containing a level-2 heading, which
//! would start a new step) is reported instead of written differently.

use serde::Serialize;

use super::parse_template;
use super::types::{
    ExpectedValue, InputDefinition, InputType, ProcedureMetadata, ProcedureTemplate, Step,
    StepContent,
};

/// Errors that can occur when writing a template as Markdown.
#[derive(Debug, thiserror::Error)]
pub enum MarkdownError {
    #[error("failed to write YAML: {0}")]
    Yaml(#[from] serde_yaml_ng::Error),
    #[error("the written template can't be read back: {0}")]
    Unreadable(#[from] super::parser::ParseError),
    #[error("the template can't be written as procnote Markdown: {0}")]
    NotRepresentable(String),
}

/// Write `template` as procnote Markdown.
///
/// Step and checkbox IDs are not written (they are assigned at execution
/// start), heading and checkbox text is trimmed with line breaks as spaces,
/// and consecutive prose items are joined, as the parser reads them.
pub fn to_markdown(template: &ProcedureTemplate) -> Result<String, MarkdownError> {
    let template = normalized(template);
    // Blocks of the document, separated by blank lines.
    let mut blocks = vec![format!(
        "---\n{}---",
        serde_yaml_ng::to_string(&FrontMatter::from(&template.metadata))?
    )];
    for step in &template.steps {
        blocks.push(format!("## {}", escape_inline(&step.heading)));
        // Consecutive checkboxes form one task list.
        let mut checklist: Vec<String> = Vec::new();
        for item in &step.content {
            if let StepContent::Checkbox {
                text,
                checked,
                nesting_level,
                ..
            } = item
            {
                let indent = "  ".repeat(nesting_level.unwrap_or(0) as usize);
                let mark = if *checked { "x" } else { " " };
                checklist.push(format!("{indent}- [{mark}] {}", escape_inline(text)));
                continue;
            }
            if !checklist.is_empty() {
                blocks.push(checklist.join("\n"));
                checklist.clear();
            }
            match item {
                StepContent::Prose { text } => blocks.push(text.clone()),
                StepContent::InputBlock { inputs } => {
                    let fields: Vec<InputFields<'_>> =
                        inputs.iter().map(InputFields::from).collect();
                    blocks.push(format!(
                        "```inputs\n{}```",
                        serde_yaml_ng::to_string(&fields)?
                    ));
                }
                StepContent::Checkbox { .. } => {}
            }
        }
        if !checklist.is_empty() {
            blocks.push(checklist.join("\n"));
        }
    }
    let out = blocks.join("\n\n") + "\n";

    let read_back = parse_template(&out)?;
    if let Some(difference) = first_difference(&template, &read_back) {
        return Err(MarkdownError::NotRepresentable(difference));
    }
    Ok(out)
}

/// The template as the parser would read it back, if the format can hold it.
fn normalized(template: &ProcedureTemplate) -> ProcedureTemplate {
    let one_line = |text: &str| text.split_whitespace().collect::<Vec<_>>().join(" ");
    let steps = template
        .steps
        .iter()
        .map(|step| {
            let mut content: Vec<StepContent> = Vec::new();
            for item in &step.content {
                match (content.last_mut(), item) {
                    (_, StepContent::Prose { text }) if text.trim().is_empty() => {}
                    (Some(StepContent::Prose { text: previous }), StepContent::Prose { text }) => {
                        previous.push_str("\n\n");
                        previous.push_str(text.trim());
                    }
                    (_, StepContent::Prose { text }) => content.push(StepContent::Prose {
                        text: text.trim().to_string(),
                    }),
                    (
                        _,
                        StepContent::Checkbox {
                            text,
                            checked,
                            nesting_level,
                            ..
                        },
                    ) => content.push(StepContent::Checkbox {
                        id: None,
                        text: one_line(text),
                        checked: *checked,
                        nesting_level: nesting_level.filter(|level| *level > 0),
                    }),
                    (_, StepContent::InputBlock { inputs }) => {
                        content.push(StepContent::InputBlock {
                            inputs: inputs.clone(),
                        });
                    }
                }
            }
            Step {
                id: None,
                heading: one_line(&step.heading),
                content,
            }
        })
        .collect();
    ProcedureTemplate {
        metadata: template.metadata.clone(),
        steps,
    }
}

/// Where the read-back template first differs, in words.
fn first_difference(written: &ProcedureTemplate, read: &ProcedureTemplate) -> Option<String> {
    if written.metadata != read.metadata {
        return Some("the metadata reads back differently".to_string());
    }
    if written.steps.len() != read.steps.len() {
        return Some(format!(
            "it reads back as {} steps instead of {} (does some prose contain a `##` heading?)",
            read.steps.len(),
            written.steps.len()
        ));
    }
    written
        .steps
        .iter()
        .zip(&read.steps)
        .find(|(a, b)| a != b)
        .map(|(step, _)| {
            format!(
                "step \"{}\" reads back differently (prose may be merging with a checklist or \
                 containing one)",
                step.heading
            )
        })
}

/// Backslash-escape the characters that would make text Markdown syntax
/// rather than literal text, and a leading ordered-list marker.
fn escape_inline(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if matches!(
            c,
            '\\' | '`' | '*' | '_' | '[' | ']' | '<' | '>' | '#' | '!' | '|' | '~' | '&'
        ) {
            out.push('\\');
        }
        out.push(c);
    }
    // "1. text" or "1) text" would start an ordered list inside a list item.
    let digits = out.chars().take_while(char::is_ascii_digit).count();
    if digits > 0 && matches!(out[digits..].chars().next(), Some('.' | ')')) {
        out.insert(digits, '\\');
    }
    // A leading "-", "+", or "=" can become a list marker or a heading underline.
    if out.starts_with(['-', '+', '=']) {
        out.insert(0, '\\');
    }
    out
}

/// The frontmatter, without empty optional fields.
#[derive(Serialize)]
struct FrontMatter<'a> {
    id: &'a str,
    title: &'a str,
    version: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    author: Option<&'a str>,
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    equipment: &'a [super::types::Equipment],
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    requirement_traces: &'a [String],
}

impl<'a> From<&'a ProcedureMetadata> for FrontMatter<'a> {
    fn from(metadata: &'a ProcedureMetadata) -> Self {
        Self {
            id: &metadata.id,
            title: &metadata.title,
            version: &metadata.version,
            author: metadata.author.as_deref(),
            equipment: &metadata.equipment,
            requirement_traces: &metadata.requirement_traces,
        }
    }
}

/// An input in an `inputs` block, without empty optional fields.
#[derive(Serialize)]
struct InputFields<'a> {
    id: &'a str,
    label: &'a str,
    #[serde(rename = "type")]
    input_type: &'a InputType,
    #[serde(skip_serializing_if = "Option::is_none")]
    unit: Option<&'a str>,
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    options: &'a [String],
    #[serde(skip_serializing_if = "Option::is_none")]
    expected: Option<&'a ExpectedValue>,
}

impl<'a> From<&'a InputDefinition> for InputFields<'a> {
    fn from(input: &'a InputDefinition) -> Self {
        Self {
            id: &input.id,
            label: &input.label,
            input_type: &input.input_type,
            unit: input.unit.as_deref(),
            options: &input.options,
            expected: input.expected.as_ref(),
        }
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "unwrap is acceptable in tests")]
mod tests {
    use super::*;

    fn round_trip(source: &str) {
        let template = parse_template(source).unwrap();
        let written = to_markdown(&template).unwrap();
        assert_eq!(parse_template(&written).unwrap(), template, "{written}");
    }

    #[test]
    fn workspace_templates_round_trip() {
        let procedures = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../procedures");
        let mut count = 0;
        for entry in std::fs::read_dir(procedures).unwrap() {
            let path = entry.unwrap().path().join("template.md");
            if path.exists() {
                round_trip(&std::fs::read_to_string(&path).unwrap());
                count += 1;
            }
        }
        assert!(count > 0);
    }

    #[test]
    fn literal_text_stays_literal() {
        let checkbox = |text: &str, level: u32| StepContent::Checkbox {
            id: None,
            text: text.to_string(),
            checked: level == 1,
            nesting_level: (level > 0).then_some(level),
        };
        let template = ProcedureTemplate {
            metadata: parse_template("---\nid: X\ntitle: T\nversion: '1'\n---\n\n## S\n")
                .unwrap()
                .metadata,
            steps: vec![Step {
                id: None,
                heading: "Step *1* #2 [a] <b> `c` 3.".to_string(),
                content: vec![
                    checkbox("1. not a list", 0),
                    checkbox("- dash", 1),
                    checkbox("snake_case and **stars**", 2),
                    checkbox("pipe | amp & bang! ~tilde~ \\ backslash", 1),
                ],
            }],
        };

        let written = to_markdown(&template).unwrap();

        assert_eq!(parse_template(&written).unwrap(), template, "{written}");
    }

    #[test]
    fn inputs_keep_every_kind_of_expectation() {
        round_trip(
            "---\nid: X\ntitle: T\nversion: '1'\nauthor: A\nrequirement_traces: [R1]\n---\n\n\
             ## S\n\nIntro **bold**.\n\n```inputs\n\
             - id: v\n  label: Voltage\n  type: measurement\n  unit: V\n  expected: {min: 1.5, max: 2}\n\
             - id: r\n  label: Result\n  type: selection\n  options: [PASS, FAIL]\n  expected: PASS\n\
             - id: n\n  label: Number as text\n  type: text\n  expected: '1.5'\n\
             ```\n\nAfter.\n",
        );
    }

    #[test]
    fn prose_with_a_step_heading_is_reported() {
        let template = ProcedureTemplate {
            metadata: parse_template("---\nid: X\ntitle: T\nversion: '1'\n---\n\n## S\n")
                .unwrap()
                .metadata,
            steps: vec![Step {
                id: None,
                heading: "S".to_string(),
                content: vec![StepContent::Prose {
                    text: "## Not a step".to_string(),
                }],
            }],
        };

        assert!(matches!(
            to_markdown(&template),
            Err(MarkdownError::NotRepresentable(_))
        ));
    }

    #[test]
    fn ids_and_whitespace_are_normalized() {
        let source = "---\nid: X\ntitle: T\nversion: '1'\n---\n\n## S\n\n- [ ] a\n";
        let mut template = parse_template(source).unwrap();
        template.steps[0].id = Some("step-0".to_string());
        template.steps[0].heading = "  S  ".to_string();
        if let StepContent::Checkbox { id, text, .. } = &mut template.steps[0].content[0] {
            *id = Some("step-0/cb-0".to_string());
            *text = "a\n".to_string();
        }

        assert_eq!(
            parse_template(&to_markdown(&template).unwrap()).unwrap(),
            parse_template(source).unwrap()
        );
    }
}
