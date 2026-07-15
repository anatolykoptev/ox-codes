use serde::{Deserialize, Serialize};

/// Describes how data flows through a function call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlowSemantic {
    /// Function name pattern (e.g., "strings.Replace", "fmt.Sprintf")
    pub method: String,
    /// Flow mappings
    pub mappings: Vec<FlowMapping>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FlowMapping {
    /// arg[src] flows to return value
    ArgToReturn(usize),
    /// arg[src] flows to arg[dst]
    ArgToArg { src: usize, dst: usize },
    /// All args flow to return (conservative)
    PassThrough,
    /// No data flows through (sanitizer-like)
    Block,
}

/// Get built-in flow semantics for a language.
pub fn builtin_semantics(lang: &str) -> Vec<FlowSemantic> {
    match lang {
        "go" | "golang" => go_semantics(),
        "python" | "py" => python_semantics(),
        _ => vec![],
    }
}

fn go_semantics() -> Vec<FlowSemantic> {
    vec![
        // String operations — arg0 flows to return
        FlowSemantic {
            method: "fmt.Sprintf".into(),
            mappings: vec![FlowMapping::PassThrough],
        },
        FlowSemantic {
            method: "strings.Replace".into(),
            mappings: vec![FlowMapping::ArgToReturn(0)],
        },
        FlowSemantic {
            method: "strings.ToLower".into(),
            mappings: vec![FlowMapping::ArgToReturn(0)],
        },
        FlowSemantic {
            method: "strings.ToUpper".into(),
            mappings: vec![FlowMapping::ArgToReturn(0)],
        },
        FlowSemantic {
            method: "strings.TrimSpace".into(),
            mappings: vec![FlowMapping::ArgToReturn(0)],
        },
        FlowSemantic {
            method: "strconv.Atoi".into(),
            mappings: vec![FlowMapping::ArgToReturn(0)],
        },
        // Sanitizers — block taint flow
        FlowSemantic {
            method: "html.EscapeString".into(),
            mappings: vec![FlowMapping::Block],
        },
        FlowSemantic {
            method: "url.QueryEscape".into(),
            mappings: vec![FlowMapping::Block],
        },
        FlowSemantic {
            method: "filepath.Clean".into(),
            mappings: vec![FlowMapping::Block],
        },
        // IO — arg1 (src) flows to arg0 (dst)
        FlowSemantic {
            method: "io.Copy".into(),
            mappings: vec![FlowMapping::ArgToArg { src: 1, dst: 0 }],
        },
        // Encoding
        FlowSemantic {
            method: "json.Marshal".into(),
            mappings: vec![FlowMapping::ArgToReturn(0)],
        },
        FlowSemantic {
            method: "json.Unmarshal".into(),
            mappings: vec![FlowMapping::ArgToArg { src: 0, dst: 1 }],
        },
    ]
}

fn python_semantics() -> Vec<FlowSemantic> {
    vec![
        FlowSemantic {
            method: "str.format".into(),
            mappings: vec![FlowMapping::PassThrough],
        },
        FlowSemantic {
            method: "str.replace".into(),
            mappings: vec![FlowMapping::ArgToReturn(0)],
        },
        FlowSemantic {
            method: "str.lower".into(),
            mappings: vec![FlowMapping::ArgToReturn(0)],
        },
        FlowSemantic {
            method: "str.strip".into(),
            mappings: vec![FlowMapping::ArgToReturn(0)],
        },
        // Sanitizers
        FlowSemantic {
            method: "html.escape".into(),
            mappings: vec![FlowMapping::Block],
        },
        FlowSemantic {
            method: "shlex.quote".into(),
            mappings: vec![FlowMapping::Block],
        },
        // SQL parameterized queries — block taint
        FlowSemantic {
            method: "cursor.execute".into(),
            mappings: vec![FlowMapping::Block],
        },
    ]
}

/// Look up semantics for a function name.
pub fn lookup<'a>(semantics: &'a [FlowSemantic], func_name: &str) -> Option<&'a FlowSemantic> {
    semantics.iter().find(|s| {
        func_name == s.method
            || func_name.ends_with(&format!(".{}", s.method.rsplit('.').next().unwrap_or("")))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn go_semantics_non_empty() {
        let sem = builtin_semantics("go");
        assert!(!sem.is_empty());
        assert!(sem.len() >= 10);
    }

    #[test]
    fn python_semantics_non_empty() {
        let sem = builtin_semantics("python");
        assert!(!sem.is_empty());
        assert!(sem.len() >= 5);
    }

    #[test]
    fn lookup_finds_exact_match() {
        let sem = builtin_semantics("go");
        let found = lookup(&sem, "html.EscapeString");
        assert!(found.is_some());
        assert_eq!(found.unwrap().method, "html.EscapeString");
    }

    #[test]
    fn lookup_finds_suffix_match() {
        let sem = builtin_semantics("go");
        let found = lookup(&sem, "template.EscapeString");
        assert!(found.is_some());
        assert_eq!(found.unwrap().method, "html.EscapeString");
    }

    #[test]
    fn lookup_returns_none_for_unknown() {
        let sem = builtin_semantics("go");
        assert!(lookup(&sem, "foo.Bar").is_none());
    }

    #[test]
    fn unknown_language_returns_empty() {
        assert!(builtin_semantics("unknown").is_empty());
        assert!(builtin_semantics("ruby").is_empty());
    }

    #[test]
    fn golang_alias_works() {
        let go = builtin_semantics("go");
        let golang = builtin_semantics("golang");
        assert_eq!(go.len(), golang.len());
    }

    #[test]
    fn python_alias_works() {
        let py = builtin_semantics("py");
        let python = builtin_semantics("python");
        assert_eq!(py.len(), python.len());
    }
}
