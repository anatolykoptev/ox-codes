//! Generic worklist-based dataflow solver (forward, MOP).
//!
//! BitSet-based iteration using `fixedbitset`. The solver computes
//! IN/OUT sets for each CFG block until a fixed point is reached.

use fixedbitset::FixedBitSet;
use petgraph::graph::NodeIndex;

use crate::cfg::Cfg;

/// A dataflow problem defines the analysis to run.
pub trait DataFlowProblem {
    /// Transfer function: compute OUT from IN for a block.
    fn transfer(&self, block: NodeIndex, in_set: &FixedBitSet) -> FixedBitSet;

    /// Meet operator: combine sets from predecessors (union for may-analysis).
    fn meet(&self, a: &FixedBitSet, b: &FixedBitSet) -> FixedBitSet;

    /// Number of bits (e.g. number of definitions).
    fn universe_size(&self) -> usize;
}

/// Result of a dataflow analysis: IN and OUT sets per block.
pub struct DataFlowResult {
    pub in_sets: Vec<FixedBitSet>,
    pub out_sets: Vec<FixedBitSet>,
}

/// Forward worklist solver (MOP: Meet Over all Paths).
///
/// Iterates in reverse-postorder until no OUT set changes.
pub fn solve_forward(cfg: &Cfg, problem: &dyn DataFlowProblem) -> DataFlowResult {
    let n = cfg.graph.node_count();
    let bits = problem.universe_size();

    let mut out_sets: Vec<FixedBitSet> =
        (0..n).map(|_| FixedBitSet::with_capacity(bits)).collect();
    let mut in_sets: Vec<FixedBitSet> =
        (0..n).map(|_| FixedBitSet::with_capacity(bits)).collect();

    let rpo = cfg.reverse_postorder();

    // Worklist seeded with all blocks in RPO order.
    let mut worklist: Vec<NodeIndex> = rpo;
    let mut in_worklist = FixedBitSet::with_capacity(n);
    for &idx in &worklist {
        in_worklist.set(idx.index(), true);
    }

    while let Some(node) = worklist.pop() {
        in_worklist.set(node.index(), false);

        // IN[b] = meet(OUT[pred]) for all predecessors.
        let preds = cfg.predecessors(node);
        let new_in = if preds.is_empty() {
            FixedBitSet::with_capacity(bits)
        } else {
            let mut result = out_sets[preds[0].index()].clone();
            for &pred in &preds[1..] {
                result = problem.meet(&result, &out_sets[pred.index()]);
            }
            result
        };
        in_sets[node.index()] = new_in;

        // OUT[b] = transfer(b, IN[b]).
        let new_out = problem.transfer(node, &in_sets[node.index()]);
        if new_out != out_sets[node.index()] {
            out_sets[node.index()] = new_out;
            // Add successors to worklist.
            for succ in cfg.successors(node) {
                if !in_worklist[succ.index()] {
                    worklist.push(succ);
                    in_worklist.set(succ.index(), true);
                }
            }
        }
    }

    DataFlowResult { in_sets, out_sets }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cfg::EdgeKind;
    use crate::il::{Const, Expr, Instr, Lval, Name};
    use crate::types::Span;

    fn span() -> Span {
        Span {
            start_byte: 0,
            end_byte: 1,
            start_line: 1,
            end_line: 1,
        }
    }

    /// Trivial problem: identity transfer, union meet.
    struct IdentityProblem {
        bits: usize,
    }

    impl DataFlowProblem for IdentityProblem {
        fn transfer(&self, _block: NodeIndex, in_set: &FixedBitSet) -> FixedBitSet {
            in_set.clone()
        }
        fn meet(&self, a: &FixedBitSet, b: &FixedBitSet) -> FixedBitSet {
            let mut r = a.clone();
            r.union_with(b);
            r
        }
        fn universe_size(&self) -> usize {
            self.bits
        }
    }

    #[test]
    fn empty_cfg_produces_empty_sets() {
        let mut cfg = Cfg::new();
        cfg.add_edge(cfg.entry, cfg.exit, EdgeKind::Fallthrough);

        let problem = IdentityProblem { bits: 4 };
        let result = solve_forward(&cfg, &problem);

        for s in &result.in_sets {
            assert!(s.is_clear());
        }
        for s in &result.out_sets {
            assert!(s.is_clear());
        }
    }

    #[test]
    fn linear_cfg_propagates_gen() {
        // entry -> b1(x=1) -> exit
        // Problem: b1 generates bit 0.
        let mut cfg = Cfg::new();
        let b1 = cfg.add_block(
            vec![Instr::Assign {
                lval: Lval::var(Name::new("x", 1)),
                rval: Expr::Const(Const::Int(1)),
                span: span(),
            }],
            None,
        );
        cfg.add_edge(cfg.entry, b1, EdgeKind::Fallthrough);
        cfg.add_edge(b1, cfg.exit, EdgeKind::Fallthrough);

        struct GenBit0 {
            target: NodeIndex,
        }
        impl DataFlowProblem for GenBit0 {
            fn transfer(&self, block: NodeIndex, in_set: &FixedBitSet) -> FixedBitSet {
                let mut out = in_set.clone();
                if block == self.target {
                    out.set(0, true);
                }
                out
            }
            fn meet(&self, a: &FixedBitSet, b: &FixedBitSet) -> FixedBitSet {
                let mut r = a.clone();
                r.union_with(b);
                r
            }
            fn universe_size(&self) -> usize {
                4
            }
        }

        let problem = GenBit0 { target: b1 };
        let result = solve_forward(&cfg, &problem);

        // IN[b1] should be empty (entry generates nothing).
        assert!(result.in_sets[b1.index()].is_clear());
        // OUT[b1] should have bit 0.
        assert!(result.out_sets[b1.index()][0]);
        // IN[exit] should have bit 0 (from b1's OUT).
        assert!(result.in_sets[cfg.exit.index()][0]);
    }
}
