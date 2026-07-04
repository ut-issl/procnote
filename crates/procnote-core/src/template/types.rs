use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// A complete procedure template parsed from a Markdown file.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
#[ts(export)]
pub struct ProcedureTemplate {
    pub metadata: ProcedureMetadata,
    pub steps: Vec<Step>,
}

/// YAML frontmatter metadata for a procedure.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
#[ts(export)]
pub struct ProcedureMetadata {
    pub id: String,
    pub title: String,
    pub version: String,
    #[serde(default)]
    #[ts(optional)]
    pub author: Option<String>,
    #[serde(default)]
    pub equipment: Vec<Equipment>,
    #[serde(default)]
    pub requirement_traces: Vec<String>,
}

/// A piece of equipment referenced by the procedure.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
#[ts(export)]
pub struct Equipment {
    pub id: String,
    pub name: String,
}

/// A single step in the procedure (corresponds to a `## ` heading).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
#[ts(export)]
pub struct Step {
    /// Stable element ID, assigned at execution start. `None` in raw templates.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub id: Option<String>,
    pub heading: String,
    pub content: Vec<StepContent>,
}

/// Content items within a step.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
#[ts(export)]
#[serde(tag = "type")]
pub enum StepContent {
    /// Free-form prose text (Markdown source).
    Prose { text: String },
    /// A checkbox item from a task list (`- [ ]` or `- [x]`).
    Checkbox {
        /// Stable element ID, assigned at execution start. `None` in raw templates.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        id: Option<String>,
        text: String,
        checked: bool,
        /// Zero-based nesting level within a pure Markdown task list.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        nesting_level: Option<u32>,
    },
    /// A block of input definitions from a fenced `inputs` code block.
    InputBlock { inputs: Vec<InputDefinition> },
}

/// Definition of an input field that operators fill in during execution.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
#[ts(export)]
pub struct InputDefinition {
    pub id: String,
    pub label: String,
    #[serde(rename = "type")]
    pub input_type: InputType,
    #[serde(default)]
    #[ts(optional)]
    pub unit: Option<String>,
    #[serde(default)]
    pub options: Vec<String>,
    #[serde(default)]
    #[ts(optional)]
    pub expected: Option<ExpectedValue>,
}

/// The type of input an operator provides.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
#[ts(export)]
#[serde(rename_all = "lowercase")]
pub enum InputType {
    Measurement,
    Text,
    Selection,
    Attachment,
}

/// Expected value for validation — either a range or an exact match.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
#[ts(export)]
#[serde(untagged)]
pub enum ExpectedValue {
    Range { min: f64, max: f64 },
    Exact(String),
}
