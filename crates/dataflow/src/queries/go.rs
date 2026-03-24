use tree_sitter::Language;

use super::LangQueries;

pub struct GoQueries {
    language: Language,
}

impl Default for GoQueries {
    fn default() -> Self {
        Self {
            language: ox_langs::get_language("go").unwrap().language,
        }
    }
}

impl GoQueries {
    pub fn new() -> Self { Self::default() }
}

impl LangQueries for GoQueries {
    fn declarations_query(&self) -> &'static str {
        // short_var_declaration: `x := expr`
        // var_spec: `var x = expr` or `var x type`
        r#"
        (short_var_declaration
            left: (expression_list (identifier) @name)
            right: (expression_list (_) @value))

        (var_spec
            name: (identifier) @name
            value: (expression_list (_) @value)?)
        "#
    }

    fn assignments_query(&self) -> &'static str {
        r#"
        (assignment_statement
            left: (expression_list (identifier) @name)
            right: (expression_list (_) @value))
        "#
    }

    fn parameters_query(&self) -> &'static str {
        r#"
        (parameter_declaration
            name: (identifier) @name)
        "#
    }

    fn references_query(&self) -> &'static str {
        "(identifier) @name"
    }

    fn calls_query(&self) -> &'static str {
        r#"
        (call_expression
            function: (_) @func
            arguments: (argument_list) @args)
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

    fn queries() -> GoQueries {
        GoQueries {
            language: tree_sitter_go::LANGUAGE.into(),
        }
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
    fn declarations_capture_short_var() {
        let q = queries();
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&q.language).unwrap();

        let src = b"package main\nfunc f() { x := 42 }";
        let tree = parser.parse(src, None).unwrap();
        let query =
            tree_sitter::Query::new(&q.language, q.declarations_query()).unwrap();
        let matches = collect_captures(&query, tree.root_node(), src);

        assert!(!matches.is_empty(), "should match short_var_declaration");
        let name_idx = query.capture_index_for_name("name").unwrap();
        let name_text = matches[0]
            .iter()
            .find(|(idx, _)| *idx == name_idx)
            .map(|(_, t)| t.as_str())
            .expect("should have @name");
        assert_eq!(name_text, "x");
    }

    #[test]
    fn assignments_capture() {
        let q = queries();
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&q.language).unwrap();

        let src = b"package main\nfunc f() { var x int; x = 10 }";
        let tree = parser.parse(src, None).unwrap();
        let query =
            tree_sitter::Query::new(&q.language, q.assignments_query()).unwrap();
        let matches = collect_captures(&query, tree.root_node(), src);
        assert!(!matches.is_empty(), "should match assignment_statement");
    }

    #[test]
    fn parameters_capture() {
        let q = queries();
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&q.language).unwrap();

        let src = b"package main\nfunc f(x int, y string) {}";
        let tree = parser.parse(src, None).unwrap();
        let query =
            tree_sitter::Query::new(&q.language, q.parameters_query()).unwrap();
        let matches = collect_captures(&query, tree.root_node(), src);
        assert_eq!(matches.len(), 2, "should match two parameters");
    }

    #[test]
    fn calls_capture() {
        let q = queries();
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&q.language).unwrap();

        let src = b"package main\nfunc f() { fmt.Println(\"hello\") }";
        let tree = parser.parse(src, None).unwrap();
        let query =
            tree_sitter::Query::new(&q.language, q.calls_query()).unwrap();
        let matches = collect_captures(&query, tree.root_node(), src);

        assert!(!matches.is_empty(), "should match call_expression");
        let func_idx = query.capture_index_for_name("func").unwrap();
        let has_func = matches[0].iter().any(|(idx, _)| *idx == func_idx);
        assert!(has_func, "should have @func capture");
    }
}
