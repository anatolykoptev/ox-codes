/// Scope kinds for AST-aware search.
#[derive(Debug, Clone, Copy)]
pub enum ScopeKind {
    FunctionBodies,
    Comments,
    Strings,
    TypeDefinitions,
    Imports,
}
