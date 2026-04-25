use tree_sitter::Language;
use super::LangQueries;

pub struct TypescriptQueries {
    language: Language,
}

impl Default for TypescriptQueries {
    fn default() -> Self {
        Self {
            language: ox_langs::get_language("typescript").unwrap().language,
        }
    }
}

impl TypescriptQueries {
    pub fn new() -> Self { Self::default() }
}

impl LangQueries for TypescriptQueries {
    fn declarations_query(&self) -> &'static str {
        // lexical_declaration: const x = 1, let x = 1
        // variable_declaration: var x = 1
        r#"
        (lexical_declaration
            (variable_declarator
                name: (identifier) @name
                value: (_)? @value))
        (variable_declaration
            (variable_declarator
                name: (identifier) @name
                value: (_)? @value))
        "#
    }

    fn assignments_query(&self) -> &'static str {
        r#"
        (assignment_expression
            left: (identifier) @name
            right: (_) @value)
        (augmented_assignment_expression
            left: (identifier) @name
            right: (_) @value)
        "#
    }

    fn parameters_query(&self) -> &'static str {
        r#"
        (required_parameter pattern: (identifier) @name)
        (optional_parameter pattern: (identifier) @name)
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

    fn queries() -> TypescriptQueries {
        TypescriptQueries::default()
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
    fn declarations_capture_const() {
        let q = queries();
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&q.language).unwrap();
        let src = b"const x = 42;";
        let tree = parser.parse(src, None).unwrap();
        let query = tree_sitter::Query::new(&q.language, q.declarations_query()).unwrap();
        let matches = collect_captures(&query, tree.root_node(), src);
        assert!(!matches.is_empty(), "should match const declaration");
        let name_idx = query.capture_index_for_name("name").unwrap();
        let name = matches[0]
            .iter()
            .find(|(i, _)| *i == name_idx)
            .map(|(_, t)| t.as_str())
            .unwrap();
        assert_eq!(name, "x");
    }

    #[test]
    fn declarations_capture_let() {
        let q = queries();
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&q.language).unwrap();
        let src = b"let y = \"hello\";";
        let tree = parser.parse(src, None).unwrap();
        let query = tree_sitter::Query::new(&q.language, q.declarations_query()).unwrap();
        let matches = collect_captures(&query, tree.root_node(), src);
        assert!(!matches.is_empty(), "should match let declaration");
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
        let src = b"let x = 1; x = 2;";
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
        let src = b"function f(x, y) { return x + y; }";
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
        let src = b"console.log(\"hello\");";
        let tree = parser.parse(src, None).unwrap();
        let query = tree_sitter::Query::new(&q.language, q.calls_query()).unwrap();
        let matches = collect_captures(&query, tree.root_node(), src);
        assert!(!matches.is_empty(), "should match call_expression");
        let func_idx = query.capture_index_for_name("func").unwrap();
        let has_func = matches[0].iter().any(|(idx, _)| *idx == func_idx);
        assert!(has_func, "should have @func capture");
    }
}
