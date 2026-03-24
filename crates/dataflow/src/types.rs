use serde::Serialize;
use smallvec::SmallVec;

/// Unique variable ID (Semgrep sid pattern — resolves shadowing).
pub type Sid = u32;

/// Text range in source.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct Span {
    pub start_byte: usize,
    pub end_byte: usize,
    pub start_line: usize, // 1-indexed
    pub end_line: usize,
}

/// Constant value for propagation.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum ConstValue {
    Int(i64),
    Float(f64),
    Str(String),
    Bool(bool),
    Nil,
}

/// Taint tag.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub enum TaintTag {
    UserInput,
    FileSystem,
    Network,
    Database,
    Environment,
    Custom(String),
}

/// A variable binding in a scope.
#[derive(Debug, Clone, Serialize)]
pub struct VarBinding {
    pub name: String,
    pub sid: Sid,
    pub def_site: Span,
    pub def_value: Option<ConstValue>,
    pub taint_tags: SmallVec<[TaintTag; 2]>,
    pub uses: Vec<Span>,
    pub is_param: bool,
}

impl VarBinding {
    pub fn is_read(&self) -> bool {
        !self.uses.is_empty()
    }
}

/// Scope kind.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub enum ScopeKind {
    Module,
    Function,
    Block,
    Loop,
}

/// A lexical scope with variable bindings.
#[derive(Debug, Clone, Serialize)]
pub struct Scope {
    pub kind: ScopeKind,
    pub vars: Vec<VarBinding>,
    pub span: Span,
}

/// Chain of scopes for a function/file.
#[derive(Debug, Clone, Serialize)]
pub struct ScopeChain {
    pub scopes: Vec<Scope>,
}

/// Finding severity.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
    Info,
}

/// Finding kind.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingKind {
    DeadStore,
    UnusedVariable,
    ConstantValue,
    // Future: UninitializedVar, NullDeref, TaintedSink
}

/// A data-flow finding.
#[derive(Debug, Clone, Serialize)]
pub struct Finding {
    pub kind: FindingKind,
    pub severity: Severity,
    pub message: String,
    pub file: String,
    pub span: Span,
    pub variable: String,
}

/// Request for dataflow analysis.
#[derive(Debug, serde::Deserialize)]
pub struct DataflowInput {
    pub root: String,
    pub language: String,
    #[serde(default = "default_max_results")]
    pub max_results: usize,
    #[serde(default)]
    pub file_glob: Option<String>,
    #[serde(default)]
    pub exclude_glob: Option<String>,
}

/// Response from dataflow analysis.
#[derive(Debug, Serialize)]
pub struct DataflowResponse {
    pub findings: Vec<Finding>,
    pub total_findings: usize,
    pub files_analyzed: usize,
    pub truncated: bool,
    pub duration_ms: u64,
}

fn default_max_results() -> usize {
    100
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn var_binding_is_read_empty() {
        let binding = VarBinding {
            name: "x".into(),
            sid: 1,
            def_site: Span {
                start_byte: 0,
                end_byte: 5,
                start_line: 1,
                end_line: 1,
            },
            def_value: None,
            taint_tags: SmallVec::new(),
            uses: vec![],
            is_param: false,
        };
        assert!(!binding.is_read());
    }

    #[test]
    fn var_binding_is_read_with_uses() {
        let span = Span {
            start_byte: 0,
            end_byte: 5,
            start_line: 1,
            end_line: 1,
        };
        let binding = VarBinding {
            name: "x".into(),
            sid: 1,
            def_site: span,
            def_value: Some(ConstValue::Int(42)),
            taint_tags: SmallVec::new(),
            uses: vec![span],
            is_param: false,
        };
        assert!(binding.is_read());
    }
}
