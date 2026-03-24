pub mod analysis;
pub mod cfg;
pub mod cfg_analysis;
pub mod cfg_builder;
pub mod def_use;
pub mod il;
mod il_build_expr;
mod il_build_lval;
pub mod il_builder;
#[cfg(test)]
mod il_builder_tests;
pub mod queries;
pub mod reaching_defs;
pub mod scope_walker;
pub mod semantics;
pub mod solver;
pub mod taint;
pub mod taint_rules;
#[cfg(test)]
mod taint_tests;
pub mod types;

pub use types::{
    ConstValue, DataflowInput, DataflowResponse, Finding, FindingKind, Scope, ScopeChain,
    ScopeKind, Severity, Span, TaintTag, VarBinding,
};
