use tree_sitter::Language;
use super::LangQueries;

pub struct RustQueries {
    language: Language,
}

impl Default for RustQueries {
    fn default() -> Self {
        Self {
            language: ox_langs::get_language("rust").unwrap().language,
        }
    }
}

impl RustQueries {
    pub fn new() -> Self { Self::default() }
}

impl LangQueries for RustQueries {
    fn declarations_query(&self) -> &'static str {
        // let_declaration: let x = 1;
        // let_declaration with mutable: let mut x = 1;
        r#"
        (let_declaration
            pattern: (identifier) @name
            value: (_)? @value)
        (let_declaration
            pattern: (mut_pattern
                (identifier) @name)
            value: (_)? @value)
        "#
    }

    fn assignments_query(&self) -> &'static str {
        r#"
        (assignment_expression
            left: (identifier) @name
            right: (_) @value)
        (compound_assignment_expr
            left: (identifier) @name
            right: (_) @value)
        "#
    }

    fn parameters_query(&self) -> &'static str {
        r#"
        (parameter pattern: (identifier) @name)
        (parameter pattern: (mut_pattern (identifier) @name))
        (self_parameter (self) @name)
        "#
    }

    fn references_query(&self) -> &'static str {
        "(identifier) @name"
    }

    fn calls_query(&self) -> &'static str {
        r#"
        (call_expression
            function: (_) @func
            arguments: (arguments) @args)
        "#
    }

    fn language(&self) -> &Language {
        &self.language
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_util::collect_captures;
    use super::*;

    fn queries() -> RustQueries {
        RustQueries::default()
    }

    #[test]
    fn declarations_query_compiles() {
        let q = queries();
        tree_sitter::Query::new(&q.language, q.declarations_query())
            .expect("declarations query should compile");
    }

    #[test]
    fn assignments_query_compiles() {
        let q = queries();
        tree_sitter::Query::new(&q.language, q.assignments_query())
            .expect("assignments query should compile");
    }

    #[test]
    fn parameters_query_compiles() {
        let q = queries();
        tree_sitter::Query::new(&q.language, q.parameters_query())
            .expect("parameters query should compile");
    }

    #[test]
    fn references_query_compiles() {
        let q = queries();
        tree_sitter::Query::new(&q.language, q.references_query())
            .expect("references query should compile");
    }

    #[test]
    fn calls_query_compiles() {
        let q = queries();
        tree_sitter::Query::new(&q.language, q.calls_query())
            .expect("calls query should compile");
    }

    #[test]
    fn declarations_capture_let() {
        let q = queries();
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&q.language).unwrap();
        let src = b"fn f() { let x = 42; }";
        let tree = parser.parse(src, None).unwrap();
        let query = tree_sitter::Query::new(&q.language, q.declarations_query()).unwrap();
        let matches = collect_captures(&query, tree.root_node(), src);
        assert!(!matches.is_empty(), "should match let_declaration");
        let name_idx = query.capture_index_for_name("name").unwrap();
        let name = matches[0]
            .iter()
            .find(|(i, _)| *i == name_idx)
            .map(|(_, t)| t.as_str())
            .unwrap();
        assert_eq!(name, "x");
    }

    #[test]
    fn declarations_capture_let_mut() {
        let q = queries();
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&q.language).unwrap();
        let src = b"fn f() { let mut y = 0; }";
        let tree = parser.parse(src, None).unwrap();
        let query = tree_sitter::Query::new(&q.language, q.declarations_query()).unwrap();
        let matches = collect_captures(&query, tree.root_node(), src);
        assert!(!matches.is_empty(), "should match let mut declaration");
        let name_idx = query.capture_index_for_name("name").unwrap();
        let name = matches[0]
            .iter()
            .find(|(i, _)| *i == name_idx)
            .map(|(_, t)| t.as_str())
            .unwrap();
        assert_eq!(name, "y");
    }

    #[test]
    fn assignments_capture() {
        let q = queries();
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&q.language).unwrap();
        let src = b"fn f() { let mut x = 0; x = 1; }";
        let tree = parser.parse(src, None).unwrap();
        let query = tree_sitter::Query::new(&q.language, q.assignments_query()).unwrap();
        let matches = collect_captures(&query, tree.root_node(), src);
        assert!(!matches.is_empty(), "should match assignment_expression");
    }

    #[test]
    fn parameters_capture() {
        let q = queries();
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&q.language).unwrap();
        let src = b"fn f(x: i32, y: String) {}";
        let tree = parser.parse(src, None).unwrap();
        let query = tree_sitter::Query::new(&q.language, q.parameters_query()).unwrap();
        let matches = collect_captures(&query, tree.root_node(), src);
        assert_eq!(matches.len(), 2, "should match two parameters");
    }

    #[test]
    fn calls_capture() {
        let q = queries();
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&q.language).unwrap();
        let src = b"fn f() { foo(1); }";
        let tree = parser.parse(src, None).unwrap();
        let query = tree_sitter::Query::new(&q.language, q.calls_query()).unwrap();
        let matches = collect_captures(&query, tree.root_node(), src);
        assert!(!matches.is_empty(), "should match call_expression");
        let func_idx = query.capture_index_for_name("func").unwrap();
        let func_text = matches[0].iter()
            .find(|(i, _)| *i == func_idx)
            .map(|(_, t)| t.as_str())
            .unwrap_or("");
        assert_eq!(func_text, "foo", "expected 'foo' as @func capture");
    }

    #[test]
    fn parameters_capture_mut() {
        let q = queries();
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&q.language).unwrap();
        let src = b"fn f(mut x: i32) {}";
        let tree = parser.parse(src, None).unwrap();
        let query = tree_sitter::Query::new(&q.language, q.parameters_query()).unwrap();
        let matches = collect_captures(&query, tree.root_node(), src);
        assert!(!matches.is_empty(), "should capture mut parameter");
        let name_idx = query.capture_index_for_name("name").unwrap();
        let name = matches[0].iter().find(|(i, _)| *i == name_idx).map(|(_, t)| t.as_str()).unwrap();
        assert_eq!(name, "x");
    }
}
