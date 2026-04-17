// TACO compressed dependency graph
//
// stores formula dependencies as a compressed directed graph where vertices are cell ranges
// and edges encode one of four compression patterns (RR, RF, FR, FF) from the TACO paper.
// vertices are spatially indexed with an R-tree for fast overlap queries.
//
// key operations:
// - insert/remove formula cell: updates the compressed graph, merging edges when possible
// - init_pending_counter: BFS from changed range to discover affected dependants (paper Algorithm 3)
// - decrease_pending_counter: decrements dependants after evaluation, returns newly-ready cells
//
// see: "Efficient and Compact Spreadsheet Formula Graphs" (2023)

use std::collections::{HashSet, VecDeque};
use std::ops::ControlFlow;

use petgraph::{
    dot::{Config, Dot, RankDir},
    stable_graph::{EdgeIndex, NodeIndex, StableDiGraph},
    visit::{EdgeRef, NodeRef},
    Direction,
};
use rstar::{RTree, RTreeObject, AABB};

use crate::storage::{
    grid::{CellValue, Grid, GridCellId},
    types::{AbsoluteCellId, CellRange, Expr, ExprAtom},
};

// --- types ---

// dependant ranges are always line ranges (single row or column vector).
// Axis tracks which direction a dependant range extends along.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Axis {
    Row,
    Col,
}

// the four TACO compression patterns + Single for uncompressed edges.
//
// RR (Relative-Relative): sliding window. both head and tail of the referenced range
//   shift relative to the formula cell. e.g. SUM(A1:B3), SUM(A2:B4), ...
// RF (Relative-Fixed): shrinking window. head shifts, tail stays fixed.
// FR (Fixed-Relative): expanding window. head stays fixed, tail shifts.
// FF (Fixed-Fixed): fixed window. both head and tail are absolute.
// Single: uncompressed edge (one formula cell, one dependency range).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EdgePattern {
    Single,
    RR,
    RF,
    FR,
    FF,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Offset {
    row: i32,
    col: i32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct CellPoint {
    row: u32,
    col: u32,
}

// metadata for a compressed edge.
// which fields are Some depends on the pattern:
//   RR: head_relative_offset + tail_relative_offset
//   RF: head_relative_offset + tail_fixed_point
//   FR: head_fixed_point + tail_relative_offset
//   FF: head_fixed_point + tail_fixed_point
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct EdgeMetadata {
    head_relative_offset: Option<Offset>,
    tail_relative_offset: Option<Offset>,
    head_fixed_point: Option<CellPoint>,
    tail_fixed_point: Option<CellPoint>,
}

#[derive(Debug, Clone, Copy)]
struct CompressedEdgeData {
    pattern: EdgePattern,
    metadata: EdgeMetadata,
}

#[derive(Debug, Clone, Copy)]
struct PendingEdgeInsert {
    dependency_range: CellRange,
    dependant_range: CellRange,
    pattern: EdgePattern,
    metadata: EdgeMetadata,
}

// --- R-tree wrappers ---

// wraps a graph NodeIndex with a spatial envelope for the R-tree on vertices
#[derive(Clone, Copy)]
struct IndexedVertex {
    vertex_index: NodeIndex,
    envelope: AABB<[i64; 3]>,
}

impl IndexedVertex {
    fn new(vertex_index: NodeIndex, range: CellRange) -> Self {
        Self {
            vertex_index,
            envelope: vertex_envelope(range),
        }
    }
}

impl PartialEq for IndexedVertex {
    fn eq(&self, other: &Self) -> bool {
        self.vertex_index == other.vertex_index
    }
}

impl Eq for IndexedVertex {}

impl RTreeObject for IndexedVertex {
    type Envelope = AABB<[i64; 3]>;

    fn envelope(&self) -> Self::Envelope {
        self.envelope
    }
}

// wraps a CellRange with a spatial envelope for the "visited" R-tree in BFS dedup
#[derive(Clone, Copy)]
struct VisitedRange {
    range: CellRange,
    envelope: AABB<[i64; 3]>,
}

impl VisitedRange {
    fn new(range: CellRange) -> Self {
        Self {
            range,
            envelope: vertex_envelope(range),
        }
    }
}

impl PartialEq for VisitedRange {
    fn eq(&self, other: &Self) -> bool {
        self.range == other.range
    }
}

impl Eq for VisitedRange {}

impl RTreeObject for VisitedRange {
    type Envelope = AABB<[i64; 3]>;

    fn envelope(&self) -> Self::Envelope {
        self.envelope
    }
}

// the TACO compressed dependency graph
//
// vertices are cell ranges, edges encode compression patterns (RR, RF, FR, FF, Single).
// the R-tree indexes vertices for fast spatial lookups (finding which vertices overlap a query range).
pub struct DependencyGraph {
    graph: StableDiGraph<CellRange, CompressedEdgeData>,
    vertex_index: RTree<IndexedVertex>,
}

impl Default for DependencyGraph {
    fn default() -> Self {
        Self {
            graph: StableDiGraph::new(),
            vertex_index: RTree::new(),
        }
    }
}

impl DependencyGraph {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn graph_size(&self) -> (usize, usize) {
        (self.graph.node_count(), self.graph.edge_count())
    }

    pub fn clear(&mut self) {
        self.graph = StableDiGraph::new();
        self.vertex_index = RTree::new();
    }

    pub fn insert_formula_cell(&mut self, formula_cell: AbsoluteCellId, ast: &[Expr]) {
        let dependant_range = CellRange::single(formula_cell);
        let mut dependency_ranges = dependency_ranges_from_ast(ast, &formula_cell);
        dependency_ranges.sort_unstable();
        dependency_ranges.dedup();

        for dependency_range in dependency_ranges {
            self.insert_dependency_range(dependency_range, dependant_range);
        }
    }

    /// Replace a formula cell's dependencies. Skips the remove+insert if deps are unchanged,
    /// avoiding edge fragmentation when only the formula body changed.
    pub fn update_formula_cell(
        &mut self,
        formula_cell: AbsoluteCellId,
        old_ast: &[Expr],
        new_ast: &[Expr],
    ) {
        let mut old_deps = dependency_ranges_from_ast(old_ast, &formula_cell);
        let mut new_deps = dependency_ranges_from_ast(new_ast, &formula_cell);
        old_deps.sort_unstable();
        old_deps.dedup();
        new_deps.sort_unstable();
        new_deps.dedup();
        if old_deps == new_deps {
            return;
        }
        self.remove_formula_cell(formula_cell);
        self.insert_formula_cell(formula_cell, new_ast);
    }

    // remove a formula cell from the dependency graph (paper Sec. IV-C, removeDep)
    //
    // finds all edges whose dependant range overlaps with the removed cell,
    // then for each edge: subtracts the removed cell from the dependant range
    // and re-inserts the remaining sub-ranges with their corresponding
    // dependency ranges computed via findPrec.
    pub fn remove_formula_cell(&mut self, formula_cell: AbsoluteCellId) {
        let removed = CellRange::single(formula_cell);

        // collect edges whose dependant vertex contains the removed cell
        // (can't modify graph while iterating, so collect first)
        let mut edges_to_update = Vec::new();
        let envelope = vertex_envelope(removed);
        for iv in self.vertex_index.locate_in_envelope_intersecting(&envelope) {
            for edge in self
                .graph
                .edges_directed(iv.vertex_index, Direction::Incoming)
            {
                edges_to_update.push(edge.id());
            }
        }

        // for each affected edge: subtract the removed cell, re-insert remaining sub-ranges
        for edge_index in edges_to_update {
            let replacements = self.split_edge_after_removing_dependant(edge_index, removed);
            self.remove_edge(edge_index);
            for replacement in replacements {
                self.insert_edge(replacement);
            }
        }
    }

    // --- recalculation: pending counter management ---

    // discover all transitive dependants of the changed ranges and set their pending counters
    //
    // follows TACO paper Algorithm 3 (Sec. IV-B):
    // BFS from all changed ranges simultaneously, using an R-tree to dedup visited ranges.
    //
    // two concerns handled separately:
    // - discovery: range-level BFS finds which cells are transitively affected
    // - counters: cell-level increments match the cell-level decrements in the eval loop
    //
    // the cell-level pass is needed because for non-RR edges (Single, FF, RF, FR),
    // findDep maps multiple dependency cells to the same dependant cells.
    // the eval loop decrements per cell, so init must increment per cell to balance.
    //
    // counter == 0 means ready (no unprocessed predecessors).
    pub fn init_pending_counter_for_new_recalculation(
        &self,
        sheets: &[Grid],
        changed_ranges: &[CellRange],
    ) {
        let mut queue = VecDeque::new();
        let mut visited: RTree<VisitedRange> = RTree::new();
        let mut ranges_buf = Vec::new();

        for &changed_range in changed_ranges {
            queue.push_back(changed_range);
            visited.insert(VisitedRange::new(changed_range));
        }

        while let Some(prec_to_visit) = queue.pop_front() {
            let envelope = vertex_envelope(prec_to_visit);

            // increment counters cell-by-cell so they match the cell-level decrease in the eval loop.
            // one R-tree query per BFS step, then iterate cells within each edge.
            for iv in self.vertex_index.locate_in_envelope_intersecting(&envelope) {
                let Some(&vertex_range) = self.graph.node_weight(iv.vertex_index) else {
                    continue;
                };
                let Some(overlap) = vertex_range.intersection(&prec_to_visit) else {
                    continue;
                };

                for edge in self
                    .graph
                    .edges_directed(iv.vertex_index, Direction::Outgoing)
                {
                    let edge_id = edge.id();
                    overlap.for_each_cell(|cell| {
                        if let Some(dep_range) =
                            find_dependant_range(&self.graph, edge_id, CellRange::single(cell))
                        {
                            dep_range.for_each_cell(|dep_cell| {
                                increase_pending_counter(sheets, dep_cell);
                            });
                        }
                    });
                }
            }

            // discover new dependant ranges at range level and enqueue unvisited portions
            self.collect_direct_dependant_ranges(prec_to_visit, &mut ranges_buf);
            for i in 0..ranges_buf.len() {
                for new_range in subtract_visited(&ranges_buf[i], &visited) {
                    visited.insert(VisitedRange::new(new_range));
                    queue.push_back(new_range);
                }
            }
        }
    }

    // detect cells stuck in cycles after evaluation.
    //
    // BFS from changed_ranges (same traversal as init_pending_counter), but instead of
    // incrementing counters, checks which cells still have pending_dependencies > 0.
    // those cells were never evaluated because their predecessors in the cycle were
    // never evaluated either. sets them to a circular reference error and resets
    // their counter so the next recalculation starts clean.
    pub fn detect_cycles(&self, sheets: &[Grid], changed_ranges: &[CellRange]) {
        let mut queue = VecDeque::new();
        let mut visited: RTree<VisitedRange> = RTree::new();
        let mut ranges_buf = Vec::new();

        for &changed_range in changed_ranges {
            queue.push_back(changed_range);
            visited.insert(VisitedRange::new(changed_range));
        }

        while let Some(prec_to_visit) = queue.pop_front() {
            let envelope = vertex_envelope(prec_to_visit);

            for iv in self.vertex_index.locate_in_envelope_intersecting(&envelope) {
                let Some(&vertex_range) = self.graph.node_weight(iv.vertex_index) else {
                    continue;
                };
                let Some(overlap) = vertex_range.intersection(&prec_to_visit) else {
                    continue;
                };

                for edge in self
                    .graph
                    .edges_directed(iv.vertex_index, Direction::Outgoing)
                {
                    let edge_id = edge.id();
                    overlap.for_each_cell(|cell| {
                        if let Some(dep_range) =
                            find_dependant_range(&self.graph, edge_id, CellRange::single(cell))
                        {
                            dep_range.for_each_cell(|dep_cell| {
                                if get_pending_counter(sheets, dep_cell) > 0 {
                                    set_circular_ref_error(sheets, dep_cell);
                                }
                            });
                        }
                    });
                }
            }

            self.collect_direct_dependant_ranges(prec_to_visit, &mut ranges_buf);
            for i in 0..ranges_buf.len() {
                for new_range in subtract_visited(&ranges_buf[i], &visited) {
                    visited.insert(VisitedRange::new(new_range));
                    queue.push_back(new_range);
                }
            }
        }
    }

    // decrement pending counters of direct dependants after evaluating a cell.
    // returns cells that became ready (counter dropped to 0).
    // thread-safe: allocates its own scratch buffer.
    pub fn decrease_pending_counter(
        &self,
        sheets: &[Grid],
        evaluated_range: CellRange,
    ) -> Vec<AbsoluteCellId> {
        let mut ready_cells = Vec::new();
        let mut ranges_buf = Vec::new();
        let envelope = vertex_envelope(evaluated_range);

        for iv in self.vertex_index.locate_in_envelope_intersecting(&envelope) {
            let Some(&vertex_range) = self.graph.node_weight(iv.vertex_index) else {
                continue;
            };
            let Some(overlap) = vertex_range.intersection(&evaluated_range) else {
                continue;
            };

            for edge in self
                .graph
                .edges_directed(iv.vertex_index, Direction::Outgoing)
            {
                if let Some(dep_range) = find_dependant_range(&self.graph, edge.id(), overlap) {
                    ranges_buf.push(dep_range);
                }
            }
        }

        for range in &ranges_buf {
            range.for_each_cell(|cell| {
                if decrease_pending_counter(sheets, cell) == 0 {
                    ready_cells.push(cell);
                }
            });
        }

        ready_cells
    }

    // --- querying ---

    // todo: remove call sites in favor of find_direct_dependant_ranges.

    // find all formula cells that directly depend on the given range
    pub fn direct_dependants_for_range(&self, dependency_range: CellRange) -> Vec<AbsoluteCellId> {
        let mut dependants = Vec::new();
        let mut seen = HashSet::new();
        let envelope = vertex_envelope(dependency_range);

        for iv in self.vertex_index.locate_in_envelope_intersecting(&envelope) {
            let Some(vertex_range) = self.graph.node_weight(iv.vertex_index).copied() else {
                continue;
            };
            let Some(overlap) = vertex_range.intersection(&dependency_range) else {
                continue;
            };

            for edge in self
                .graph
                .edges_directed(iv.vertex_index, Direction::Outgoing)
            {
                let Some(dependant_range) = find_dependant_range(&self.graph, edge.id(), overlap)
                else {
                    continue;
                };
                dependant_range.for_each_cell(|cell| {
                    if seen.insert(cell) {
                        dependants.push(cell);
                    }
                });
            }
        }

        dependants
    }

    // collect direct dependant ranges into buf.
    // walks outgoing edges of overlapping vertices, using findDep (paper Sec. III)
    // to compute the actual affected sub-range of each dependant.
    fn collect_direct_dependant_ranges(
        &self,
        dependency_range: CellRange,
        buf: &mut Vec<CellRange>,
    ) {
        buf.clear();
        let envelope = vertex_envelope(dependency_range);

        for iv in self.vertex_index.locate_in_envelope_intersecting(&envelope) {
            let Some(&vertex_range) = self.graph.node_weight(iv.vertex_index) else {
                continue;
            };
            let Some(overlap) = vertex_range.intersection(&dependency_range) else {
                continue;
            };

            for edge in self
                .graph
                .edges_directed(iv.vertex_index, Direction::Outgoing)
            {
                if let Some(dep_range) = find_dependant_range(&self.graph, edge.id(), overlap) {
                    buf.push(dep_range);
                }
            }
        }
    }

    // --- graphviz export ---

    pub fn to_dot(&self) -> String {
        let dot = Dot::with_attr_getters(
            &self.graph,
            &[
                Config::NodeNoLabel,
                Config::EdgeNoLabel,
                Config::RankDir(RankDir::LR),
            ],
            &|_, edge| {
                let edge_data = edge.weight();
                format!(
                    concat!(
                        "label=\"{}\"",
                        ", color=\"#94a3b8\"",
                        ", fontcolor=\"#dbe4ff\"",
                        ", penwidth=\"1.4\""
                    ),
                    format_edge_pattern(edge_data.pattern),
                )
            },
            &|graph, node| {
                let vertex_range = *node.weight();
                let has_incoming = graph
                    .edges_directed(node.id(), Direction::Incoming)
                    .next()
                    .is_some();
                let has_outgoing = graph
                    .edges_directed(node.id(), Direction::Outgoing)
                    .next()
                    .is_some();

                let fill_color = match (has_incoming, has_outgoing) {
                    (false, true) => "#14332b",
                    (true, false) => "#2f1834",
                    (true, true) => "#172033",
                    (false, false) => "#1f2937",
                };

                format!(
                    concat!(
                        "label=\"{}\"",
                        ", shape=\"box\"",
                        ", style=\"rounded,filled\"",
                        ", color=\"#64748b\"",
                        ", fillcolor=\"{}\"",
                        ", fontcolor=\"#f8fafc\"",
                        ", margin=\"0.14,0.08\""
                    ),
                    format_cell_range(vertex_range),
                    fill_color,
                )
            },
        );

        format!("{dot:?}")
    }

    // --- internal: vertex management ---

    // find or create a vertex for the given range.
    // uses the R-tree to check for an existing vertex with the exact same range.
    fn intern_vertex(&mut self, range: CellRange) -> NodeIndex {
        let envelope = vertex_envelope(range);
        let existing = self
            .vertex_index
            .locate_in_envelope_intersecting(&envelope)
            .find(|iv| self.graph.node_weight(iv.vertex_index) == Some(&range));

        if let Some(iv) = existing {
            return iv.vertex_index;
        }

        let vertex_index = self.graph.add_node(range);
        self.vertex_index
            .insert(IndexedVertex::new(vertex_index, range));
        vertex_index
    }

    // remove a vertex if it has no edges (no longer referenced by any compressed edge)
    fn trim_vertex_if_unused(&mut self, vertex_index: NodeIndex) {
        let Some(&vertex_range) = self.graph.node_weight(vertex_index) else {
            return;
        };
        let has_edges = self
            .graph
            .edges_directed(vertex_index, Direction::Outgoing)
            .next()
            .is_some()
            || self
                .graph
                .edges_directed(vertex_index, Direction::Incoming)
                .next()
                .is_some();
        if has_edges {
            return;
        }

        self.vertex_index
            .remove(&IndexedVertex::new(vertex_index, vertex_range));
        self.graph.remove_node(vertex_index);
    }

    // --- internal: edge compression (TACO Sec. IV-A) ---

    // try to insert a dependency edge, merging with existing edges when possible.
    //
    // first checks if the dependency is already covered by an existing edge.
    // then looks for adjacent edges that can be extended with a compression pattern.
    // if no compression is possible, inserts as a Single (uncompressed) edge.
    fn insert_dependency_range(&mut self, dependency_range: CellRange, dependant_range: CellRange) {
        if self.dependency_already_exists(dependency_range, dependant_range) {
            return;
        }

        let candidate_edge_indexes = self.find_candidate_edge_indexes(dependant_range);

        let mut best_replacement: Option<(EdgeIndex, PendingEdgeInsert)> = None;
        for edge_index in candidate_edge_indexes {
            let Some(candidate) =
                self.try_extend_edge(edge_index, dependency_range, dependant_range)
            else {
                continue;
            };

            if best_replacement.as_ref().map_or(true, |(_, current_best)| {
                compare_pending_edge_inserts(&candidate, current_best).is_gt()
            }) {
                best_replacement = Some((edge_index, candidate));
            }
        }

        if let Some((edge_index, replacement_edge)) = best_replacement {
            self.remove_edge(edge_index);
            self.insert_edge(replacement_edge);
            return;
        }

        self.insert_edge(PendingEdgeInsert {
            dependency_range,
            dependant_range,
            pattern: EdgePattern::Single,
            metadata: EdgeMetadata::default(),
        });
    }

    // check if the given dependency is already fully covered by an existing compressed edge.
    // uses early exit: breaks out of the R-tree traversal as soon as a covering edge is found.
    fn dependency_already_exists(
        &self,
        dependency_range: CellRange,
        dependant_range: CellRange,
    ) -> bool {
        let envelope = vertex_envelope(dependency_range);
        self.vertex_index
            .locate_in_envelope_intersecting_int(&envelope, |iv| {
                let Some(existing_dep_range) = self.graph.node_weight(iv.vertex_index) else {
                    return ControlFlow::<()>::Continue(());
                };
                if !existing_dep_range.contains(&dependency_range) {
                    return ControlFlow::<()>::Continue(());
                }

                let mut outgoing = self
                    .graph
                    .edges_directed(iv.vertex_index, Direction::Outgoing);
                while let Some(edge) = outgoing.next() {
                    let Some(existing_dependant_range) =
                        find_dependant_range(&self.graph, edge.id(), dependency_range)
                    else {
                        continue;
                    };
                    if existing_dependant_range.contains(&dependant_range) {
                        return ControlFlow::Break(());
                    }
                }

                ControlFlow::<()>::Continue(())
            })
            .is_break()
    }

    // find edges adjacent to the new dependant that might be extendable.
    // checks +-1 in row and column directions for overlapping dependant vertices.
    fn find_candidate_edge_indexes(&self, dependant_range: CellRange) -> Vec<EdgeIndex> {
        let mut candidate_edge_indexes = Vec::new();

        for (row_delta, col_delta) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
            let Some(shifted) = dependant_range.shifted(row_delta, col_delta) else {
                continue;
            };

            let envelope = vertex_envelope(shifted);
            for iv in self.vertex_index.locate_in_envelope_intersecting(&envelope) {
                for edge in self
                    .graph
                    .edges_directed(iv.vertex_index, Direction::Incoming)
                {
                    candidate_edge_indexes.push(edge.id());
                }
            }
        }

        candidate_edge_indexes.sort_unstable_by_key(|ei| ei.index());
        candidate_edge_indexes.dedup_by_key(|ei| ei.index());
        candidate_edge_indexes
    }

    // try to extend an existing edge to include the new dependency.
    // returns the merged edge if the new dependency matches the existing pattern.
    fn try_extend_edge(
        &self,
        edge_index: EdgeIndex,
        new_dependency_range: CellRange,
        new_dependant_range: CellRange,
    ) -> Option<PendingEdgeInsert> {
        let (dependency_vertex_index, dependant_vertex_index) =
            self.graph.edge_endpoints(edge_index)?;
        let existing_dependency_range = *self.graph.node_weight(dependency_vertex_index)?;
        let existing_dependant_range = *self.graph.node_weight(dependant_vertex_index)?;
        let edge_data = *self.graph.edge_weight(edge_index)?;

        if edge_data.pattern == EdgePattern::Single {
            let mut best_replacement = None;
            for pattern in [
                EdgePattern::RR,
                EdgePattern::RF,
                EdgePattern::FR,
                EdgePattern::FF,
            ] {
                let Some(candidate_replacement) = try_extend_with_pattern(
                    pattern,
                    existing_dependency_range,
                    existing_dependant_range,
                    new_dependency_range,
                    new_dependant_range,
                ) else {
                    continue;
                };

                if best_replacement.as_ref().map_or(true, |current_best| {
                    compare_pending_edge_inserts(&candidate_replacement, current_best).is_gt()
                }) {
                    best_replacement = Some(candidate_replacement);
                }
            }

            return best_replacement;
        }

        try_extend_existing_pattern(
            edge_data.pattern,
            edge_data.metadata,
            existing_dependency_range,
            existing_dependant_range,
            new_dependency_range,
            new_dependant_range,
        )
    }

    // split a compressed edge after removing a dependant cell.
    // the removed cell's sub-range is subtracted from the dependant range,
    // producing 0-4 replacement edges for the remaining sub-ranges.
    fn split_edge_after_removing_dependant(
        &self,
        edge_index: EdgeIndex,
        removed_dependant_range: CellRange,
    ) -> Vec<PendingEdgeInsert> {
        let Some((_, dependant_vertex_index)) = self.graph.edge_endpoints(edge_index) else {
            return Vec::new();
        };
        let Some(dependant_range) = self.graph.node_weight(dependant_vertex_index).copied() else {
            return Vec::new();
        };
        let Some(edge_data) = self.graph.edge_weight(edge_index).copied() else {
            return Vec::new();
        };

        let remaining_dependant_ranges = dependant_range.subtract(&removed_dependant_range);
        let mut replacement_edges = Vec::with_capacity(remaining_dependant_ranges.len());
        for remaining_dependant_range in remaining_dependant_ranges {
            replacement_edges.push(PendingEdgeInsert {
                dependency_range: find_dependency_range(
                    &self.graph,
                    edge_index,
                    remaining_dependant_range,
                ),
                dependant_range: remaining_dependant_range,
                pattern: if remaining_dependant_range.is_single() {
                    EdgePattern::Single
                } else {
                    edge_data.pattern
                },
                metadata: if remaining_dependant_range.is_single() {
                    EdgeMetadata::default()
                } else {
                    edge_data.metadata
                },
            });
        }

        replacement_edges
    }

    fn insert_edge(&mut self, edge_to_insert: PendingEdgeInsert) {
        let dependency_vertex_index = self.intern_vertex(edge_to_insert.dependency_range);
        let dependant_vertex_index = self.intern_vertex(edge_to_insert.dependant_range);
        self.graph.add_edge(
            dependency_vertex_index,
            dependant_vertex_index,
            CompressedEdgeData {
                pattern: edge_to_insert.pattern,
                metadata: edge_to_insert.metadata,
            },
        );
    }

    fn remove_edge(&mut self, edge_index: EdgeIndex) {
        let Some((dependency_vertex_index, dependant_vertex_index)) =
            self.graph.edge_endpoints(edge_index)
        else {
            return;
        };

        self.graph.remove_edge(edge_index);
        self.trim_vertex_if_unused(dependency_vertex_index);
        self.trim_vertex_if_unused(dependant_vertex_index);
    }

    #[cfg(test)]
    fn edge_count(&self) -> usize {
        self.graph.edge_count()
    }
}

// --- free functions: AST helpers ---

pub(crate) fn dependency_ranges_from_ast(
    ast: &[Expr],
    source_cell: &AbsoluteCellId,
) -> Vec<CellRange> {
    let mut dependency_ranges = Vec::new();

    for expr in ast {
        let Expr::Atom(ExprAtom::Reference(reference)) = expr else {
            continue;
        };
        dependency_ranges.push(reference.to_cell_range(source_cell));
    }

    dependency_ranges
}

// --- free functions: edge compression ---

// try to create a compressed edge from two Single edges that share a pattern.
// checks if both edges have the same relative/fixed metadata for the given pattern type.
fn try_extend_with_pattern(
    pattern: EdgePattern,
    existing_dependency_range: CellRange,
    existing_dependant_range: CellRange,
    new_dependency_range: CellRange,
    new_dependant_range: CellRange,
) -> Option<PendingEdgeInsert> {
    let existing_metadata =
        single_edge_metadata(pattern, existing_dependency_range, existing_dependant_range)?;
    let new_metadata = single_edge_metadata(pattern, new_dependency_range, new_dependant_range)?;
    if existing_metadata != new_metadata {
        return None;
    }

    let merged_dependant_range = existing_dependant_range.bounding_union(&new_dependant_range);
    if merged_dependant_range.sheet_id != existing_dependant_range.sheet_id
        || !merged_dependant_range.is_line()
    {
        return None;
    }

    Some(PendingEdgeInsert {
        dependency_range: existing_dependency_range.bounding_union(&new_dependency_range),
        dependant_range: merged_dependant_range,
        pattern,
        metadata: existing_metadata,
    })
}

// try to extend an already-compressed edge with a new Single edge
fn try_extend_existing_pattern(
    pattern: EdgePattern,
    existing_metadata: EdgeMetadata,
    existing_dependency_range: CellRange,
    existing_dependant_range: CellRange,
    new_dependency_range: CellRange,
    new_dependant_range: CellRange,
) -> Option<PendingEdgeInsert> {
    let new_metadata = single_edge_metadata(pattern, new_dependency_range, new_dependant_range)?;
    if new_metadata != existing_metadata {
        return None;
    }

    let merged_dependant_range = existing_dependant_range.bounding_union(&new_dependant_range);
    if merged_dependant_range.sheet_id != existing_dependant_range.sheet_id
        || !merged_dependant_range.is_line()
    {
        return None;
    }

    Some(PendingEdgeInsert {
        dependency_range: existing_dependency_range.bounding_union(&new_dependency_range),
        dependant_range: merged_dependant_range,
        pattern,
        metadata: existing_metadata,
    })
}

// compute the edge metadata for a single (uncompressed) edge under a given pattern.
// returns None if the dependant is not a single cell or sheets don't match.
fn single_edge_metadata(
    pattern: EdgePattern,
    dependency_range: CellRange,
    dependant_range: CellRange,
) -> Option<EdgeMetadata> {
    if !dependant_range.is_single() || dependency_range.sheet_id != dependant_range.sheet_id {
        return None;
    }

    let dependant_point = point_from_cell_id(dependant_range.head_cell());
    let dependency_head = point_from_cell_id(dependency_range.head_cell());
    let dependency_tail = point_from_cell_id(dependency_range.tail_cell());

    Some(match pattern {
        EdgePattern::Single => EdgeMetadata::default(),
        EdgePattern::RR => EdgeMetadata {
            head_relative_offset: Some(point_offset(dependency_head, dependant_point)),
            tail_relative_offset: Some(point_offset(dependency_tail, dependant_point)),
            ..EdgeMetadata::default()
        },
        EdgePattern::RF => EdgeMetadata {
            head_relative_offset: Some(point_offset(dependency_head, dependant_point)),
            tail_fixed_point: Some(dependency_tail),
            ..EdgeMetadata::default()
        },
        EdgePattern::FR => EdgeMetadata {
            head_fixed_point: Some(dependency_head),
            tail_relative_offset: Some(point_offset(dependency_tail, dependant_point)),
            ..EdgeMetadata::default()
        },
        EdgePattern::FF => EdgeMetadata {
            head_fixed_point: Some(dependency_head),
            tail_fixed_point: Some(dependency_tail),
            ..EdgeMetadata::default()
        },
    })
}

// --- free functions: findDep / findPrec (paper Sec. III) ---

// given a compressed edge and a sub-range of its dependency vertex,
// compute the corresponding sub-range of its dependant vertex.
// this is the TACO `findDep` operation.
fn find_dependant_range(
    graph: &StableDiGraph<CellRange, CompressedEdgeData>,
    edge_index: EdgeIndex,
    dependency_subrange: CellRange,
) -> Option<CellRange> {
    let (dependency_vertex_index, dependant_vertex_index) = graph.edge_endpoints(edge_index)?;
    let dependency_range = *graph.node_weight(dependency_vertex_index)?;
    let dependant_range = *graph.node_weight(dependant_vertex_index)?;
    let edge_data = *graph.edge_weight(edge_index)?;
    let overlap = dependency_range.intersection(&dependency_subrange)?;

    match edge_data.pattern {
        EdgePattern::Single => Some(dependant_range),
        EdgePattern::RR => {
            let axis = dependant_axis(dependant_range);
            let dependency_range = orient_range(dependency_range, axis);
            let dependant_range = orient_range(dependant_range, axis);
            let overlap = orient_range(overlap, axis);
            let metadata = orient_metadata(edge_data.metadata, axis);

            let dependency_overlap_head = CellPoint {
                row: overlap.start_row,
                col: dependency_range.end_col,
            };
            let dependant_head = point_sub(
                dependency_overlap_head,
                metadata.tail_relative_offset.unwrap(),
            );
            let dependency_overlap_tail = CellPoint {
                row: overlap.end_row,
                col: dependency_range.start_col,
            };
            let dependant_tail = point_sub(
                dependency_overlap_tail,
                metadata.head_relative_offset.unwrap(),
            );
            let found_dependant_range =
                range_from_points(dependant_range.sheet_id, dependant_head, dependant_tail)
                    .intersection(&dependant_range)?;
            Some(deorient_range(found_dependant_range, axis))
        }
        EdgePattern::RF => {
            let axis = dependant_axis(dependant_range);
            let dependency_range = orient_range(dependency_range, axis);
            let dependant_range = orient_range(dependant_range, axis);
            let overlap = orient_range(overlap, axis);
            let metadata = orient_metadata(edge_data.metadata, axis);

            let dependant_head = point_from_cell_id(dependant_range.head_cell());
            let dependency_overlap_tail = CellPoint {
                row: overlap.end_row,
                col: dependency_range.start_col,
            };
            let dependant_tail = point_sub(
                dependency_overlap_tail,
                metadata.head_relative_offset.unwrap(),
            );
            let found_dependant_range =
                range_from_points(dependant_range.sheet_id, dependant_head, dependant_tail)
                    .intersection(&dependant_range)?;
            Some(deorient_range(found_dependant_range, axis))
        }
        EdgePattern::FR => {
            let axis = dependant_axis(dependant_range);
            let dependency_range = orient_range(dependency_range, axis);
            let dependant_range = orient_range(dependant_range, axis);
            let overlap = orient_range(overlap, axis);
            let metadata = orient_metadata(edge_data.metadata, axis);

            let dependency_overlap_head = CellPoint {
                row: overlap.start_row,
                col: dependency_range.end_col,
            };
            let dependant_head = point_sub(
                dependency_overlap_head,
                metadata.tail_relative_offset.unwrap(),
            );
            let dependant_tail = point_from_cell_id(dependant_range.tail_cell());
            let found_dependant_range =
                range_from_points(dependant_range.sheet_id, dependant_head, dependant_tail)
                    .intersection(&dependant_range)?;
            Some(deorient_range(found_dependant_range, axis))
        }
        EdgePattern::FF => Some(dependant_range),
    }
}

// given a compressed edge and a sub-range of its dependant vertex,
// compute the corresponding sub-range of its dependency vertex.
// this is the TACO `findPrec` operation.
fn find_dependency_range(
    graph: &StableDiGraph<CellRange, CompressedEdgeData>,
    edge_index: EdgeIndex,
    dependant_subrange: CellRange,
) -> CellRange {
    let (dependency_vertex_index, dependant_vertex_index) =
        graph.edge_endpoints(edge_index).unwrap();
    let dependency_range = *graph.node_weight(dependency_vertex_index).unwrap();
    let dependant_range = *graph.node_weight(dependant_vertex_index).unwrap();
    let edge_data = *graph.edge_weight(edge_index).unwrap();

    if edge_data.pattern == EdgePattern::Single {
        return dependency_range;
    }

    let axis = dependant_axis(dependant_range);
    let dependant_subrange = orient_range(dependant_subrange, axis);
    let metadata = orient_metadata(edge_data.metadata, axis);

    let dependency_range = match edge_data.pattern {
        EdgePattern::RR => {
            let dependency_head = point_add(
                point_from_cell_id(dependant_subrange.head_cell()),
                metadata.head_relative_offset.unwrap(),
            );
            let dependency_tail = point_add(
                point_from_cell_id(dependant_subrange.tail_cell()),
                metadata.tail_relative_offset.unwrap(),
            );
            range_from_points(
                dependant_subrange.sheet_id,
                dependency_head,
                dependency_tail,
            )
        }
        EdgePattern::RF => {
            let dependency_head = point_add(
                point_from_cell_id(dependant_subrange.head_cell()),
                metadata.head_relative_offset.unwrap(),
            );
            range_from_points(
                dependant_subrange.sheet_id,
                dependency_head,
                metadata.tail_fixed_point.unwrap(),
            )
        }
        EdgePattern::FR => {
            let dependency_tail = point_add(
                point_from_cell_id(dependant_subrange.tail_cell()),
                metadata.tail_relative_offset.unwrap(),
            );
            range_from_points(
                dependant_subrange.sheet_id,
                metadata.head_fixed_point.unwrap(),
                dependency_tail,
            )
        }
        EdgePattern::FF => range_from_points(
            dependant_subrange.sheet_id,
            metadata.head_fixed_point.unwrap(),
            metadata.tail_fixed_point.unwrap(),
        ),
        EdgePattern::Single => unreachable!(),
    };

    deorient_range(dependency_range, axis)
}

// --- free functions: edge scoring ---

fn compare_pending_edge_inserts(
    left: &PendingEdgeInsert,
    right: &PendingEdgeInsert,
) -> std::cmp::Ordering {
    score_pending_edge_insert(left).cmp(&score_pending_edge_insert(right))
}

fn score_pending_edge_insert(edge_to_insert: &PendingEdgeInsert) -> (u8, u8, u32) {
    (
        edge_to_insert.dependant_range.is_col_vector() as u8,
        match edge_to_insert.pattern {
            EdgePattern::Single => 0,
            EdgePattern::RR => 1,
            EdgePattern::RF | EdgePattern::FR => 2,
            EdgePattern::FF => 3,
        },
        edge_to_insert.dependant_range.cell_count(),
    )
}

// --- free functions: axis orientation ---

// dependant ranges are always line ranges. this determines which axis they extend along.
fn dependant_axis(dependant_range: CellRange) -> Axis {
    if dependant_range.is_row_vector() {
        Axis::Row
    } else {
        Axis::Col
    }
}

// orient a range so that the "varying" axis is always the row axis.
// for column vectors this is a no-op; for row vectors it transposes row<->col.
fn orient_range(range: CellRange, axis: Axis) -> CellRange {
    match axis {
        Axis::Col => range,
        Axis::Row => CellRange::new(
            range.sheet_id,
            range.start_col,
            range.start_row,
            range.end_col,
            range.end_row,
        ),
    }
}

fn deorient_range(range: CellRange, axis: Axis) -> CellRange {
    orient_range(range, axis)
}

fn orient_metadata(metadata: EdgeMetadata, axis: Axis) -> EdgeMetadata {
    match axis {
        Axis::Col => metadata,
        Axis::Row => EdgeMetadata {
            head_relative_offset: metadata.head_relative_offset.map(transpose_offset),
            tail_relative_offset: metadata.tail_relative_offset.map(transpose_offset),
            head_fixed_point: metadata.head_fixed_point.map(transpose_point),
            tail_fixed_point: metadata.tail_fixed_point.map(transpose_point),
        },
    }
}

// --- free functions: point arithmetic ---

fn transpose_offset(offset: Offset) -> Offset {
    Offset {
        row: offset.col,
        col: offset.row,
    }
}

fn transpose_point(point: CellPoint) -> CellPoint {
    CellPoint {
        row: point.col,
        col: point.row,
    }
}

fn point_from_cell_id(cell_id: AbsoluteCellId) -> CellPoint {
    CellPoint {
        row: cell_id.row,
        col: cell_id.col,
    }
}

fn point_add(point: CellPoint, offset: Offset) -> CellPoint {
    CellPoint {
        row: (point.row as i32 + offset.row).max(0) as u32,
        col: (point.col as i32 + offset.col).max(0) as u32,
    }
}

fn point_sub(point: CellPoint, offset: Offset) -> CellPoint {
    CellPoint {
        row: (point.row as i32 - offset.row).max(0) as u32,
        col: (point.col as i32 - offset.col).max(0) as u32,
    }
}

fn point_offset(target: CellPoint, origin: CellPoint) -> Offset {
    Offset {
        row: target.row as i32 - origin.row as i32,
        col: target.col as i32 - origin.col as i32,
    }
}

fn range_from_points(sheet_id: u32, head: CellPoint, tail: CellPoint) -> CellRange {
    CellRange::new(sheet_id, head.row, head.col, tail.row, tail.col)
}

// --- free functions: formatting ---

fn format_edge_pattern(pattern: EdgePattern) -> &'static str {
    match pattern {
        EdgePattern::Single => "Single",
        EdgePattern::RR => "RR",
        EdgePattern::RF => "RF",
        EdgePattern::FR => "FR",
        EdgePattern::FF => "FF",
    }
}

fn format_cell_range(range: CellRange) -> String {
    let start = format_cell_address(range.start_row, range.start_col);
    let end = format_cell_address(range.end_row, range.end_col);
    let prefix = format!("S{}!", range.sheet_id);

    if range.is_single() {
        return prefix + &start;
    }

    format!("{prefix}{start}:{end}")
}

fn format_cell_address(row: u32, col: u32) -> String {
    format!("{}{}", column_name(col), row + 1)
}

fn column_name(col: u32) -> String {
    let mut value = col + 1;
    let mut letters = Vec::new();

    while value > 0 {
        let remainder = ((value - 1) % 26) as u8;
        letters.push((b'A' + remainder) as char);
        value = (value - 1) / 26;
    }

    letters.iter().rev().collect()
}

fn vertex_envelope(range: CellRange) -> AABB<[i64; 3]> {
    AABB::from_corners(
        [
            range.sheet_id as i64,
            range.start_row as i64,
            range.start_col as i64,
        ],
        [
            range.sheet_id as i64,
            range.end_row as i64,
            range.end_col as i64,
        ],
    )
}

// --- free functions: pending counter helpers ---

// atomic: no &mut needed
fn increase_pending_counter(sheets: &[Grid], cell_id: AbsoluteCellId) {
    let grid_cell_id: GridCellId = (&cell_id).into();
    sheets[cell_id.sheet_id as usize].increase_pending_dependencies(&grid_cell_id);
}

// atomic: returns value after decrement
fn decrease_pending_counter(sheets: &[Grid], cell_id: AbsoluteCellId) -> u32 {
    let grid_cell_id: GridCellId = (&cell_id).into();
    sheets[cell_id.sheet_id as usize].decrease_pending_dependencies(&grid_cell_id)
}

fn get_pending_counter(sheets: &[Grid], cell_id: AbsoluteCellId) -> u32 {
    let grid_cell_id: GridCellId = (&cell_id).into();
    sheets[cell_id.sheet_id as usize].get_pending_dependencies(&grid_cell_id)
}

fn set_circular_ref_error(sheets: &[Grid], cell_id: AbsoluteCellId) {
    let grid_cell_id: GridCellId = (&cell_id).into();
    let grid = &sheets[cell_id.sheet_id as usize];
    grid.set_value(&grid_cell_id, CellValue::err("Cycle"));
    grid.reset_pending_dependencies(&grid_cell_id);
}

// --- free functions: BFS dedup ---

// subtract all visited ranges from `range`, returning the unvisited portions
//
// queries the visited R-tree for overlapping ranges, then iteratively
// subtracts each overlap. the result is a list of sub-ranges that have
// not been visited yet.
fn subtract_visited(range: &CellRange, visited: &RTree<VisitedRange>) -> Vec<CellRange> {
    let envelope = vertex_envelope(*range);
    let overlapping: Vec<CellRange> = visited
        .locate_in_envelope_intersecting(&envelope)
        .filter(|vr| vr.range.sheet_id == range.sheet_id)
        .map(|vr| vr.range)
        .collect();

    let mut remaining = vec![*range];
    for visited_range in overlapping {
        remaining = remaining
            .into_iter()
            .flat_map(|r| r.subtract(&visited_range))
            .collect();

        if remaining.is_empty() {
            break;
        }
    }

    remaining
}

// --- tests ---

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::atomic::AtomicU32;

    use crate::storage::{
        grid::{Cell, CellValue},
        types::FormulaId,
    };

    fn cell(sheet_id: u32, row: u32, col: u32) -> AbsoluteCellId {
        AbsoluteCellId { sheet_id, row, col }
    }

    fn range(
        sheet_id: u32,
        start_row: u32,
        start_col: u32,
        end_row: u32,
        end_col: u32,
    ) -> CellRange {
        CellRange::new(sheet_id, start_row, start_col, end_row, end_col)
    }

    fn sheets_with_cells(cells: &[AbsoluteCellId]) -> Vec<Grid> {
        let mut sheets = vec![Grid::default()];
        for cell_id in cells {
            let grid_cell_id: GridCellId = cell_id.into();
            sheets[0].insert_cell(
                &grid_cell_id,
                Cell {
                    defined_by_formula: None,
                    val: CellValue::err(""),
                    pending_dependencies: AtomicU32::new(0),
                },
            );
        }
        sheets
    }

    fn sheets_with_formula_cells(
        value_cells: &[AbsoluteCellId],
        formula_cells: &[(AbsoluteCellId, FormulaId)],
    ) -> Vec<Grid> {
        let mut sheets = sheets_with_cells(value_cells);
        for (cell_id, formula_id) in formula_cells {
            let grid_cell_id: GridCellId = cell_id.into();
            sheets[0].insert_cell(
                &grid_cell_id,
                Cell {
                    defined_by_formula: Some(*formula_id),
                    val: CellValue::err(""),
                    pending_dependencies: AtomicU32::new(0),
                },
            );
        }
        sheets
    }

    fn sort_cells(cells: &mut Vec<AbsoluteCellId>) {
        cells.sort_unstable_by_key(|c| (c.sheet_id, c.row, c.col));
    }

    #[test]
    fn merges_rr_edges() {
        let mut graph = DependencyGraph::new();
        graph.insert_dependency_range(range(0, 0, 0, 2, 0), CellRange::single(cell(0, 0, 2)));
        graph.insert_dependency_range(range(0, 1, 0, 3, 0), CellRange::single(cell(0, 1, 2)));

        assert_eq!(graph.edge_count(), 1);
    }

    #[test]
    fn merges_ff_edges() {
        let mut graph = DependencyGraph::new();
        graph.insert_dependency_range(range(0, 0, 0, 2, 1), CellRange::single(cell(0, 0, 2)));
        graph.insert_dependency_range(range(0, 0, 0, 2, 1), CellRange::single(cell(0, 1, 2)));

        assert_eq!(graph.edge_count(), 1);
    }

    #[test]
    fn remove_formula_cell_splits_edge() {
        let mut graph = DependencyGraph::new();
        graph.insert_dependency_range(range(0, 0, 0, 2, 1), CellRange::single(cell(0, 0, 2)));
        graph.insert_dependency_range(range(0, 0, 0, 2, 1), CellRange::single(cell(0, 1, 2)));
        graph.insert_dependency_range(range(0, 0, 0, 2, 1), CellRange::single(cell(0, 2, 2)));

        graph.remove_formula_cell(cell(0, 1, 2));

        assert_eq!(graph.edge_count(), 2);
    }

    #[test]
    fn rr_direct_dependants_expand_subrange() {
        let mut graph = DependencyGraph::new();
        graph.insert_dependency_range(range(0, 0, 0, 2, 0), CellRange::single(cell(0, 0, 2)));
        graph.insert_dependency_range(range(0, 1, 0, 3, 0), CellRange::single(cell(0, 1, 2)));
        graph.insert_dependency_range(range(0, 2, 0, 4, 0), CellRange::single(cell(0, 2, 2)));

        let mut dependants = graph.direct_dependants_for_range(CellRange::single(cell(0, 2, 0)));
        sort_cells(&mut dependants);

        assert_eq!(
            dependants,
            vec![cell(0, 0, 2), cell(0, 1, 2), cell(0, 2, 2)]
        );
    }

    #[test]
    fn duplicate_subsumed_dependency_is_skipped() {
        let mut graph = DependencyGraph::new();
        graph.insert_dependency_range(range(0, 0, 0, 2, 0), CellRange::single(cell(0, 0, 2)));
        graph.insert_dependency_range(range(0, 1, 0, 1, 0), CellRange::single(cell(0, 0, 2)));

        assert_eq!(graph.edge_count(), 1);
    }

    // A(value) -> C(formula) -> E(formula)
    // changing A should make C ready first, then E
    #[test]
    fn init_and_decrease_pending_counter_follow_chain() {
        let mut graph = DependencyGraph::new();
        graph.insert_dependency_range(
            CellRange::single(cell(0, 0, 0)),
            CellRange::single(cell(0, 0, 2)),
        );
        graph.insert_dependency_range(
            CellRange::single(cell(0, 0, 2)),
            CellRange::single(cell(0, 0, 4)),
        );

        let mut sheets =
            sheets_with_formula_cells(&[cell(0, 0, 0)], &[(cell(0, 0, 2), 0), (cell(0, 0, 4), 1)]);

        graph.init_pending_counter_for_new_recalculation(
            &mut sheets,
            &[CellRange::single(cell(0, 0, 0))],
        );

        // C: edge from A = 1
        // E: edge from C = 1
        assert_eq!(get_pending_counter(&sheets, cell(0, 0, 2)), 1);
        assert_eq!(get_pending_counter(&sheets, cell(0, 0, 4)), 1);

        // simulate evaluating the changed cell A (decrease its dependants)
        let first_wave =
            graph.decrease_pending_counter(&mut sheets, CellRange::single(cell(0, 0, 0)));
        assert_eq!(first_wave, vec![cell(0, 0, 2)]);

        // simulate evaluating C
        let second_wave =
            graph.decrease_pending_counter(&mut sheets, CellRange::single(cell(0, 0, 2)));
        assert_eq!(second_wave, vec![cell(0, 0, 4)]);
    }

    // both A and B are changed formula cells, A -> B
    // B should wait for A
    #[test]
    fn init_pending_counter_counts_edges_between_changed_cells() {
        let mut graph = DependencyGraph::new();
        graph.insert_dependency_range(
            CellRange::single(cell(0, 0, 0)),
            CellRange::single(cell(0, 0, 2)),
        );

        let mut sheets = sheets_with_formula_cells(&[], &[(cell(0, 0, 0), 0), (cell(0, 0, 2), 1)]);

        // init from both A and B simultaneously
        graph.init_pending_counter_for_new_recalculation(
            &mut sheets,
            &[
                CellRange::single(cell(0, 0, 0)),
                CellRange::single(cell(0, 0, 2)),
            ],
        );

        // A has no affected predecessors, counter = 0
        // B has one edge from A, counter = 1
        assert_eq!(get_pending_counter(&sheets, cell(0, 0, 0)), 0);
        assert_eq!(get_pending_counter(&sheets, cell(0, 0, 2)), 1);

        // decrease from A -> B becomes ready
        let ready = graph.decrease_pending_counter(&mut sheets, CellRange::single(cell(0, 0, 0)));
        assert_eq!(ready, vec![cell(0, 0, 2)]);
    }

    // A -> B and B -> A (cycle)
    #[test]
    fn cycle_cells_get_counters() {
        let mut graph = DependencyGraph::new();
        graph.insert_dependency_range(
            CellRange::single(cell(0, 0, 0)),
            CellRange::single(cell(0, 0, 1)),
        );
        graph.insert_dependency_range(
            CellRange::single(cell(0, 0, 1)),
            CellRange::single(cell(0, 0, 0)),
        );

        let mut sheets = sheets_with_formula_cells(&[], &[(cell(0, 0, 0), 0), (cell(0, 0, 1), 1)]);

        graph.init_pending_counter_for_new_recalculation(
            &mut sheets,
            &[CellRange::single(cell(0, 0, 0))],
        );

        // BFS from A: discovers B (edge from A = 1), then from B discovers A
        // but A is already visited, so no enqueue. A still gets edge from B = 1.
        assert_eq!(get_pending_counter(&sheets, cell(0, 0, 0)), 1);
        assert_eq!(get_pending_counter(&sheets, cell(0, 0, 1)), 1);
    }

    // A(value, deleted) -> C(formula)
    // C should still get its counter incremented
    #[test]
    fn deleted_changed_cell_still_affects_dependants() {
        let mut graph = DependencyGraph::new();
        graph.insert_dependency_range(
            CellRange::single(cell(0, 0, 0)),
            CellRange::single(cell(0, 0, 2)),
        );

        let mut sheets = sheets_with_formula_cells(&[], &[(cell(0, 0, 2), 0)]);
        graph.init_pending_counter_for_new_recalculation(
            &mut sheets,
            &[CellRange::single(cell(0, 0, 0))],
        );

        // C: edge from A = 1
        assert_eq!(get_pending_counter(&sheets, cell(0, 0, 2)), 1);

        let ready = graph.decrease_pending_counter(&mut sheets, CellRange::single(cell(0, 0, 0)));
        assert_eq!(ready, vec![cell(0, 0, 2)]);
    }

    // diamond: A -> B, A -> C, B -> D, C -> D
    // D should only become ready after both B and C are evaluated
    #[test]
    fn diamond_dependency_waits_for_all_predecessors() {
        let mut graph = DependencyGraph::new();
        graph.insert_dependency_range(
            CellRange::single(cell(0, 0, 0)),
            CellRange::single(cell(0, 0, 1)),
        );
        graph.insert_dependency_range(
            CellRange::single(cell(0, 0, 0)),
            CellRange::single(cell(0, 0, 2)),
        );
        graph.insert_dependency_range(
            CellRange::single(cell(0, 0, 1)),
            CellRange::single(cell(0, 0, 3)),
        );
        graph.insert_dependency_range(
            CellRange::single(cell(0, 0, 2)),
            CellRange::single(cell(0, 0, 3)),
        );

        let mut sheets = sheets_with_formula_cells(
            &[cell(0, 0, 0)],
            &[(cell(0, 0, 1), 0), (cell(0, 0, 2), 1), (cell(0, 0, 3), 2)],
        );

        graph.init_pending_counter_for_new_recalculation(
            &mut sheets,
            &[CellRange::single(cell(0, 0, 0))],
        );

        // B: edge from A = 1
        // C: edge from A = 1
        // D: edge from B + edge from C = 2
        assert_eq!(get_pending_counter(&sheets, cell(0, 0, 1)), 1);
        assert_eq!(get_pending_counter(&sheets, cell(0, 0, 2)), 1);
        assert_eq!(get_pending_counter(&sheets, cell(0, 0, 3)), 2);

        // decrease from A -> B and C become ready (counter 0)
        let mut ready =
            graph.decrease_pending_counter(&mut sheets, CellRange::single(cell(0, 0, 0)));
        sort_cells(&mut ready);
        assert_eq!(ready, vec![cell(0, 0, 1), cell(0, 0, 2)]);

        // decrease from B -> D goes to 1, not ready
        let ready = graph.decrease_pending_counter(&mut sheets, CellRange::single(cell(0, 0, 1)));
        assert!(ready.is_empty());

        // decrease from C -> D goes to 0, ready!
        let ready = graph.decrease_pending_counter(&mut sheets, CellRange::single(cell(0, 0, 2)));
        assert_eq!(ready, vec![cell(0, 0, 3)]);
    }
}
