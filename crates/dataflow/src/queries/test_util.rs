#![cfg(test)]

use tree_sitter::StreamingIterator;

/// Collect match captures into (capture_index, text) pairs per match.
pub fn collect_captures(
    query: &tree_sitter::Query,
    node: tree_sitter::Node,
    src: &[u8],
) -> Vec<Vec<(u32, String)>> {
    let mut cursor = tree_sitter::QueryCursor::new();
    let mut qmatches = cursor.matches(query, node, src);
    let mut result = Vec::new();
    while let Some(m) = qmatches.next() {
        let caps: Vec<_> = m
            .captures
            .iter()
            .map(|c| {
                let text = std::str::from_utf8(&src[c.node.byte_range()])
                    .unwrap()
                    .to_string();
                (c.index, text)
            })
            .collect();
        result.push(caps);
    }
    result
}
