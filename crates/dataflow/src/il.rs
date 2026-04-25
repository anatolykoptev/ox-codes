//! Three-address intermediate language for data-flow analysis.
//!
//! Language-agnostic IL inspired by Semgrep's approach. Every language's
//! tree-sitter AST gets translated to this representation, so all analyses
//! (CFG, reaching defs, taint) work on one IR.

use serde::Serialize;
use smallvec::SmallVec;

use crate::types::{Sid, Span};

/// Variable name with unique ID (resolves shadowing via scope-unique numbering).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct Name {
    pub ident: String,
    pub sid: Sid,
}

/// Base of an l-value (what's being assigned to).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub enum Base {
    Var(Name),
    /// Unknown/unsupported base (Semgrep Fixme pattern).
    Unknown(String),
}

/// Field/index offset for field-sensitive tracking.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub enum Offset {
    /// `.field_name`
    Field(String),
    /// `[0]`, `[1]`
    Index(i64),
    /// `[expr]` — unknown index
    DynIndex,
}

/// Left-hand side value (assignable location).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct Lval {
    pub base: Base,
    pub offsets: SmallVec<[Offset; 2]>,
}

/// Constant value.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum Const {
    Int(i64),
    Float(f64),
    Str(String),
    Bool(bool),
    Nil,
}

/// Expression (right-hand side).
#[derive(Debug, Clone, Serialize)]
pub enum Expr {
    Const(Const),
    Lval(Lval),
    BinOp {
        op: String,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    UnaryOp {
        op: String,
        operand: Box<Expr>,
    },
    Call {
        func: Box<Expr>,
        args: Vec<Expr>,
    },
    /// Unknown/unsupported expression (error recovery).
    Fixme(String),
}

/// Jump target kind.
#[derive(Debug, Clone, Serialize)]
pub enum JumpTarget {
    Break,
    Continue,
    Goto(String),
    Fallthrough,
}

/// IL instruction.
#[derive(Debug, Clone, Serialize)]
pub enum Instr {
    /// `x = expr`
    Assign { lval: Lval, rval: Expr, span: Span },
    /// `[result =] func(args)`
    Call {
        result: Option<Lval>,
        func: Expr,
        args: Vec<Expr>,
        span: Span,
    },
    /// `return [expr]`
    Return { value: Option<Expr>, span: Span },
    /// `if cond` (two CFG successors: true/false branch)
    Branch { cond: Expr, span: Span },
    /// Unconditional jump (break, continue, goto)
    Jump { target: JumpTarget, span: Span },
    /// Unsupported construct — skip, don't crash.
    Fixme { reason: String, span: Span },
}

/// A function's IL representation.
#[derive(Debug, Clone, Serialize)]
pub struct IlFunction {
    pub name: String,
    pub params: Vec<Name>,
    pub body: Vec<Instr>,
    pub span: Span,
}

/// IL for an entire file.
#[derive(Debug, Clone, Serialize)]
pub struct IlFile {
    pub functions: Vec<IlFunction>,
}

impl Lval {
    pub fn var(name: Name) -> Self {
        Self {
            base: Base::Var(name),
            offsets: SmallVec::new(),
        }
    }

    pub fn name(&self) -> Option<&Name> {
        match &self.base {
            Base::Var(n) => Some(n),
            Base::Unknown(_) => None,
        }
    }
}

impl Name {
    pub fn new(ident: impl Into<String>, sid: Sid) -> Self {
        Self {
            ident: ident.into(),
            sid,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lval_var_creates_correct_lval() {
        let name = Name::new("x", 1);
        let lval = Lval::var(name.clone());
        assert_eq!(lval.name(), Some(&name));
        assert!(lval.offsets.is_empty());
    }

    #[test]
    fn name_new_works() {
        let n = Name::new("foo", 42);
        assert_eq!(n.ident, "foo");
        assert_eq!(n.sid, 42);
    }

    #[test]
    fn lval_unknown_has_no_name() {
        let lval = Lval {
            base: Base::Unknown("complex".into()),
            offsets: SmallVec::new(),
        };
        assert_eq!(lval.name(), None);
    }
}
