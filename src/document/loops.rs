//! Edge loops: from an edge, on along the edges that carry on straightest.

use std::collections::{HashMap, HashSet};

use super::*;

impl Document {
    /// The edge loop through edge `a`–`b`: from each end, on along the
    /// edge that carries on straightest, while it turns by at most
    /// `max_turn` (radians). It ends where none does, or where it comes
    /// back to itself (round to where it started, say), so it never goes
    /// round twice. Its edges in the order walked: from `a`–`b` on past
    /// `b`, then on past `a`. Empty if `a`–`b` is no edge.
    pub fn edge_loop(&self, a: VertexId, b: VertexId, max_turn: f32) -> Vec<(VertexId, VertexId)> {
        let mut around: HashMap<VertexId, Vec<VertexId>> = HashMap::new();
        for (u, v) in self.unique_edges() {
            around.entry(u).or_default().push(v);
            around.entry(v).or_default().push(u);
        }
        if !around.get(&a).is_some_and(|ends| ends.contains(&b)) {
            return Vec::new();
        }
        let turn = |from: VertexId, to: VertexId, on: VertexId| {
            let (d, e) = (
                self.vertex(to) - self.vertex(from),
                self.vertex(on) - self.vertex(to),
            );
            (d.x * e.y - d.y * e.x).atan2(d.x * e.x + d.y * e.y).abs()
        };
        let mut edges = vec![(a, b)];
        let mut passed = HashSet::from([a, b]);
        for (mut from, mut to) in [(a, b), (b, a)] {
            loop {
                let straightest = around[&to]
                    .iter()
                    .filter(|&&on| on != from)
                    .map(|&on| (on, turn(from, to, on)))
                    .filter(|&(_, turn)| turn <= max_turn)
                    .min_by(|x, y| x.1.total_cmp(&y.1));
                let Some((on, _)) = straightest else { break };
                let edge = (to, on);
                if edges.iter().any(|&(u, v)| (u, v) == edge || (v, u) == edge) {
                    break;
                }
                edges.push(edge);
                // Back at itself: closed.
                if !passed.insert(on) {
                    break;
                }
                (from, to) = (to, on);
            }
        }
        edges
    }
}
