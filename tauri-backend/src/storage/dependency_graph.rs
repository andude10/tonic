use std::collections::{HashMap, HashSet};

use petgraph::{
    dot::{Config, Dot, RankDir},
    stable_graph::{EdgeIndex, NodeIndex, StableDiGraph},
    visit::{EdgeRef, NodeRef},
    Direction,
};
use rstar::{RTree, RTreeObject, AABB};

use crate::storage::{
    grid::{Grid, GridCellId},
    types::{AbsoluteCellId, CellRange, Expr, ExprAtom},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Axis {
    Row,
    Col,
}

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

pub struct DependencyGraph {
    graph: StableDiGraph<CellRange, CompressedEdgeData>,
    vertex_lookup: HashMap<CellRange, NodeIndex>,
    vertex_index: RTree<IndexedVertex>,

    cached_dependants_by_dependency: HashMap<AbsoluteCellId, std::ops::Range<usize>>,
    cached_dependants: Vec<AbsoluteCellId>,
    changed_value_cells: Vec<AbsoluteCellId>,
    changed_value_ranges: Vec<CellRange>,
    affected_cells: Vec<AbsoluteCellId>,
    ready_cells: Vec<AbsoluteCellId>,
    pending_queue: Vec<AbsoluteCellId>,
    current_dependants: Vec<AbsoluteCellId>,
    current_dependant_set: HashSet<AbsoluteCellId>,
    candidate_edge_indexes: Vec<EdgeIndex>,
    overlapping_vertex_indexes: Vec<NodeIndex>,
}

impl Default for DependencyGraph {
    fn default() -> Self {
        Self {
            graph: StableDiGraph::new(),
            vertex_lookup: HashMap::new(),
            vertex_index: RTree::new(),
            cached_dependants_by_dependency: HashMap::new(),
            cached_dependants: Vec::new(),
            changed_value_cells: Vec::new(),
            changed_value_ranges: Vec::new(),
            affected_cells: Vec::new(),
            ready_cells: Vec::new(),
            pending_queue: Vec::new(),
            current_dependants: Vec::new(),
            current_dependant_set: HashSet::new(),
            candidate_edge_indexes: Vec::new(),
            overlapping_vertex_indexes: Vec::new(),
        }
    }
}

// todo: write comments

impl DependencyGraph {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn clear(&mut self) {
        self.graph = StableDiGraph::new();
        self.vertex_lookup.clear();
        self.vertex_index = RTree::new();
        self.cached_dependants_by_dependency.clear();
        self.cached_dependants.clear();
        self.changed_value_cells.clear();
        self.changed_value_ranges.clear();
        self.affected_cells.clear();
        self.ready_cells.clear();
        self.pending_queue.clear();
        self.current_dependants.clear();
        self.current_dependant_set.clear();
        self.candidate_edge_indexes.clear();
        self.overlapping_vertex_indexes.clear();
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

    pub fn remove_formula_cell(&mut self, formula_cell: AbsoluteCellId) {
        let removed_dependant_cell = CellRange::single(formula_cell);
        self.collect_overlapping_vertex_indexes(removed_dependant_cell);
        self.candidate_edge_indexes.clear();

        let graph = &self.graph;
        let overlapping_vertex_indexes = &self.overlapping_vertex_indexes;
        let candidate_edge_indexes = &mut self.candidate_edge_indexes;

        for position in 0..overlapping_vertex_indexes.len() {
            let dependant_vertex_index = overlapping_vertex_indexes[position];
            let Some(dependant_vertex_range) = graph.node_weight(dependant_vertex_index) else {
                continue;
            };
            if !dependant_vertex_range.intersects(&removed_dependant_cell) {
                continue;
            }

            let mut incoming_edges =
                graph.edges_directed(dependant_vertex_index, Direction::Incoming);
            while let Some(incoming_edge) = incoming_edges.next() {
                candidate_edge_indexes.push(incoming_edge.id());
            }
        }

        candidate_edge_indexes.sort_unstable_by_key(|edge_index| edge_index.index());
        candidate_edge_indexes.dedup_by_key(|edge_index| edge_index.index());

        let edge_indexes_to_update = self.candidate_edge_indexes.clone();
        for edge_index in edge_indexes_to_update {
            let Some((_, dependant_vertex_index)) = self.graph.edge_endpoints(edge_index) else {
                continue;
            };
            let Some(dependant_vertex_range) =
                self.graph.node_weight(dependant_vertex_index).copied()
            else {
                continue;
            };
            let Some(removed_dependant_range) =
                dependant_vertex_range.intersection(&removed_dependant_cell)
            else {
                continue;
            };

            let replacement_edges =
                self.split_edge_after_removing_dependant(edge_index, removed_dependant_range);
            self.remove_edge(edge_index);
            for replacement_edge in replacement_edges {
                self.insert_edge(replacement_edge);
            }
        }
    }

    pub fn init_pending_counter_for_new_recalculation(
        &mut self,
        sheets: &mut [Grid],
        changed_cells: &[AbsoluteCellId],
    ) -> &[AbsoluteCellId] {
        // pending counters stay at 1 after the previous recalculation
        // so clear the old affected set first
        for position in 0..self.affected_cells.len() {
            reset_pending_counter(sheets, self.affected_cells[position]);
        }

        // clear per-recalculation scratch state
        self.cached_dependants_by_dependency.clear();
        self.cached_dependants.clear();
        self.changed_value_cells.clear();
        self.changed_value_ranges.clear();
        self.affected_cells.clear();
        self.ready_cells.clear();
        self.pending_queue.clear();

        // changed value cells are already final
        //
        // changed formula cells still need evaluation, so they participate
        // in the pending-count graph immediately
        for changed_cell in changed_cells {
            if cell_requires_recalculation(sheets, *changed_cell) {
                if get_pending_counter(sheets, *changed_cell) == 0 {
                    increase_pending_counter(sheets, *changed_cell);
                    self.affected_cells.push(*changed_cell);
                    self.pending_queue.push(*changed_cell);
                }
            } else {
                self.changed_value_cells.push(*changed_cell);
            }
        }

        // first discover formulas that depend directly on changed values
        //
        // merge changed values into line ranges so one compressed edge can be
        // queried once instead of once per changed cell
        self.changed_value_ranges = merge_cells_into_line_ranges(&self.changed_value_cells);

        for position in 0..self.changed_value_ranges.len() {
            let changed_value_range = self.changed_value_ranges[position];
            self.collect_direct_dependants_for_range(changed_value_range);

            for dependant_position in 0..self.current_dependants.len() {
                let dependant_cell = self.current_dependants[dependant_position];
                if get_pending_counter(sheets, dependant_cell) == 0 {
                    increase_pending_counter(sheets, dependant_cell);
                    self.affected_cells.push(dependant_cell);
                    self.pending_queue.push(dependant_cell);
                }
            }
        }

        // then walk only formula cells
        //
        // every discovered formula cell can delay its formula dependants,
        // so only this part contributes to pending counts
        let mut queue_head = 0;
        while queue_head < self.pending_queue.len() {
            let dependency_formula_cell = self.pending_queue[queue_head];
            queue_head += 1;

            self.collect_direct_dependants(dependency_formula_cell);

            let cached_dependants_start = self.cached_dependants.len();
            self.cached_dependants
                .extend_from_slice(&self.current_dependants);
            let cached_dependants_end = self.cached_dependants.len();
            self.cached_dependants_by_dependency.insert(
                dependency_formula_cell,
                cached_dependants_start..cached_dependants_end,
            );

            for position in 0..self.current_dependants.len() {
                let dependant_cell = self.current_dependants[position];
                if get_pending_counter(sheets, dependant_cell) == 0 {
                    increase_pending_counter(sheets, dependant_cell);
                    self.affected_cells.push(dependant_cell);
                    self.pending_queue.push(dependant_cell);
                }

                // every affected formula predecessor increments in-degree
                increase_pending_counter(sheets, dependant_cell);
            }
        }

        // collect the first evaluation wave
        for position in 0..self.affected_cells.len() {
            let affected_cell = self.affected_cells[position];
            let pending_counter = get_pending_counter(sheets, affected_cell);
            if pending_counter == 1
                || (pending_counter == 0 && changed_cells.contains(&affected_cell))
            {
                self.ready_cells.push(affected_cell);
            }
        }

        &self.ready_cells
    }

    pub fn decrease_pending_counter(
        &mut self,
        sheets: &mut [Grid],
        dependency_cell: AbsoluteCellId,
    ) -> &[AbsoluteCellId] {
        self.ready_cells.clear();
        let Some(cached_range) = self
            .cached_dependants_by_dependency
            .get(&dependency_cell)
            .cloned()
        else {
            self.collect_direct_dependants(dependency_cell);

            for position in 0..self.current_dependants.len() {
                let dependant_cell = self.current_dependants[position];
                decrease_pending_counter(sheets, dependant_cell);
                if get_pending_counter(sheets, dependant_cell) == 1 {
                    self.ready_cells.push(dependant_cell);
                }
            }

            return &self.ready_cells;
        };

        for position in cached_range {
            let dependant_cell = self.cached_dependants[position];
            decrease_pending_counter(sheets, dependant_cell);
            if get_pending_counter(sheets, dependant_cell) == 1 {
                self.ready_cells.push(dependant_cell);
            }
        }

        &self.ready_cells
    }

    pub fn collect_unresolved_cells(&mut self, sheets: &[Grid]) -> &[AbsoluteCellId] {
        self.ready_cells.clear();

        for position in 0..self.affected_cells.len() {
            let affected_cell = self.affected_cells[position];
            if get_pending_counter(sheets, affected_cell) > 1 {
                self.ready_cells.push(affected_cell);
            }
        }

        &self.ready_cells
    }

    pub fn direct_dependants_for_range(
        &mut self,
        dependency_range: CellRange,
    ) -> Vec<AbsoluteCellId> {
        self.collect_direct_dependants_for_range(dependency_range);
        self.current_dependants.clone()
    }

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

    #[cfg(test)]
    fn edge_count(&self) -> usize {
        self.graph.edge_count()
    }

    fn insert_dependency_range(&mut self, dependency_range: CellRange, dependant_range: CellRange) {
        if self.dependency_already_exists(dependency_range, dependant_range) {
            return;
        }

        self.collect_candidate_edge_indexes(dependant_range);

        let mut best_replacement: Option<(EdgeIndex, PendingEdgeInsert)> = None;
        for position in 0..self.candidate_edge_indexes.len() {
            let edge_index = self.candidate_edge_indexes[position];
            let Some(candidate_replacement) =
                self.try_extend_edge(edge_index, dependency_range, dependant_range)
            else {
                continue;
            };

            if best_replacement.as_ref().map_or(true, |(_, current_best)| {
                compare_pending_edge_inserts(&candidate_replacement, current_best).is_gt()
            }) {
                best_replacement = Some((edge_index, candidate_replacement));
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

    fn dependency_already_exists(
        &mut self,
        dependency_range: CellRange,
        dependant_range: CellRange,
    ) -> bool {
        self.collect_overlapping_vertex_indexes(dependency_range);

        let graph = &self.graph;
        for position in 0..self.overlapping_vertex_indexes.len() {
            let dependency_vertex_index = self.overlapping_vertex_indexes[position];
            let Some(existing_dependency_range) = graph.node_weight(dependency_vertex_index) else {
                continue;
            };
            if !existing_dependency_range.contains(&dependency_range) {
                continue;
            }

            let mut outgoing_edges =
                graph.edges_directed(dependency_vertex_index, Direction::Outgoing);
            while let Some(outgoing_edge) = outgoing_edges.next() {
                let Some(existing_dependant_range) =
                    find_dependant_range(graph, outgoing_edge.id(), dependency_range)
                else {
                    continue;
                };
                if existing_dependant_range.contains(&dependant_range) {
                    return true;
                }
            }
        }

        false
    }

    fn collect_candidate_edge_indexes(&mut self, dependant_range: CellRange) {
        self.candidate_edge_indexes.clear();

        for (row_delta, col_delta) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
            let Some(shifted_dependant_range) = dependant_range.shifted(row_delta, col_delta)
            else {
                continue;
            };

            self.collect_overlapping_vertex_indexes(shifted_dependant_range);
            let graph = &self.graph;

            for position in 0..self.overlapping_vertex_indexes.len() {
                let dependant_vertex_index = self.overlapping_vertex_indexes[position];
                let mut incoming_edges =
                    graph.edges_directed(dependant_vertex_index, Direction::Incoming);
                while let Some(incoming_edge) = incoming_edges.next() {
                    self.candidate_edge_indexes.push(incoming_edge.id());
                }
            }
        }

        self.candidate_edge_indexes
            .sort_unstable_by_key(|edge_index| edge_index.index());
        self.candidate_edge_indexes
            .dedup_by_key(|edge_index| edge_index.index());
    }

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

    fn collect_direct_dependants(&mut self, dependency_cell: AbsoluteCellId) {
        self.collect_direct_dependants_for_range(CellRange::single(dependency_cell));
    }

    fn collect_direct_dependants_for_range(&mut self, dependency_range: CellRange) {
        self.current_dependants.clear();
        self.current_dependant_set.clear();

        self.collect_overlapping_vertex_indexes(dependency_range);

        let graph = &self.graph;
        let overlapping_vertex_indexes = &self.overlapping_vertex_indexes;
        let current_dependants = &mut self.current_dependants;
        let current_dependant_set = &mut self.current_dependant_set;

        for position in 0..overlapping_vertex_indexes.len() {
            let dependency_vertex_index = overlapping_vertex_indexes[position];
            let Some(dependency_vertex_range) = graph.node_weight(dependency_vertex_index).copied()
            else {
                continue;
            };
            let Some(overlap) = dependency_vertex_range.intersection(&dependency_range) else {
                continue;
            };

            let mut outgoing_edges =
                graph.edges_directed(dependency_vertex_index, Direction::Outgoing);
            while let Some(outgoing_edge) = outgoing_edges.next() {
                let Some(dependant_range) =
                    find_dependant_range(graph, outgoing_edge.id(), overlap)
                else {
                    continue;
                };

                dependant_range.for_each_cell(|dependant_cell| {
                    if current_dependant_set.insert(dependant_cell) {
                        current_dependants.push(dependant_cell);
                    }
                });
            }
        }
    }

    fn collect_overlapping_vertex_indexes(&mut self, range: CellRange) {
        self.overlapping_vertex_indexes.clear();
        let envelope = vertex_envelope(range);

        let vertex_index = &self.vertex_index;
        let overlapping_vertex_indexes = &mut self.overlapping_vertex_indexes;
        for indexed_vertex in vertex_index.locate_in_envelope_intersecting(&envelope) {
            overlapping_vertex_indexes.push(indexed_vertex.vertex_index);
        }
    }

    fn intern_vertex(&mut self, range: CellRange) -> NodeIndex {
        if let Some(vertex_index) = self.vertex_lookup.get(&range) {
            return *vertex_index;
        }

        let vertex_index = self.graph.add_node(range);
        self.vertex_lookup.insert(range, vertex_index);
        self.vertex_index
            .insert(IndexedVertex::new(vertex_index, range));
        vertex_index
    }

    fn trim_vertex_if_unused(&mut self, vertex_index: NodeIndex) {
        if self.graph.node_weight(vertex_index).is_none() {
            return;
        }
        if self.graph.edges(vertex_index).next().is_some() {
            return;
        }

        let vertex_range = *self.graph.node_weight(vertex_index).unwrap();
        self.vertex_lookup.remove(&vertex_range);
        self.vertex_index
            .remove(&IndexedVertex::new(vertex_index, vertex_range));
        self.graph.remove_node(vertex_index);
    }
}

fn dependency_ranges_from_ast(ast: &[Expr], source_cell: &AbsoluteCellId) -> Vec<CellRange> {
    let mut dependency_ranges = Vec::new();

    for expr in ast {
        let Expr::Atom(ExprAtom::Reference(reference)) = expr else {
            continue;
        };
        dependency_ranges.push(reference.to_cell_range(source_cell));
    }

    dependency_ranges
}

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

fn dependant_axis(dependant_range: CellRange) -> Axis {
    if dependant_range.is_row_vector() {
        Axis::Row
    } else {
        Axis::Col
    }
}

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

fn sort_cells(cells: &mut Vec<AbsoluteCellId>) {
    cells.sort_unstable_by_key(|cell| (cell.sheet_id, cell.row, cell.col));
}

fn cell_requires_recalculation(sheets: &[Grid], cell_id: AbsoluteCellId) -> bool {
    let grid_cell_id: GridCellId = (&cell_id).into();
    sheets[cell_id.sheet_id as usize]
        .get_cell(&grid_cell_id)
        .and_then(|cell| cell.defined_by_formula)
        .is_some()
}

fn reset_pending_counter(sheets: &mut [Grid], cell_id: AbsoluteCellId) {
    let grid_cell_id: GridCellId = (&cell_id).into();
    sheets[cell_id.sheet_id as usize].reset_pending_dependencies(&grid_cell_id);
}

fn increase_pending_counter(sheets: &mut [Grid], cell_id: AbsoluteCellId) {
    let grid_cell_id: GridCellId = (&cell_id).into();
    sheets[cell_id.sheet_id as usize].increase_pending_dependencies(&grid_cell_id);
}

fn decrease_pending_counter(sheets: &mut [Grid], cell_id: AbsoluteCellId) {
    let grid_cell_id: GridCellId = (&cell_id).into();
    sheets[cell_id.sheet_id as usize].decrease_pending_dependencies(&grid_cell_id);
}

fn get_pending_counter(sheets: &[Grid], cell_id: AbsoluteCellId) -> u32 {
    let grid_cell_id: GridCellId = (&cell_id).into();
    sheets[cell_id.sheet_id as usize].get_pending_dependencies(&grid_cell_id)
}

fn merge_cells_into_line_ranges(cells: &[AbsoluteCellId]) -> Vec<CellRange> {
    if cells.is_empty() {
        return Vec::new();
    }

    let mut row_sorted_cells = cells.to_vec();
    row_sorted_cells.sort_unstable_by_key(|cell| (cell.sheet_id, cell.row, cell.col));
    let row_ranges = merge_sorted_cells_into_line_ranges(&row_sorted_cells, true);

    let mut col_sorted_cells = cells.to_vec();
    col_sorted_cells.sort_unstable_by_key(|cell| (cell.sheet_id, cell.col, cell.row));
    let col_ranges = merge_sorted_cells_into_line_ranges(&col_sorted_cells, false);

    if col_ranges.len() < row_ranges.len() {
        col_ranges
    } else {
        row_ranges
    }
}

fn merge_sorted_cells_into_line_ranges(
    sorted_cells: &[AbsoluteCellId],
    same_row_ranges: bool,
) -> Vec<CellRange> {
    let mut ranges = Vec::new();
    let mut range_start = sorted_cells[0];
    let mut range_end = sorted_cells[0];

    for position in 1..sorted_cells.len() {
        let current_cell = sorted_cells[position];
        let extends_current_range = if same_row_ranges {
            current_cell.sheet_id == range_end.sheet_id
                && current_cell.row == range_end.row
                && current_cell.col == range_end.col + 1
        } else {
            current_cell.sheet_id == range_end.sheet_id
                && current_cell.col == range_end.col
                && current_cell.row == range_end.row + 1
        };

        if extends_current_range {
            range_end = current_cell;
            continue;
        }

        ranges.push(CellRange::new(
            range_start.sheet_id,
            range_start.row,
            range_start.col,
            range_end.row,
            range_end.col,
        ));
        range_start = current_cell;
        range_end = current_cell;
    }

    ranges.push(CellRange::new(
        range_start.sheet_id,
        range_start.row,
        range_start.col,
        range_end.row,
        range_end.col,
    ));

    ranges
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn cells_from_graph(
        graph: &mut DependencyGraph,
        dependency_cell: AbsoluteCellId,
    ) -> Vec<AbsoluteCellId> {
        graph.collect_direct_dependants(dependency_cell);
        let mut cells = graph.current_dependants.clone();
        sort_cells(&mut cells);
        cells
    }

    fn sheets_with_cells(cells: &[AbsoluteCellId]) -> Vec<Grid> {
        let mut sheets = vec![Grid::default()];
        for cell_id in cells {
            let grid_cell_id: GridCellId = cell_id.into();
            sheets[0].insert_cell(
                &grid_cell_id,
                Cell {
                    defined_by_formula: None,
                    val: CellValue::Error(String::new()),
                    pending_dependencies: 0,
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
                    val: CellValue::Error(String::new()),
                    pending_dependencies: 0,
                },
            );
        }
        sheets
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

        let dependant_cells = cells_from_graph(&mut graph, cell(0, 2, 0));

        assert_eq!(
            dependant_cells,
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
        let first_wave = graph
            .init_pending_counter_for_new_recalculation(&mut sheets, &[cell(0, 0, 0)])
            .to_vec();
        assert_eq!(first_wave, vec![cell(0, 0, 2)]);
        assert_eq!(get_pending_counter(&sheets, cell(0, 0, 2)), 1);
        assert_eq!(get_pending_counter(&sheets, cell(0, 0, 4)), 2);

        let second_wave = graph
            .decrease_pending_counter(&mut sheets, cell(0, 0, 2))
            .to_vec();
        assert_eq!(second_wave, vec![cell(0, 0, 4)]);
    }

    #[test]
    fn init_pending_counter_counts_edges_between_changed_cells() {
        let mut graph = DependencyGraph::new();
        graph.insert_dependency_range(
            CellRange::single(cell(0, 0, 0)),
            CellRange::single(cell(0, 0, 2)),
        );

        let mut sheets = sheets_with_formula_cells(&[], &[(cell(0, 0, 0), 0), (cell(0, 0, 2), 1)]);
        let first_wave = graph
            .init_pending_counter_for_new_recalculation(
                &mut sheets,
                &[cell(0, 0, 2), cell(0, 0, 0)],
            )
            .to_vec();

        assert_eq!(first_wave, vec![cell(0, 0, 0)]);
        assert_eq!(get_pending_counter(&sheets, cell(0, 0, 2)), 2);
    }

    #[test]
    fn collect_unresolved_cells_finds_cycle() {
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
        let first_wave = graph
            .init_pending_counter_for_new_recalculation(&mut sheets, &[cell(0, 0, 0)])
            .to_vec();

        assert!(first_wave.is_empty());
        assert_eq!(
            graph.collect_unresolved_cells(&sheets).to_vec(),
            vec![cell(0, 0, 0), cell(0, 0, 1)]
        );
    }

    #[test]
    fn deleted_changed_cell_is_still_in_first_wave() {
        let mut graph = DependencyGraph::new();
        graph.insert_dependency_range(
            CellRange::single(cell(0, 0, 0)),
            CellRange::single(cell(0, 0, 2)),
        );

        // the changed dependency cell is deleted, so it is not stored in the grid
        let mut sheets = sheets_with_formula_cells(&[], &[(cell(0, 0, 2), 0)]);
        let first_wave = graph
            .init_pending_counter_for_new_recalculation(&mut sheets, &[cell(0, 0, 0)])
            .to_vec();

        assert_eq!(first_wave, vec![cell(0, 0, 2)]);
    }

    #[test]
    fn merge_cells_into_line_ranges_prefers_fewer_ranges() {
        let merged_ranges =
            merge_cells_into_line_ranges(&[cell(0, 0, 0), cell(0, 1, 0), cell(0, 2, 0)]);

        assert_eq!(merged_ranges, vec![range(0, 0, 0, 2, 0)]);
    }
}
