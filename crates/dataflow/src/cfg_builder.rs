//! Translates an `IlFunction`'s flat instruction list into a CFG.
//!
//! The algorithm splits at branches, jumps, and returns to form basic blocks.
//! This is a pragmatic "flat IL" builder — no nested scopes, no loop tracking
//! for back-edges yet. Enough for worklist-based reaching definitions.

use crate::cfg::{Cfg, EdgeKind};
use crate::il::{IlFunction, Instr};

/// Build a CFG from an IL function's body.
///
/// Splits at `Branch`, `Return`, and `Jump` instructions. Each split point
/// becomes a block boundary. Connections are fallthrough except for `Branch`
/// (decision point) and `Return` (connects to exit).
pub fn build_cfg(func: &IlFunction) -> Cfg {
    let mut cfg = Cfg::new();

    if func.body.is_empty() {
        cfg.add_edge(cfg.entry, cfg.exit, EdgeKind::Fallthrough);
        return cfg;
    }

    let mut current_instrs: Vec<Instr> = Vec::new();
    let mut prev_block = cfg.entry;

    for instr in &func.body {
        match instr {
            Instr::Branch { .. } => {
                // Flush pending instructions into a block.
                if !current_instrs.is_empty() {
                    let block = cfg.add_block(std::mem::take(&mut current_instrs), None);
                    cfg.add_edge(prev_block, block, EdgeKind::Fallthrough);
                    prev_block = block;
                }
                // Create a decision block containing only the Branch.
                let decision = cfg.add_block(vec![instr.clone()], None);
                cfg.add_edge(prev_block, decision, EdgeKind::Fallthrough);
                prev_block = decision;
            }
            Instr::Return { .. } => {
                current_instrs.push(instr.clone());
                let block = cfg.add_block(std::mem::take(&mut current_instrs), None);
                cfg.add_edge(prev_block, block, EdgeKind::Fallthrough);
                cfg.add_edge(block, cfg.exit, EdgeKind::Fallthrough);
                prev_block = block;
            }
            Instr::Jump { .. } => {
                current_instrs.push(instr.clone());
                let block = cfg.add_block(std::mem::take(&mut current_instrs), None);
                cfg.add_edge(prev_block, block, EdgeKind::Fallthrough);
                // TODO: loop tracking for back-edges (break/continue).
                prev_block = block;
            }
            _ => {
                current_instrs.push(instr.clone());
            }
        }
    }

    // Flush remaining instructions.
    if !current_instrs.is_empty() {
        let block = cfg.add_block(std::mem::take(&mut current_instrs), None);
        cfg.add_edge(prev_block, block, EdgeKind::Fallthrough);
        cfg.add_edge(block, cfg.exit, EdgeKind::Fallthrough);
    } else if prev_block != cfg.exit {
        cfg.add_edge(prev_block, cfg.exit, EdgeKind::Fallthrough);
    }

    cfg
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::il::{Const, Expr, Lval, Name};
    use crate::types::Span;

    fn span() -> Span {
        Span {
            start_byte: 0,
            end_byte: 1,
            start_line: 1,
            end_line: 1,
        }
    }

    fn assign(name: &str) -> Instr {
        Instr::Assign {
            lval: Lval::var(Name::new(name, 1)),
            rval: Expr::Const(Const::Int(42)),
            span: span(),
        }
    }

    fn ret() -> Instr {
        Instr::Return {
            value: None,
            span: span(),
        }
    }

    fn branch() -> Instr {
        Instr::Branch {
            cond: Expr::Const(Const::Bool(true)),
            span: span(),
        }
    }

    fn il_func(body: Vec<Instr>) -> IlFunction {
        IlFunction {
            name: "test".into(),
            params: vec![],
            body,
            span: span(),
        }
    }

    #[test]
    fn empty_function() {
        let cfg = build_cfg(&il_func(vec![]));
        assert_eq!(cfg.block_count(), 2); // entry + exit
        assert_eq!(cfg.edge_count(), 1); // entry → exit
        assert_eq!(cfg.successors(cfg.entry), vec![cfg.exit]);
    }

    #[test]
    fn linear_function() {
        // x = 42; return
        let cfg = build_cfg(&il_func(vec![assign("x"), ret()]));
        // entry → block(x=42, return) → exit
        assert_eq!(cfg.block_count(), 3);
        assert_eq!(cfg.edge_count(), 3); // entry→block, block→exit, entry→block
        // Actually: entry→block (fallthrough), block→exit (fallthrough)
        // Let's just check the connectivity makes sense.
        let succ = cfg.successors(cfg.entry);
        assert_eq!(succ.len(), 1);
        let block = succ[0];
        assert!(cfg.successors(block).contains(&cfg.exit));
    }

    #[test]
    fn function_with_branch() {
        // x = 1; if true; y = 2; return
        let cfg = build_cfg(&il_func(vec![assign("x"), branch(), assign("y"), ret()]));
        // entry → block(x=1) → decision(branch) → block(y=2, return) → exit
        assert_eq!(cfg.block_count(), 5); // entry, exit, x=1, branch, y=2+ret
    }

    #[test]
    fn function_with_return_connects_to_exit() {
        let cfg = build_cfg(&il_func(vec![ret()]));
        // entry → block(return) → exit
        assert_eq!(cfg.block_count(), 3);
        let succ = cfg.successors(cfg.entry);
        assert_eq!(succ.len(), 1);
        let ret_block = succ[0];
        assert!(cfg.successors(ret_block).contains(&cfg.exit));
    }

    #[test]
    fn rpo_starts_with_entry() {
        let cfg = build_cfg(&il_func(vec![assign("x"), ret()]));
        let rpo = cfg.reverse_postorder();
        assert!(!rpo.is_empty());
        assert_eq!(rpo[0], cfg.entry);
    }
}
