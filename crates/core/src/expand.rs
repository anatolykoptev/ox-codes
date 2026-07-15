use tree_sitter::{Language, Parser};

use crate::types::{ExpandMode, ExpandedBlock};

/// Node kinds that represent "function-like" symbols per language.
const FUNCTION_KINDS: &[&str] = &[
    "function_declaration", // Go, JS
    "method_declaration",   // Go, Java
    "function_definition",  // Python, C, C++, Rust
    "function_item",        // Rust
    "method_definition",    // Python, Ruby, JS class methods
    "arrow_function",       // JS/TS
    "closure_expression",   // Rust, Swift
    "func_literal",         // Go
];

/// Node kinds that represent "block-level" symbols (superset of functions).
const BLOCK_KINDS: &[&str] = &[
    // Functions (all from above)
    "function_declaration",
    "method_declaration",
    "function_definition",
    "function_item",
    "method_definition",
    "arrow_function",
    "closure_expression",
    "func_literal",
    // Types / classes
    "type_declaration",      // Go
    "struct_item",           // Rust
    "impl_item",             // Rust
    "enum_item",             // Rust
    "trait_item",            // Rust
    "class_declaration",     // Java, JS, TS
    "class_definition",      // Python
    "interface_declaration", // Java, TS
    "struct_specifier",      // C/C++
    "class_specifier",       // C++
    "module",                // Ruby
    "namespace_declaration", // C++, C#
];

pub fn find_enclosing_symbol(
    source: &[u8],
    language: &Language,
    byte_offset: usize,
    mode: &ExpandMode,
) -> Option<ExpandedBlock> {
    if matches!(mode, ExpandMode::None) {
        return None;
    }

    let mut parser = Parser::new();
    parser.set_language(language).ok()?;
    let tree = parser.parse(source, None)?;

    let mut node = tree
        .root_node()
        .descendant_for_byte_range(byte_offset, byte_offset)?;

    let target_kinds: &[&str] = match mode {
        ExpandMode::Function => FUNCTION_KINDS,
        ExpandMode::Block => BLOCK_KINDS,
        ExpandMode::None => return None,
    };

    loop {
        if target_kinds.contains(&node.kind()) {
            let text = &source[node.byte_range()];
            return Some(ExpandedBlock {
                symbol_name: extract_symbol_name(&node, source),
                symbol_kind: node.kind().to_string(),
                line_start: node.start_position().row + 1,
                line_end: node.end_position().row + 1,
                body: String::from_utf8_lossy(text).into_owned(),
            });
        }
        node = node.parent()?;
    }
}

/// Extract the name of a symbol node (function, type, class, etc.).
fn extract_symbol_name(node: &tree_sitter::Node, source: &[u8]) -> String {
    for field in &["name", "type"] {
        if let Some(name_node) = node.child_by_field_name(field) {
            let text = &source[name_node.byte_range()];
            return String::from_utf8_lossy(text).into_owned();
        }
    }
    if node.child_count() >= 2
        && let Some(child) = node.child(1)
    {
        let kind = child.kind();
        if kind.contains("identifier") || kind.contains("name") {
            let text = &source[child.byte_range()];
            return String::from_utf8_lossy(text).into_owned();
        }
    }
    "<anonymous>".to_string()
}

/// Wrap a body string in fenced code block if format is Markdown.
pub fn wrap_body(body: String, format: crate::types::Format, lang: Option<&str>) -> String {
    match format {
        crate::types::Format::Markdown => {
            let lang_str = lang.unwrap_or("text");
            format!("```{lang_str}\n{body}\n```")
        }
        crate::types::Format::Plain => body,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn go_lang() -> Language {
        ox_langs::get_language("go").unwrap().language
    }

    #[test]
    fn test_expand_function_go() {
        let src = b"package main\n\nfunc foo() {\n    x := 1\n    y := 2\n}\n";
        let offset = src.windows(5).position(|w| w == b"x := ").unwrap();
        let lang = go_lang();
        let block = find_enclosing_symbol(src, &lang, offset, &ExpandMode::Function).unwrap();
        assert_eq!(block.symbol_name, "foo");
        assert_eq!(block.symbol_kind, "function_declaration");
        assert!(block.body.contains("x := 1"));
        assert!(block.body.contains("y := 2"));
    }

    #[test]
    fn test_expand_block_type_go() {
        let src = b"package main\n\ntype Config struct {\n    Name string\n    Port int\n}\n";
        let offset = src.windows(4).position(|w| w == b"Name").unwrap();
        let lang = go_lang();
        let block = find_enclosing_symbol(src, &lang, offset, &ExpandMode::Block).unwrap();
        assert_eq!(block.symbol_kind, "type_declaration");
        assert!(block.body.contains("Name string"));
    }

    #[test]
    fn test_expand_none_returns_none() {
        let src = b"package main\n\nfunc foo() {}\n";
        let lang = go_lang();
        let result = find_enclosing_symbol(src, &lang, 20, &ExpandMode::None);
        assert!(result.is_none());
    }

    #[test]
    fn test_expand_top_level_returns_none() {
        let src = b"package main\n\nvar x = 1\n";
        let lang = go_lang();
        let result = find_enclosing_symbol(src, &lang, 0, &ExpandMode::Function);
        assert!(result.is_none());
    }

    #[test]
    fn test_extract_symbol_name_go_func() {
        let src = b"package main\n\nfunc myHandler() {}\n";
        let lang = go_lang();
        let mut parser = Parser::new();
        parser.set_language(&lang).unwrap();
        let tree = parser.parse(src, None).unwrap();
        let root = tree.root_node();
        let func = (0..root.child_count())
            .filter_map(|i| root.child(i as u32))
            .find(|n| n.kind() == "function_declaration")
            .unwrap();
        assert_eq!(extract_symbol_name(&func, src), "myHandler");
    }
}
