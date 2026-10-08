use serde::{Deserialize, Serialize};

/// The agent-authored script, as supplied. Only [`super::Walkthrough`] has been checked
/// against a model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Script {
    pub schema_version: String,
    pub id: String,
    pub model_id: String,
    pub expected_revision: u64,
    pub title: String,
    pub steps: Vec<Step>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Step {
    pub id: String,
    pub title: String,
    pub root: Option<String>,
    pub focus: Vec<String>,
    /// None means `hide` for a traced step and `dim` otherwise; see [`Step::context`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<Context>,
    #[serde(default)]
    pub reveal: Reveal,
    #[serde(default)]
    pub trace: Vec<Hop>,
    #[serde(default)]
    pub repeat_trace: bool,
    #[serde(default)]
    pub labels: Vec<Label>,
    #[serde(default)]
    pub camera: CameraIntent,
    #[serde(default)]
    pub presentation: Presentation,
}

impl Step {
    /// The context treatment this step plays with. A traced step defaults to
    /// `hide` so the path reads on its own; other steps default to `dim`.
    pub fn context(&self) -> Context {
        self.context.unwrap_or(if self.trace.is_empty() {
            Context::Dim
        } else {
            Context::Hide
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Context {
    #[default]
    Dim,
    Hide,
    Show,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reveal {
    #[default]
    Step,
    Cumulative,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CameraIntent {
    Overview,
    #[default]
    FrameFocus,
    FollowTrace,
    CloseUp,
    Hold,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Presentation {
    #[default]
    Assembled,
    Exploded,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hop {
    pub relation: String,
    #[serde(default, rename = "return")]
    pub returning: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Label {
    pub target: String,
    pub text: String,
}

/// One hop after validation: the endpoints the animation travels between, in travel order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ResolvedHop {
    pub relation: String,
    pub from: String,
    pub to: String,
    pub returning: bool,
}

/// What validation derived for one step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ResolvedStep {
    /// The supplied root, or the closest common owner of the focus.
    pub root: String,
    pub hops: Vec<ResolvedHop>,
}
