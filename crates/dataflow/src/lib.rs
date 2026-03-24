pub mod analysis;
pub mod il;
mod il_build_expr;
mod il_build_lval;
pub mod il_builder;
#[cfg(test)]
mod il_builder_tests;
pub mod queries;
pub mod scope_walker;
pub mod types;

pub use types::{
    ConstValue, DataflowInput, DataflowResponse, Finding, FindingKind, Scope, ScopeChain,
    ScopeKind, Severity, Span, TaintTag, VarBinding,
};
