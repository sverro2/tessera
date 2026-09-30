//! Layers: drawings stacked on top of each other, optionally in (nested)
//! groups. Each layer has its own [`Document`], so geometry on different
//! layers has nothing to do with each other and may overlap.
//!
//! The first node is the front; the stack is drawn back to front, i.e. in
//! reverse. A hidden group hides everything in it.

use std::sync::Arc;

use crate::document::{Document, next_revision};

/// Identifies a layer or group; unique within [`Layers`].
pub type NodeId = u64;

#[derive(Debug, Clone)]
pub struct Layer {
    pub id: NodeId,
    pub name: String,
    pub visible: bool,
    /// Whether painted edges blend into their neighbours' colours at their
    /// ends.
    pub crossfade: Crossfade,
    /// Whether its edges are drawn when painting (and exported); hidden,
    /// their paint is kept.
    pub show_edges: bool,
    /// Shared between undo steps until changed.
    pub document: Arc<Document>,
}

/// Whether a layer's painted edges blend into their neighbours'.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Crossfade {
    pub edges: bool,
    /// How far (world units) from its ends an edge fades into the edges
    /// meeting there; in between, it's its own colour.
    pub width: f32,
}

impl Crossfade {
    pub const WIDTH: f32 = 6.0;
}

impl Default for Crossfade {
    fn default() -> Self {
        Crossfade {
            edges: false,
            width: Crossfade::WIDTH,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Group {
    pub id: NodeId,
    pub name: String,
    pub visible: bool,
    /// Whether its contents are listed (in the layers panel).
    pub expanded: bool,
    /// Front first, like the stack.
    pub children: Vec<Node>,
}

#[derive(Debug, Clone)]
pub enum Node {
    Layer(Layer),
    Group(Group),
}

impl Node {
    pub fn id(&self) -> NodeId {
        match self {
            Node::Layer(layer) => layer.id,
            Node::Group(group) => group.id,
        }
    }

    fn visible_mut(&mut self) -> &mut bool {
        match self {
            Node::Layer(layer) => &mut layer.visible,
            Node::Group(group) => &mut group.visible,
        }
    }

    fn name_mut(&mut self) -> &mut String {
        match self {
            Node::Layer(layer) => &mut layer.name,
            Node::Group(group) => &mut group.name,
        }
    }

    /// Whether this is or contains a layer.
    fn has_layer(&self) -> bool {
        match self {
            Node::Layer(_) => true,
            Node::Group(group) => group.children.iter().any(Node::has_layer),
        }
    }
}

/// The layers, front first.
#[derive(Debug, Clone)]
pub struct Layers {
    nodes: Vec<Node>,
    next_id: NodeId,
    /// Changes with every change to the layers themselves (not to what's
    /// drawn on them): see [`Self::signature`].
    revision: u64,
}

impl Default for Layers {
    fn default() -> Self {
        Layers::from_nodes(Vec::new())
    }
}

/// Everything about the layers that saving keeps: compared to tell whether
/// there are unsaved changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signature(Vec<u64>);

/// Where to put a layer or group that's moved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    /// Just in front of (above, in the list) this one, next to it.
    Before(NodeId),
    /// Just behind (below, in the list) this one, next to it.
    After(NodeId),
    /// In this group, at its front.
    Into(NodeId),
    /// At the very back, outside any group.
    Last,
}

/// A line in the layers panel.
#[derive(Debug, Clone)]
pub struct Row<'a> {
    pub id: NodeId,
    /// How deep in groups.
    pub depth: usize,
    pub name: &'a str,
    pub visible: bool,
    /// Visible, and so are the groups it's in.
    pub shown: bool,
    /// For a group, whether it's expanded.
    pub group: Option<bool>,
}

impl Layers {
    /// Layers from these nodes (e.g. read from a file), with ids made
    /// unique; with one empty layer if there's none.
    pub fn from_nodes(mut nodes: Vec<Node>) -> Self {
        fn renumber(nodes: &mut [Node], next: &mut NodeId) {
            for node in nodes {
                *next += 1;
                match node {
                    Node::Layer(layer) => layer.id = *next,
                    Node::Group(group) => {
                        group.id = *next;
                        renumber(&mut group.children, next);
                    }
                }
            }
        }
        let mut next = 0;
        renumber(&mut nodes, &mut next);

        let mut layers = Layers {
            nodes,
            next_id: next + 1,
            revision: next_revision(),
        };
        if !layers.nodes.iter().any(Node::has_layer) {
            let layer = layers.new_layer();
            layers.nodes.insert(0, layer);
        }
        layers
    }

    pub fn nodes(&self) -> &[Node] {
        &self.nodes
    }

    pub fn layer(&self, id: NodeId) -> Option<&Layer> {
        fn find(nodes: &[Node], id: NodeId) -> Option<&Layer> {
            nodes.iter().find_map(|node| match node {
                Node::Layer(layer) => (layer.id == id).then_some(layer),
                Node::Group(group) => find(&group.children, id),
            })
        }
        find(&self.nodes, id)
    }

    pub fn layer_mut(&mut self, id: NodeId) -> Option<&mut Layer> {
        fn find(nodes: &mut [Node], id: NodeId) -> Option<&mut Layer> {
            nodes.iter_mut().find_map(|node| match node {
                Node::Layer(layer) => (layer.id == id).then_some(layer),
                Node::Group(group) => find(&mut group.children, id),
            })
        }
        find(&mut self.nodes, id)
    }

    /// Every layer, front first.
    pub fn layers(&self) -> Vec<&Layer> {
        fn walk<'a>(nodes: &'a [Node], out: &mut Vec<&'a Layer>) {
            for node in nodes {
                match node {
                    Node::Layer(layer) => out.push(layer),
                    Node::Group(group) => walk(&group.children, out),
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.nodes, &mut out);
        out
    }

    /// The frontmost layer.
    pub fn first_layer(&self) -> NodeId {
        self.layers()[0].id
    }

    /// Whether a layer or group with this id exists.
    pub fn contains(&self, id: NodeId) -> bool {
        self.find(id).is_some()
    }

    /// Whether nothing is drawn on any layer.
    pub fn is_empty(&self) -> bool {
        self.layers().iter().all(|layer| layer.document.is_empty())
    }

    pub fn signature(&self) -> Signature {
        let mut signature = vec![self.revision];
        signature.extend(self.layers().iter().map(|layer| layer.document.revision()));
        Signature(signature)
    }

    /// Whether a layer or group is shown: it and the groups it's in are
    /// visible.
    pub fn shown(&self, id: NodeId) -> bool {
        fn walk(nodes: &[Node], id: NodeId) -> Option<bool> {
            nodes.iter().find_map(|node| {
                let (visible, children) = match node {
                    Node::Layer(layer) => (layer.visible, &[][..]),
                    Node::Group(group) => (group.visible, &group.children[..]),
                };
                if node.id() == id {
                    Some(visible)
                } else {
                    walk(children, id).map(|shown| shown && visible)
                }
            })
        }
        walk(&self.nodes, id).unwrap_or(false)
    }

    /// The panel's lines, front first; the contents of collapsed groups
    /// left out.
    pub fn rows(&self) -> Vec<Row<'_>> {
        fn walk<'a>(nodes: &'a [Node], depth: usize, shown: bool, out: &mut Vec<Row<'a>>) {
            for node in nodes {
                match node {
                    Node::Layer(layer) => out.push(Row {
                        id: layer.id,
                        depth,
                        name: &layer.name,
                        visible: layer.visible,
                        shown: shown && layer.visible,
                        group: None,
                    }),
                    Node::Group(group) => {
                        out.push(Row {
                            id: group.id,
                            depth,
                            name: &group.name,
                            visible: group.visible,
                            shown: shown && group.visible,
                            group: Some(group.expanded),
                        });
                        if group.expanded {
                            walk(&group.children, depth + 1, shown && group.visible, out);
                        }
                    }
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.nodes, 0, true, &mut out);
        out
    }

    /// The visible layers behind layer `current` and those in front of it,
    /// each back to front: the order to draw them in.
    pub fn around<'a>(&'a self, current: NodeId) -> (Vec<&'a Layer>, Vec<&'a Layer>) {
        // Back to front, with whether each is shown.
        fn walk<'a>(nodes: &'a [Node], shown: bool, out: &mut Vec<(&'a Layer, bool)>) {
            for node in nodes.iter().rev() {
                match node {
                    Node::Layer(layer) => out.push((layer, shown && layer.visible)),
                    Node::Group(group) => walk(&group.children, shown && group.visible, out),
                }
            }
        }
        let mut stack = Vec::new();
        walk(&self.nodes, true, &mut stack);

        let at = stack
            .iter()
            .position(|(layer, _)| layer.id == current)
            .unwrap_or(stack.len());
        let shown = |part: &[(&'a Layer, bool)]| -> Vec<&'a Layer> {
            part.iter()
                .filter(|(_, shown)| *shown)
                .map(|(layer, _)| *layer)
                .collect()
        };
        (
            shown(&stack[..at]),
            shown(&stack[(at + 1).min(stack.len())..]),
        )
    }

    /// Adds an empty layer in front of `near` (a layer or group), next to
    /// it; returns its id.
    pub fn add_layer(&mut self, near: NodeId) -> NodeId {
        let layer = self.new_layer();
        let id = layer.id();
        match self.siblings_mut(near) {
            Some((siblings, i)) => siblings.insert(i, layer),
            None => self.nodes.insert(0, layer),
        }
        self.changed();
        id
    }

    /// Puts `id` in a new group, in its place; returns the group's id.
    pub fn group(&mut self, id: NodeId) -> Option<NodeId> {
        let group_id = self.next_id;
        let name = format!(
            "Group {}",
            self.count(|node| matches!(node, Node::Group(_))) + 1
        );
        let (siblings, i) = self.siblings_mut(id)?;
        let node = siblings.remove(i);
        siblings.insert(
            i,
            Node::Group(Group {
                id: group_id,
                name,
                visible: true,
                expanded: true,
                children: vec![node],
            }),
        );
        self.next_id += 1;
        self.changed();
        Some(group_id)
    }

    /// Replaces group `id` with its contents.
    pub fn ungroup(&mut self, id: NodeId) -> bool {
        let Some((siblings, i)) = self.siblings_mut(id) else {
            return false;
        };
        let Node::Group(group) = &siblings[i] else {
            return false;
        };
        let children = group.children.clone();
        siblings.splice(i..=i, children);
        self.changed();
        true
    }

    /// Removes a layer or group, with all in it. If that leaves no layer,
    /// an empty one takes its place.
    pub fn remove(&mut self, id: NodeId) -> bool {
        let Some((siblings, i)) = self.siblings_mut(id) else {
            return false;
        };
        siblings.remove(i);
        if !self.nodes.iter().any(Node::has_layer) {
            let layer = self.new_layer();
            self.nodes.insert(0, layer);
        }
        self.changed();
        true
    }

    /// Moves `id` (with all in it) to `place`. Nothing happens if that's
    /// where it is, or inside it.
    pub fn move_to(&mut self, id: NodeId, place: Place) -> bool {
        let target = match place {
            Place::Before(target) | Place::After(target) | Place::Into(target) => Some(target),
            Place::Last => None,
        };
        if let Some(target) = target {
            let inside = self
                .find(id)
                .is_some_and(|node| find_in(std::slice::from_ref(node), target).is_some());
            if inside || !self.contains(target) {
                return false;
            }
            if matches!(place, Place::Into(_)) && !matches!(self.find(target), Some(Node::Group(_)))
            {
                return false;
            }
        }
        let Some((siblings, i)) = self.siblings_mut(id) else {
            return false;
        };
        let node = siblings.remove(i);
        match place {
            Place::Before(target) => {
                let (siblings, i) = self.siblings_mut(target).expect("checked");
                siblings.insert(i, node);
            }
            Place::After(target) => {
                let (siblings, i) = self.siblings_mut(target).expect("checked");
                siblings.insert(i + 1, node);
            }
            Place::Into(target) => {
                let (siblings, i) = self.siblings_mut(target).expect("checked");
                if let Node::Group(group) = &mut siblings[i] {
                    group.children.insert(0, node);
                    group.expanded = true;
                }
            }
            Place::Last => self.nodes.push(node),
        }
        self.changed();
        true
    }

    pub fn rename(&mut self, id: NodeId, name: String) {
        if let Some((siblings, i)) = self.siblings_mut(id) {
            *siblings[i].name_mut() = name;
            self.changed();
        }
    }

    pub fn set_crossfade(&mut self, id: NodeId, crossfade: Crossfade) {
        if let Some(layer) = self.layer_mut(id) {
            layer.crossfade = crossfade;
            self.changed();
        }
    }

    pub fn set_show_edges(&mut self, id: NodeId, show: bool) {
        if let Some(layer) = self.layer_mut(id) {
            layer.show_edges = show;
            self.changed();
        }
    }

    pub fn set_visible(&mut self, id: NodeId, visible: bool) {
        if let Some((siblings, i)) = self.siblings_mut(id) {
            *siblings[i].visible_mut() = visible;
            self.changed();
        }
    }

    /// Shows or hides a group's contents in the panel. Not a change to
    /// the drawing.
    pub fn toggle_expanded(&mut self, id: NodeId) {
        if let Some((siblings, i)) = self.siblings_mut(id)
            && let Node::Group(group) = &mut siblings[i]
        {
            group.expanded = !group.expanded;
        }
    }

    fn new_layer(&mut self) -> Node {
        let id = self.next_id;
        self.next_id += 1;
        Node::Layer(Layer {
            id,
            name: format!(
                "Layer {}",
                self.count(|node| matches!(node, Node::Layer(_))) + 1
            ),
            visible: true,
            crossfade: Crossfade::default(),
            show_edges: true,
            document: Arc::new(Document::default()),
        })
    }

    fn changed(&mut self) {
        self.revision = next_revision();
    }

    fn count(&self, what: impl Fn(&Node) -> bool + Copy) -> usize {
        fn walk(nodes: &[Node], what: impl Fn(&Node) -> bool + Copy) -> usize {
            nodes
                .iter()
                .map(|node| {
                    usize::from(what(node))
                        + match node {
                            Node::Group(group) => walk(&group.children, what),
                            Node::Layer(_) => 0,
                        }
                })
                .sum()
        }
        walk(&self.nodes, what)
    }

    fn find(&self, id: NodeId) -> Option<&Node> {
        find_in(&self.nodes, id)
    }

    /// The list `id` is in, and where in it.
    fn siblings_mut(&mut self, id: NodeId) -> Option<(&mut Vec<Node>, usize)> {
        fn find(nodes: &mut Vec<Node>, id: NodeId) -> Option<(&mut Vec<Node>, usize)> {
            if let Some(i) = nodes.iter().position(|node| node.id() == id) {
                return Some((nodes, i));
            }
            nodes.iter_mut().find_map(|node| match node {
                Node::Group(group) => find(&mut group.children, id),
                Node::Layer(_) => None,
            })
        }
        find(&mut self.nodes, id)
    }
}

/// The node `id` among `nodes` or in them.
fn find_in(nodes: &[Node], id: NodeId) -> Option<&Node> {
    nodes.iter().find_map(|node| {
        if node.id() == id {
            return Some(node);
        }
        match node {
            Node::Group(group) => find_in(&group.children, id),
            Node::Layer(_) => None,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::Edit;
    use iced::Point;

    fn names(layers: &Layers) -> Vec<(usize, String)> {
        layers
            .rows()
            .iter()
            .map(|row| (row.depth, row.name.to_string()))
            .collect()
    }

    fn draw(layers: &mut Layers, id: NodeId) {
        let layer = layers.layer_mut(id).unwrap();
        assert!(Arc::make_mut(&mut layer.document).apply(Edit::AddTriangle {
            corners: [
                Point::new(0.0, 0.0),
                Point::new(10.0, 0.0),
                Point::new(5.0, 10.0),
            ],
            snap: 0.0,
        }));
    }

    #[test]
    fn layers_stack_and_group() {
        let mut layers = Layers::default();
        let one = layers.first_layer();
        let two = layers.add_layer(one);
        let three = layers.add_layer(two);
        assert_eq!(
            names(&layers),
            [(0, "Layer 3"), (0, "Layer 2"), (0, "Layer 1")].map(|(d, n)| (d, n.into()))
        );

        let group = layers.group(two).unwrap();
        assert!(layers.move_to(one, Place::After(two)));
        assert_eq!(
            names(&layers),
            [
                (0, "Layer 3"),
                (0, "Group 1"),
                (1, "Layer 2"),
                (1, "Layer 1")
            ]
            .map(|(d, n)| (d, n.into()))
        );

        // Hidden groups hide what's in them; drawn back to front around
        // the current layer.
        layers.set_visible(group, false);
        let (below, above) = layers.around(three);
        assert_eq!((below.len(), above.len()), (0, 0));
        assert!(!layers.shown(one) && !layers.shown(group) && layers.shown(three));
        layers.set_visible(group, true);
        let (below, above) = layers.around(two);
        assert_eq!((below.len(), above.len()), (1, 1));
        assert_eq!(below[0].id, one);

        assert!(layers.move_to(one, Place::After(group)));
        assert!(layers.ungroup(group));
        assert_eq!(
            names(&layers),
            [(0, "Layer 3"), (0, "Layer 2"), (0, "Layer 1")].map(|(d, n)| (d, n.into()))
        );

        assert!(layers.move_to(one, Place::Before(two)));
        assert_eq!(
            names(&layers),
            [(0, "Layer 3"), (0, "Layer 1"), (0, "Layer 2")].map(|(d, n)| (d, n.into()))
        );

        assert!(layers.remove(two));
        assert!(layers.remove(three));
        assert_eq!(layers.layers().len(), 1);

        // A group holding every layer goes too; an empty layer is left.
        draw(&mut layers, one);
        let group = layers.group(one).unwrap();
        assert!(layers.remove(group));
        let left = layers.layers();
        assert_eq!(left.len(), 1);
        assert!(left[0].document.is_empty());
        assert_ne!(left[0].id, one);
        assert!(!layers.contains(group));
    }

    #[test]
    fn moving_by_dragging() {
        let mut layers = Layers::default();
        let one = layers.first_layer();
        let two = layers.add_layer(one);
        let three = layers.add_layer(two);
        let group = layers.group(two).unwrap();
        // Layer 3, Group 1 [Layer 2], Layer 1.

        assert!(layers.move_to(one, Place::Into(group)));
        assert!(layers.move_to(three, Place::After(two)));
        assert_eq!(
            names(&layers),
            [
                (0, "Group 1"),
                (1, "Layer 1"),
                (1, "Layer 2"),
                (1, "Layer 3")
            ]
            .map(|(d, n)| (d, n.into()))
        );

        // Not into itself, nor into a layer.
        assert!(!layers.move_to(group, Place::Into(group)));
        assert!(!layers.move_to(group, Place::Before(two)));
        assert!(!layers.move_to(one, Place::Into(two)));

        assert!(layers.move_to(two, Place::Last));
        assert!(layers.move_to(three, Place::Before(group)));
        assert_eq!(
            names(&layers),
            [
                (0, "Layer 3"),
                (0, "Group 1"),
                (1, "Layer 1"),
                (0, "Layer 2")
            ]
            .map(|(d, n)| (d, n.into()))
        );
    }

    #[test]
    fn the_signature_follows_changes_and_undo() {
        let mut layers = Layers::default();
        let one = layers.first_layer();
        let saved = layers.signature();
        let before = layers.clone();

        draw(&mut layers, one);
        assert_ne!(layers.signature(), saved);
        // Undo: the copy kept shares the untouched document.
        layers = before.clone();
        assert_eq!(layers.signature(), saved);

        layers.rename(one, "Sketch".into());
        assert_ne!(layers.signature(), saved);
        // Expanding groups isn't saved.
        let group = layers.group(one).unwrap();
        let grouped = layers.signature();
        layers.toggle_expanded(group);
        assert_eq!(layers.signature(), grouped);
    }
}
