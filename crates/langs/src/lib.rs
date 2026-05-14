mod bash;
mod c;
mod cpp;
mod csharp;
mod go;
mod java;
mod kotlin;
mod lua;
mod php;
mod python;
mod ruby;
mod rust_lang;
mod svelte;
mod swift;
mod typescript;
mod zig;

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
        "svelte" => Some(svelte::config()),
        "java" => Some(java::config()),
        "c" => Some(c::config()),
        "cpp" | "c++" | "cxx" => Some(cpp::config()),
        "ruby" | "rb" => Some(ruby::config()),
        "csharp" | "c#" | "cs" => Some(csharp::config()),
        "php" => Some(php::config()),
        "bash" | "sh" => Some(bash::config()),
        "lua" => Some(lua::config()),
        "swift" => Some(swift::config()),
        "kotlin" | "kt" => Some(kotlin::config()),
        "zig" => Some(zig::config()),
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
        "svelte" => Some(svelte::scope_query(scope)),
        "java" => Some(java::scope_query(scope)),
        "c" => Some(c::scope_query(scope)),
        "cpp" | "c++" | "cxx" => Some(cpp::scope_query(scope)),
        "ruby" | "rb" => Some(ruby::scope_query(scope)),
        "csharp" | "c#" | "cs" => Some(csharp::scope_query(scope)),
        "php" => Some(php::scope_query(scope)),
        "bash" | "sh" => Some(bash::scope_query(scope)),
        "lua" => Some(lua::scope_query(scope)),
        "swift" => Some(swift::scope_query(scope)),
        "kotlin" | "kt" => Some(kotlin::scope_query(scope)),
        "zig" => Some(zig::scope_query(scope)),
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
        "svelte" => Some("svelte"),
        "java" => Some("java"),
        "c" | "h" => Some("c"),
        "cpp" | "cc" | "cxx" | "hpp" | "hh" => Some("cpp"),
        "rb" => Some("ruby"),
        "cs" => Some("csharp"),
        "php" => Some("php"),
        "sh" | "bash" => Some("bash"),
        "lua" => Some("lua"),
        "swift" => Some("swift"),
        "kt" | "kts" => Some("kotlin"),
        "zig" => Some("zig"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL_LANGS: &[&str] = &[
        "go", "rust", "python", "typescript", "svelte", "java", "c", "cpp", "ruby", "csharp",
        "php", "bash", "lua", "swift", "kotlin", "zig",
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
        assert!(get_language("php").is_some());
        assert!(get_language("bash").is_some());
        assert!(get_language("sh").is_some());
        assert!(get_language("lua").is_some());
        assert!(get_language("swift").is_some());
        assert!(get_language("kotlin").is_some());
        assert!(get_language("kt").is_some());
        assert!(get_language("zig").is_some());
        assert!(get_language("cobol").is_none());
    }

    #[test]
    fn test_get_language_svelte() {
        let cfg = get_language("svelte").unwrap();
        assert_eq!(cfg.extensions, &["svelte"]);
    }

    #[test]
    fn test_get_scope_query_svelte() {
        assert!(get_scope_query("svelte", ScopeKind::Imports).is_some());
        assert!(get_scope_query("svelte", ScopeKind::FunctionBodies).is_some());
    }

    #[test]
    fn test_detect_language_svelte() {
        assert_eq!(detect_language("App.svelte"), Some("svelte"));
        assert_eq!(detect_language("+page.svelte"), Some("svelte"));
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
        assert_eq!(detect_language("index.php"), Some("php"));
        assert_eq!(detect_language("script.sh"), Some("bash"));
        assert_eq!(detect_language("script.bash"), Some("bash"));
        assert_eq!(detect_language("init.lua"), Some("lua"));
        assert_eq!(detect_language("App.swift"), Some("swift"));
        assert_eq!(detect_language("Main.kt"), Some("kotlin"));
        assert_eq!(detect_language("build.kts"), Some("kotlin"));
        assert_eq!(detect_language("main.zig"), Some("zig"));
        assert_eq!(detect_language("README.md"), None);
        assert_eq!(detect_language("Makefile"), None);
    }
}
