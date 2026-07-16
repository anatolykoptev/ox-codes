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
        "rust" | "rs" => rust_rules(),
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

fn rust_rules() -> Vec<TaintRule> {
    use crate::taint::{Sanitizer, TaintSink, TaintSource};
    vec![
        // CWE-78: OS Command Injection
        TaintRule {
            id: "rust-command-injection".into(),
            sources: vec![
                TaintSource {
                    pattern: "env::var".into(),
                    tag: "env".into(),
                },
                TaintSource {
                    pattern: "env::args".into(),
                    tag: "user_input".into(),
                },
                TaintSource {
                    pattern: "stdin".into(),
                    tag: "user_input".into(),
                },
                TaintSource {
                    pattern: "read_line".into(),
                    tag: "user_input".into(),
                },
            ],
            sinks: vec![
                TaintSink {
                    pattern: "Command::new".into(),
                    arg_index: 0,
                    cwe: "CWE-78".into(),
                    description: "User input in command execution".into(),
                },
                TaintSink {
                    pattern: "Command::arg".into(),
                    arg_index: 0,
                    cwe: "CWE-78".into(),
                    description: "User input in command argument".into(),
                },
            ],
            sanitizers: vec![
                Sanitizer {
                    pattern: "shell_escape".into(),
                },
                Sanitizer {
                    pattern: "try_escape".into(),
                },
            ],
            severity: "error".into(),
        },
        // CWE-22: Path Traversal
        TaintRule {
            id: "rust-path-traversal".into(),
            sources: vec![
                TaintSource {
                    pattern: "env::args".into(),
                    tag: "user_input".into(),
                },
                TaintSource {
                    pattern: "read_line".into(),
                    tag: "user_input".into(),
                },
                TaintSource {
                    pattern: "text".into(),
                    tag: "user_input".into(),
                },
            ],
            sinks: vec![
                TaintSink {
                    pattern: "read_to_string".into(),
                    arg_index: 0,
                    cwe: "CWE-22".into(),
                    description: "Path traversal via file read".into(),
                },
                TaintSink {
                    pattern: "fs::write".into(),
                    arg_index: 0,
                    cwe: "CWE-22".into(),
                    description: "Path traversal via file write".into(),
                },
                TaintSink {
                    pattern: "File::open".into(),
                    arg_index: 0,
                    cwe: "CWE-22".into(),
                    description: "Path traversal via file open".into(),
                },
                TaintSink {
                    pattern: "remove_file".into(),
                    arg_index: 0,
                    cwe: "CWE-22".into(),
                    description: "Path traversal via file deletion".into(),
                },
            ],
            sanitizers: vec![
                Sanitizer {
                    pattern: "canonicalize".into(),
                },
                Sanitizer {
                    pattern: "Path::clean".into(),
                },
            ],
            severity: "error".into(),
        },
        // CWE-502: Deserialization of Untrusted Data
        TaintRule {
            id: "rust-deserialization".into(),
            sources: vec![
                TaintSource {
                    pattern: "reqwest::get".into(),
                    tag: "network".into(),
                },
                TaintSource {
                    pattern: "to_bytes".into(),
                    tag: "network".into(),
                },
                TaintSource {
                    pattern: "read_line".into(),
                    tag: "user_input".into(),
                },
            ],
            sinks: vec![
                TaintSink {
                    pattern: "from_str".into(),
                    arg_index: 0,
                    cwe: "CWE-502".into(),
                    description: "Deserialization of untrusted data".into(),
                },
                TaintSink {
                    pattern: "deserialize".into(),
                    arg_index: 0,
                    cwe: "CWE-502".into(),
                    description: "Deserialization of untrusted data".into(),
                },
            ],
            sanitizers: vec![],
            severity: "warning".into(),
        },
        // CWE-89: SQL Injection (Rust DB crates)
        TaintRule {
            id: "rust-sql-injection".into(),
            sources: vec![
                TaintSource {
                    pattern: "env::var".into(),
                    tag: "env".into(),
                },
                TaintSource {
                    pattern: "read_line".into(),
                    tag: "user_input".into(),
                },
                TaintSource {
                    pattern: "text".into(),
                    tag: "user_input".into(),
                },
            ],
            sinks: vec![
                TaintSink {
                    pattern: "query".into(),
                    arg_index: 0,
                    cwe: "CWE-89".into(),
                    description: "SQL injection via raw query".into(),
                },
                TaintSink {
                    pattern: "execute".into(),
                    arg_index: 0,
                    cwe: "CWE-89".into(),
                    description: "SQL injection via execute".into(),
                },
            ],
            sanitizers: vec![
                Sanitizer {
                    pattern: "query_with".into(),
                },
                Sanitizer {
                    pattern: "bind".into(),
                },
            ],
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
    fn rust_rules_non_empty() {
        let rules = default_rules("rust");
        assert!(!rules.is_empty());
        let ids: Vec<_> = rules.iter().map(|r| r.id.as_str()).collect();
        assert!(ids.contains(&"rust-command-injection"));
        assert!(ids.contains(&"rust-path-traversal"));
        assert!(ids.contains(&"rust-deserialization"));
        assert!(ids.contains(&"rust-sql-injection"));
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

    #[test]
    fn rust_command_injection_has_sanitizers() {
        let rules = default_rules("rust");
        let cmd = rules
            .iter()
            .find(|r| r.id == "rust-command-injection")
            .unwrap();
        assert!(!cmd.sinks.is_empty());
        assert!(!cmd.sanitizers.is_empty());
        assert!(cmd.sinks.iter().any(|s| s.cwe == "CWE-78"));
    }
}
