use tree_sitter::Language;

use super::LangQueries;

pub struct PythonQueries {
    language: Language,
}

impl Default for PythonQueries {
    fn default() -> Self {
        Self {
            language: ox_langs::get_language("python").unwrap().language,
        }
    }
}

impl PythonQueries {
    pub fn new() -> Self { Self::default() }
}

impl LangQueries for PythonQueries {
    fn declarations_query(&self) -> &'static str {
        // Python has no separate declaration — assignment is declaration.
        // We treat first assignment as declaration in the analyzer.
        r#"
        (assignment
            left: (identifier) @name
            right: (_) @value)
        "#
    }

    fn assignments_query(&self) -> &'static str {
        r#"
        (augmented_assignment
            left: (identifier) @name
            right: (_) @value)
        "#
    }

    fn parameters_query(&self) -> &'static str {
        r#"
        (function_definition
            parameters: (parameters
                (identifier) @name))

        (function_definition
            parameters: (parameters
                (typed_parameter (identifier) @name)))

        (function_definition
            parameters: (parameters
                (default_parameter
                    name: (identifier) @name)))

        (function_definition
            parameters: (parameters
                (typed_default_parameter
                    name: (identifier) @name)))
        "#
    }

    fn references_query(&self) -> &'static str {
        "(identifier) @name"
    }

    fn calls_query(&self) -> &'static str {
        r#"
        (call
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

    fn queries() -> PythonQueries {
        PythonQueries {
            language: tree_sitter_python::LANGUAGE.into(),
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
    fn declarations_capture_assignment() {
        let q = queries();
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&q.language).unwrap();

        let src = b"x = 42\ny = \"hello\"";
        let tree = parser.parse(src, None).unwrap();
        let query =
            tree_sitter::Query::new(&q.language, q.declarations_query()).unwrap();
        let matches = collect_captures(&query, tree.root_node(), src);

        assert_eq!(matches.len(), 2, "should match two assignments");
        let name_idx = query.capture_index_for_name("name").unwrap();
        let name_text = matches[0]
            .iter()
            .find(|(idx, _)| *idx == name_idx)
            .map(|(_, t)| t.as_str())
            .unwrap();
        assert_eq!(name_text, "x");
    }

    #[test]
    fn parameters_capture_typed() {
        let q = queries();
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&q.language).unwrap();

        let src = b"def f(x: int, y=\"default\"):\n    pass";
        let tree = parser.parse(src, None).unwrap();
        let query =
            tree_sitter::Query::new(&q.language, q.parameters_query()).unwrap();
        let matches = collect_captures(&query, tree.root_node(), src);

        assert!(
            matches.len() >= 2,
            "should match typed and default params, got {}",
            matches.len()
        );
    }

    #[test]
    fn calls_capture() {
        let q = queries();
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&q.language).unwrap();

        let src = b"print(\"hello\")\nos.path.join(a, b)";
        let tree = parser.parse(src, None).unwrap();
        let query =
            tree_sitter::Query::new(&q.language, q.calls_query()).unwrap();
        let matches = collect_captures(&query, tree.root_node(), src);
        assert_eq!(matches.len(), 2, "should match two calls");
    }
}
