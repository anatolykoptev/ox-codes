mod c;
mod cpp;
mod csharp;
mod go;
mod java;
mod python;
mod ruby;
mod rust_lang;
mod typescript;

use tree_sitter::Language;

/// Scope kinds for AST-aware search.
#[derive(Debug, Clone, Copy)]
pub enum ScopeKind {
    FunctionBodies,
    Comments,
    Strings,
    TypeDefinitions,
    Imports,
}

/// Language configuration: tree-sitter Language + file extensions.
pub struct LangConfig {
    pub language: Language,
    pub extensions: &'static [&'static str],
}

/// Get tree-sitter Language + config by name.
pub fn get_language(name: &str) -> Option<LangConfig> {
    match name {
        "go" | "golang" => Some(go::config()),
        "rust" | "rs" => Some(rust_lang::config()),
        "python" | "py" => Some(python::config()),
        "typescript" | "ts" | "javascript" | "js" => Some(typescript::config()),
        "java" => Some(java::config()),
        "c" => Some(c::config()),
        "cpp" | "c++" | "cxx" => Some(cpp::config()),
        "ruby" | "rb" => Some(ruby::config()),
        "csharp" | "c#" | "cs" => Some(csharp::config()),
        _ => None,
    }
}

/// Get tree-sitter query string for a scope kind in a language.
pub fn get_scope_query(name: &str, scope: ScopeKind) -> Option<&'static str> {
    match name {
        "go" | "golang" => Some(go::scope_query(scope)),
        "rust" | "rs" => Some(rust_lang::scope_query(scope)),
        "python" | "py" => Some(python::scope_query(scope)),
        "typescript" | "ts" | "javascript" | "js" => Some(typescript::scope_query(scope)),
        "java" => Some(java::scope_query(scope)),
        "c" => Some(c::scope_query(scope)),
        "cpp" | "c++" | "cxx" => Some(cpp::scope_query(scope)),
        "ruby" | "rb" => Some(ruby::scope_query(scope)),
        "csharp" | "c#" | "cs" => Some(csharp::scope_query(scope)),
        _ => None,
    }
}

/// Detect language from file extension.
pub fn detect_language(path: &str) -> Option<&'static str> {
    let ext = path.rsplit('.').next()?;
    match ext {
        "go" => Some("go"),
        "rs" => Some("rust"),
        "py" => Some("python"),
        "ts" | "tsx" | "js" | "jsx" => Some("typescript"),
        "java" => Some("java"),
        "c" | "h" => Some("c"),
        "cpp" | "cc" | "cxx" | "hpp" | "hh" => Some("cpp"),
        "rb" => Some("ruby"),
        "cs" => Some("csharp"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL_LANGS: &[&str] = &[
        "go", "rust", "python", "typescript", "java", "c", "cpp", "ruby", "csharp",
    ];

    const ALL_SCOPES: &[ScopeKind] = &[
        ScopeKind::FunctionBodies,
        ScopeKind::Comments,
        ScopeKind::Strings,
        ScopeKind::TypeDefinitions,
        ScopeKind::Imports,
    ];

    #[test]
    fn test_get_language_go() {
        let cfg = get_language("go").unwrap();
        assert_eq!(cfg.extensions, &["go"]);
    }

    #[test]
    fn test_get_language_aliases() {
        assert!(get_language("golang").is_some());
        assert!(get_language("rs").is_some());
        assert!(get_language("py").is_some());
        assert!(get_language("ts").is_some());
        assert!(get_language("js").is_some());
        assert!(get_language("java").is_some());
        assert!(get_language("c").is_some());
        assert!(get_language("cpp").is_some());
        assert!(get_language("c++").is_some());
        assert!(get_language("ruby").is_some());
        assert!(get_language("rb").is_some());
        assert!(get_language("csharp").is_some());
        assert!(get_language("cs").is_some());
        assert!(get_language("cobol").is_none());
    }

    #[test]
    fn test_scope_query_compiles() {
        for lang_name in ALL_LANGS {
            let cfg = get_language(lang_name).unwrap();
            for scope in ALL_SCOPES {
                let query_str = get_scope_query(lang_name, *scope).unwrap();
                tree_sitter::Query::new(&cfg.language, query_str)
                    .unwrap_or_else(|e| panic!("{lang_name}/{scope:?}: {e}"));
            }
        }
    }

    #[test]
    fn test_detect_language() {
        assert_eq!(detect_language("main.go"), Some("go"));
        assert_eq!(detect_language("lib.rs"), Some("rust"));
        assert_eq!(detect_language("app.py"), Some("python"));
        assert_eq!(detect_language("app.tsx"), Some("typescript"));
        assert_eq!(detect_language("app.ts"), Some("typescript"));
        assert_eq!(detect_language("app.js"), Some("typescript"));
        assert_eq!(detect_language("Main.java"), Some("java"));
        assert_eq!(detect_language("main.c"), Some("c"));
        assert_eq!(detect_language("main.h"), Some("c"));
        assert_eq!(detect_language("main.cpp"), Some("cpp"));
        assert_eq!(detect_language("main.cc"), Some("cpp"));
        assert_eq!(detect_language("main.hpp"), Some("cpp"));
        assert_eq!(detect_language("app.rb"), Some("ruby"));
        assert_eq!(detect_language("Program.cs"), Some("csharp"));
        assert_eq!(detect_language("README.md"), None);
        assert_eq!(detect_language("Makefile"), None);
    }
}
