//! Control Flow Graph representation for data-flow analysis.
//!
//! A `Cfg` is a directed graph of `BasicBlock`s connected by typed edges.
//! Synthetic entry/exit nodes bookend the graph so every path has a uniform
//! start and end — required by the worklist solver.

use petgraph::graph::{DiGraph, NodeIndex};
use petgraph::Direction;
use serde::Serialize;

use crate::il::Instr;
use crate::types::Span;

/// Edge kind in the CFG.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum EdgeKind {
    Fallthrough,
    TrueBranch,
    FalseBranch,
    BackEdge,
}

/// A basic block: a sequence of instructions with no branches except at the end.
#[derive(Debug, Clone, Serialize)]
pub struct BasicBlock {
    pub id: u32,
    pub instrs: Vec<Instr>,
    pub span: Option<Span>,
    pub kind: BlockKind,
}

/// Discriminator for synthetic vs normal blocks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum BlockKind {
    Entry,
    Exit,
    Normal,
}

/// The control flow graph for a single function.
#[derive(Debug)]
pub struct Cfg {
    pub graph: DiGraph<BasicBlock, EdgeKind>,
    pub entry: NodeIndex,
    pub exit: NodeIndex,
}

impl Cfg {
    /// Create a new CFG with synthetic entry and exit nodes.
    pub fn new() -> Self {
        let mut graph = DiGraph::new();
        let entry = graph.add_node(BasicBlock {
            id: 0,
            instrs: vec![],
            span: None,
            kind: BlockKind::Entry,
        });
        let exit = graph.add_node(BasicBlock {
            id: 1,
            instrs: vec![],
            span: None,
            kind: BlockKind::Exit,
        });
        Self { graph, entry, exit }
    }

    /// Add a basic block and return its index.
    pub fn add_block(&mut self, instrs: Vec<Instr>, span: Option<Span>) -> NodeIndex {
        let id = self.graph.node_count() as u32;
        self.graph.add_node(BasicBlock {
            id,
            instrs,
            span,
            kind: BlockKind::Normal,
        })
    }

    /// Add an edge between two blocks.
    pub fn add_edge(&mut self, from: NodeIndex, to: NodeIndex, kind: EdgeKind) {
        self.graph.add_edge(from, to, kind);
    }

    /// Number of basic blocks (including entry/exit).
    pub fn block_count(&self) -> usize {
        self.graph.node_count()
    }

    /// Number of edges.
    pub fn edge_count(&self) -> usize {
        self.graph.edge_count()
    }

    /// Get predecessors of a block.
    pub fn predecessors(&self, node: NodeIndex) -> Vec<NodeIndex> {
        self.graph
            .neighbors_directed(node, Direction::Incoming)
            .collect()
    }

    /// Get successors of a block.
    pub fn successors(&self, node: NodeIndex) -> Vec<NodeIndex> {
        self.graph
            .neighbors_directed(node, Direction::Outgoing)
            .collect()
    }

    /// Iterate blocks in reverse postorder (for worklist solver).
    pub fn reverse_postorder(&self) -> Vec<NodeIndex> {
        use petgraph::visit::{DfsPostOrder, Walker};

        let mut rpo: Vec<_> = DfsPostOrder::new(&self.graph, self.entry)
            .iter(&self.graph)
            .collect();
        rpo.reverse();
        rpo
    }
}

impl Default for Cfg {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::il::{Expr, Name};
    use crate::types::Span;

    fn span() -> Span {
        Span {
            start_byte: 0,
            end_byte: 1,
            start_line: 1,
            end_line: 1,
        }
    }

    fn ret_instr() -> Instr {
        Instr::Return {
            value: None,
            span: span(),
        }
    }

    fn assign_instr(name: &str) -> Instr {
        use crate::il::Lval;
        Instr::Assign {
            lval: Lval::var(Name::new(name, 1)),
            rval: Expr::Const(crate::il::Const::Int(0)),
            span: span(),
        }
    }

    #[test]
    fn new_cfg_has_entry_and_exit() {
        let cfg = Cfg::new();
        assert_eq!(cfg.block_count(), 2);
        assert_eq!(cfg.edge_count(), 0);
        assert_eq!(cfg.graph[cfg.entry].kind, BlockKind::Entry);
        assert_eq!(cfg.graph[cfg.exit].kind, BlockKind::Exit);
    }

    #[test]
    fn add_blocks_and_edges() {
        let mut cfg = Cfg::new();
        let b1 = cfg.add_block(vec![assign_instr("x")], Some(span()));
        let b2 = cfg.add_block(vec![ret_instr()], Some(span()));

        cfg.add_edge(cfg.entry, b1, EdgeKind::Fallthrough);
        cfg.add_edge(b1, b2, EdgeKind::Fallthrough);
        cfg.add_edge(b2, cfg.exit, EdgeKind::Fallthrough);

        assert_eq!(cfg.block_count(), 4);
        assert_eq!(cfg.edge_count(), 3);

        assert_eq!(cfg.predecessors(b1), vec![cfg.entry]);
        assert_eq!(cfg.successors(b1), vec![b2]);
        assert_eq!(cfg.predecessors(b2), vec![b1]);
        assert_eq!(cfg.successors(b2), vec![cfg.exit]);
    }

    #[test]
    fn reverse_postorder_entry_first() {
        let mut cfg = Cfg::new();
        let b = cfg.add_block(vec![ret_instr()], None);
        cfg.add_edge(cfg.entry, b, EdgeKind::Fallthrough);
        cfg.add_edge(b, cfg.exit, EdgeKind::Fallthrough);

        let rpo = cfg.reverse_postorder();
        assert_eq!(rpo[0], cfg.entry);
        assert_eq!(*rpo.last().unwrap(), cfg.exit);
    }
}
