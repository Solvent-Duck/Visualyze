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
    /// Temporary flattened views over a subset of the model (ADR 0010).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub teaching_views: Vec<TeachingView>,
    pub steps: Vec<Step>,
}

/// An agent-chosen subset of the model, laid out on its own for explanation. It cites
/// existing entity IDs only; the viewer derives everything else from the model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeachingView {
    pub id: String,
    pub title: String,
    pub members: Vec<String>,
    #[serde(default)]
    pub layout: TeachingLayout,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TeachingLayout {
    #[default]
    Pipeline,
    Peers,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Step {
    pub id: String,
    pub title: String,
    pub root: Option<String>,
    /// A teaching view ID from [`Script::teaching_views`]; the step plays inside it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub view: Option<String>,
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
    /// Index into [`super::Walkthrough::teaching_views`] when the step plays in one.
    pub view: Option<usize>,
}

/// What validation derived for one teaching view, against the full model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ResolvedTeachingView {
    pub id: String,
    /// Members plus the interfaces they expose.
    pub entities: std::collections::BTreeSet<String>,
    /// Relations between shown entities, other than containment.
    pub relations: std::collections::BTreeSet<String>,
    /// Flow relations with exactly one end in the view; they are not drawn.
    pub outside_connections: usize,
    /// Entities in the full model, for the honesty badge.
    pub model_entities: usize,
}
