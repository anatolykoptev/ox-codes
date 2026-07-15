//! Def-use chain extraction from reaching definitions.

use petgraph::graph::NodeIndex;

use crate::cfg::Cfg;
use crate::il::{Expr, Instr, Name};
use crate::reaching_defs::{Definition, reaching_definitions};
use crate::types::Span;

/// A use of a variable.
#[derive(Debug, Clone)]
pub struct Use {
    pub name: Name,
    pub span: Span,
    pub block: NodeIndex,
}

/// A def-use chain: one definition and all its reaching uses.
#[derive(Debug, Clone)]
pub struct DefUseChain {
    pub def: Definition,
    pub uses: Vec<Use>,
}

/// Build def-use chains from a CFG.
///
/// Returns `None` if reaching definitions bailed out.
pub fn build_def_use_chains(cfg: &Cfg) -> Option<Vec<DefUseChain>> {
    let (defs, result) = reaching_definitions(cfg)?;

    let mut chains: Vec<DefUseChain> = defs
        .iter()
        .map(|d| DefUseChain {
            def: d.clone(),
            uses: Vec::new(),
        })
        .collect();

    // Walk each block instruction-by-instruction, tracking which defs
    // are live at each point (handles intra-block def-use correctly).
    for idx in cfg.graph.node_indices() {
        let block = &cfg.graph[idx];
        // Start with defs reaching this block's entry.
        let mut live = result.in_sets[idx.index()].clone();

        for instr in &block.instrs {
            // First: collect uses from this instruction.
            let mut instr_uses = Vec::new();
            uses_from_instr(instr, idx, &mut instr_uses);
            for u in &instr_uses {
                for def in &defs {
                    if def.name.ident == u.name.ident && live[def.id] {
                        chains[def.id].uses.push(u.clone());
                    }
                }
            }

            // Then: update live set with any definition from this instruction.
            if let Some((name, ispan)) = def_of_instr(instr) {
                // Kill previous defs of the same variable.
                for def in &defs {
                    if def.name.ident == name.ident {
                        live.set(def.id, false);
                    }
                }
                // Gen the new def (match by block + span).
                for def in &defs {
                    if def.name.ident == name.ident
                        && def.block == idx
                        && def.span.start_byte == ispan.start_byte
                    {
                        live.set(def.id, true);
                    }
                }
            }
        }
    }

    Some(chains)
}

/// Extract the defined variable from an instruction, if any.
fn def_of_instr(instr: &Instr) -> Option<(Name, Span)> {
    match instr {
        Instr::Assign { lval, span, .. } => lval.name().cloned().map(|n| (n, *span)),
        Instr::Call {
            result: Some(lval),
            span,
            ..
        } => lval.name().cloned().map(|n| (n, *span)),
        _ => None,
    }
}

/// Extract variable uses from an instruction.
fn uses_from_instr(instr: &Instr, block: NodeIndex, uses: &mut Vec<Use>) {
    match instr {
        Instr::Assign { rval, span, .. } => uses_from_expr(rval, *span, block, uses),
        Instr::Call {
            func, args, span, ..
        } => {
            uses_from_expr(func, *span, block, uses);
            for arg in args {
                uses_from_expr(arg, *span, block, uses);
            }
        }
        Instr::Return {
            value: Some(expr),
            span,
        } => uses_from_expr(expr, *span, block, uses),
        Instr::Branch { cond, span } => uses_from_expr(cond, *span, block, uses),
        _ => {}
    }
}

/// Extract variable uses from an expression (recursive).
fn uses_from_expr(expr: &Expr, span: Span, block: NodeIndex, uses: &mut Vec<Use>) {
    match expr {
        Expr::Lval(lval) => {
            if let Some(name) = lval.name() {
                uses.push(Use {
                    name: name.clone(),
                    span,
                    block,
                });
            }
        }
        Expr::BinOp { left, right, .. } => {
            uses_from_expr(left, span, block, uses);
            uses_from_expr(right, span, block, uses);
        }
        Expr::UnaryOp { operand, .. } => uses_from_expr(operand, span, block, uses),
        Expr::Call { func, args } => {
            uses_from_expr(func, span, block, uses);
            for arg in args {
                uses_from_expr(arg, span, block, uses);
            }
        }
        Expr::Const(_) | Expr::Fixme(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cfg_builder::build_cfg;
    use crate::il_builder::build_il;

    fn cfg_from_go(src: &[u8]) -> Cfg {
        let il = build_il(src, "go").unwrap();
        assert!(!il.functions.is_empty(), "no functions parsed");
        build_cfg(&il.functions[0])
    }

    #[test]
    fn simple_chain() {
        let cfg = cfg_from_go(
            br#"package main
func foo() { x := 1; y := x + 2; _ = y }"#,
        );
        let chains = build_def_use_chains(&cfg).unwrap();
        let x_chain = chains.iter().find(|c| c.def.name.ident == "x").unwrap();
        assert!(!x_chain.uses.is_empty(), "x should have uses");
    }

    #[test]
    fn redefinition_kills() {
        let cfg = cfg_from_go(
            br#"package main
func foo() { x := 1; x = 2; fmt.Println(x) }"#,
        );
        let chains = build_def_use_chains(&cfg).unwrap();
        let x_chains: Vec<_> = chains.iter().filter(|c| c.def.name.ident == "x").collect();
        assert!(x_chains.len() >= 2, "need at least 2 defs of x");
    }

    #[test]
    fn no_uses_produces_chain() {
        let cfg = cfg_from_go(
            br#"package main
func foo() { x := 1; _ = x }"#,
        );
        let chains = build_def_use_chains(&cfg).unwrap();
        assert!(!chains.is_empty());
    }
}
