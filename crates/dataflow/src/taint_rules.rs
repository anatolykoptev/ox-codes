//! Built-in taint rules for common vulnerability patterns.
//!
//! Hardcoded for now — YAML loading can be added later.

/// A taint rule: sources + sinks + sanitizers.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct TaintRule {
    pub id: String,
    pub sources: Vec<crate::taint::TaintSource>,
    pub sinks: Vec<crate::taint::TaintSink>,
    #[serde(default)]
    pub sanitizers: Vec<crate::taint::Sanitizer>,
    pub severity: String,
}

/// Get default taint rules for a language.
pub fn default_rules(lang: &str) -> Vec<TaintRule> {
    match lang {
        "go" | "golang" => go_rules(),
        "python" | "py" => python_rules(),
        _ => vec![],
    }
}

fn go_rules() -> Vec<TaintRule> {
    use crate::taint::{Sanitizer, TaintSink, TaintSource};
    vec![
        TaintRule {
            id: "sql-injection".into(),
            sources: vec![
                TaintSource {
                    pattern: "FormValue".into(),
                    tag: "user_input".into(),
                },
                TaintSource {
                    pattern: "Query".into(),
                    tag: "user_input".into(),
                },
                TaintSource {
                    pattern: "ReadAll".into(),
                    tag: "user_input".into(),
                },
                TaintSource {
                    pattern: "Getenv".into(),
                    tag: "env".into(),
                },
            ],
            sinks: vec![TaintSink {
                pattern: "Exec".into(),
                arg_index: 0,
                cwe: "CWE-89".into(),
                description: "SQL injection".into(),
            }],
            sanitizers: vec![
                Sanitizer {
                    pattern: "EscapeString".into(),
                },
                Sanitizer {
                    pattern: "QuoteIdentifier".into(),
                },
            ],
            severity: "error".into(),
        },
        TaintRule {
            id: "command-injection".into(),
            sources: vec![
                TaintSource {
                    pattern: "FormValue".into(),
                    tag: "user_input".into(),
                },
                TaintSource {
                    pattern: "ReadAll".into(),
                    tag: "user_input".into(),
                },
            ],
            sinks: vec![TaintSink {
                pattern: "Command".into(),
                arg_index: -1,
                cwe: "CWE-78".into(),
                description: "OS command injection".into(),
            }],
            sanitizers: vec![],
            severity: "error".into(),
        },
        TaintRule {
            id: "xss".into(),
            sources: vec![TaintSource {
                pattern: "FormValue".into(),
                tag: "user_input".into(),
            }],
            sinks: vec![
                TaintSink {
                    pattern: "Fprintf".into(),
                    arg_index: -1,
                    cwe: "CWE-79".into(),
                    description: "Cross-site scripting".into(),
                },
                TaintSink {
                    pattern: "Write".into(),
                    arg_index: 0,
                    cwe: "CWE-79".into(),
                    description: "Cross-site scripting".into(),
                },
            ],
            sanitizers: vec![
                Sanitizer {
                    pattern: "EscapeString".into(),
                },
                Sanitizer {
                    pattern: "HTMLEscapeString".into(),
                },
            ],
            severity: "warning".into(),
        },
    ]
}

fn python_rules() -> Vec<TaintRule> {
    use crate::taint::{TaintSink, TaintSource};
    vec![
        TaintRule {
            id: "sql-injection".into(),
            sources: vec![
                TaintSource {
                    pattern: "input".into(),
                    tag: "user_input".into(),
                },
                TaintSource {
                    pattern: "getenv".into(),
                    tag: "env".into(),
                },
            ],
            sinks: vec![TaintSink {
                pattern: "execute".into(),
                arg_index: 0,
                cwe: "CWE-89".into(),
                description: "SQL injection".into(),
            }],
            sanitizers: vec![],
            severity: "error".into(),
        },
        TaintRule {
            id: "command-injection".into(),
            sources: vec![TaintSource {
                pattern: "input".into(),
                tag: "user_input".into(),
            }],
            sinks: vec![
                TaintSink {
                    pattern: "system".into(),
                    arg_index: 0,
                    cwe: "CWE-78".into(),
                    description: "OS command injection".into(),
                },
                TaintSink {
                    pattern: "popen".into(),
                    arg_index: 0,
                    cwe: "CWE-78".into(),
                    description: "OS command injection".into(),
                },
            ],
            sanitizers: vec![],
            severity: "error".into(),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn go_rules_non_empty() {
        let rules = default_rules("go");
        assert!(!rules.is_empty());
        let ids: Vec<_> = rules.iter().map(|r| r.id.as_str()).collect();
        assert!(ids.contains(&"sql-injection"));
        assert!(ids.contains(&"command-injection"));
        assert!(ids.contains(&"xss"));
    }

    #[test]
    fn python_rules_non_empty() {
        let rules = default_rules("python");
        assert!(!rules.is_empty());
        let ids: Vec<_> = rules.iter().map(|r| r.id.as_str()).collect();
        assert!(ids.contains(&"sql-injection"));
        assert!(ids.contains(&"command-injection"));
    }

    #[test]
    fn unknown_lang_returns_empty() {
        assert!(default_rules("haskell").is_empty());
        assert!(default_rules("").is_empty());
    }

    #[test]
    fn go_sql_injection_has_sources_and_sinks() {
        let rules = default_rules("go");
        let sqli = rules.iter().find(|r| r.id == "sql-injection").unwrap();
        assert!(!sqli.sources.is_empty());
        assert!(!sqli.sinks.is_empty());
        assert_eq!(sqli.severity, "error");
        assert!(sqli.sinks.iter().any(|s| s.cwe == "CWE-89"));
    }
}
