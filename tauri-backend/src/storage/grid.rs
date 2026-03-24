use std::fmt;

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use tauri_plugin_log::log::debug;

use crate::storage::types::{AbsoluteCellId, FormulaId};

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq)]
pub struct GridCellId {
    pub row: u32,
    pub col: u32,
}

/// Content of a cell (value + formula info). Separate from dependents tracking.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct CellContent {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub defined_by_formula: Option<FormulaId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dependencies: Option<Vec<AbsoluteCellId>>,
    pub val: CellValue,
    #[serde(skip)]
    pub pending_dependencies: u32,
}

impl CellContent {
    pub fn text(s: String) -> Self {
        Self {
            defined_by_formula: None,
            dependencies: None,
            val: CellValue::Text(s),
            pending_dependencies: 0,
        }
    }

    pub fn number(n: Decimal) -> Self {
        Self {
            defined_by_formula: None,
            dependencies: None,
            val: CellValue::Number(n),
            pending_dependencies: 0,
        }
    }

    pub fn error(msg: String) -> Self {
        Self {
            defined_by_formula: None,
            dependencies: None,
            val: CellValue::Error(msg),
            pending_dependencies: 0,
        }
    }
}

// todo: move out dependents and dependencies outside of cell

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Cell {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) dependents: Option<Vec<AbsoluteCellId>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) content: Option<CellContent>,
}

impl Cell {
    fn new() -> Self {
        Self {
            dependents: None,
            content: None,
        }
    }

    fn is_empty(&self) -> bool {
        self.dependents.is_none() && self.content.is_none()
    }

    fn has_dependents(&self) -> bool {
        self.dependents.as_ref().map_or(false, |d| !d.is_empty())
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum CellValue {
    Text(String),
    Number(Decimal),
    Error(String),
}

// todo: remove
impl fmt::Display for CellValue {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            CellValue::Text(s) => write!(f, "{}", s),
            CellValue::Number(n) => write!(f, "{}", n),
            CellValue::Error(s) => write!(f, "#ERROR: {}", s),
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
struct Block {
    cells: Box<[[Option<Cell>; 32]; 32]>,
    nonempty_values_count: u32,
    dependants_count: u32,
}

impl Block {
    fn new() -> Self {
        Self {
            cells: Box::new(std::array::from_fn(|_| std::array::from_fn(|_| None))),
            nonempty_values_count: 0,
            dependants_count: 0,
        }
    }

    fn should_deallocate(&self) -> bool {
        self.nonempty_values_count == 0 && self.dependants_count == 0
    }
}

/// Sparse infinite grid backed by 32×32 blocks.
#[derive(Serialize, Deserialize)]
pub struct Grid {
    blocks: Vec<Option<Block>>,
    stride: usize,
}

impl GridCellId {
    fn block_idx(&self, stride: usize) -> usize {
        (self.row as usize >> 5) * stride + (self.col as usize >> 5)
    }
    fn local(&self) -> (usize, usize) {
        (self.row as usize & 31, self.col as usize & 31)
    }
}

impl Default for Grid {
    fn default() -> Self {
        // todo: show errors to user when exceeding max size
        // todo: allow setting max cols/rows?

        // stride=22 block-columns -> 22*32 = 704 columns (covers A–ZZ = 702)
        // 1_375_000 / 22 = 62_500 row-blocks -> 62_500*32 = 2_000_000 rows
        const STRIDE: usize = 22;
        const MAX_BLOCKS: usize = 62_500 * STRIDE;
        let mut blocks = Vec::with_capacity(MAX_BLOCKS);
        blocks.resize_with(MAX_BLOCKS, || None);
        Self {
            blocks,
            stride: STRIDE,
        }
    }
}

impl Grid {
    /// Returns a reference to the cell content, or `None` if the cell has no content.
    pub fn get_content(&self, id: &GridCellId) -> Option<&CellContent> {
        let idx = id.block_idx(self.stride);
        let block = self.blocks.get(idx)?.as_ref()?;
        let (r, c) = id.local();
        block.cells[r][c].as_ref()?.content.as_ref()
    }

    /// Returns the cell's value, or `None` if the cell has no content.
    pub fn get_value(&self, id: &GridCellId) -> Option<&CellValue> {
        self.get_content(id).map(|c| &c.val)
    }

    /// Sets the value of a cell, creating it if it doesn't exist.
    pub fn set_value(&mut self, id: &GridCellId, val: CellValue) {
        let idx = id.block_idx(self.stride);
        let block = self.blocks[idx].get_or_insert_with(Block::new);
        let (r, c) = id.local();
        let cell = block.cells[r][c].get_or_insert_with(Cell::new);
        match cell.content.as_mut() {
            Some(content) => content.val = val,
            None => {
                cell.content = Some(CellContent {
                    val,
                    defined_by_formula: None,
                    dependencies: None,
                    pending_dependencies: 0,
                });
                block.nonempty_values_count += 1;
            }
        }
    }

    /// Returns true if the cell has any dependents.
    pub fn has_dependants(&self, id: &GridCellId) -> bool {
        let Some(block) = self.blocks[id.block_idx(self.stride)].as_ref() else {
            return false;
        };
        let (r, c) = id.local();
        block.cells[r][c]
            .as_ref()
            .map_or(false, |c| c.has_dependents())
    }

    /// Inserts or updates cell content, preserving dependents.
    pub fn insert_content(&mut self, id: &GridCellId, content: CellContent) {
        let idx = id.block_idx(self.stride);
        let block = self.blocks[idx].get_or_insert_with(Block::new);
        let (r, c) = id.local();
        let cell = block.cells[r][c].get_or_insert_with(Cell::new);
        if cell.content.is_none() {
            block.nonempty_values_count += 1;
        }
        cell.content = Some(content);
    }

    /// Removes cell content, preserving dependents. Deallocates block if empty.
    pub fn remove_content(&mut self, id: &GridCellId) {
        let idx = id.block_idx(self.stride);
        let Some(block) = self.blocks[idx].as_mut() else {
            return;
        };
        let (r, c) = id.local();
        let Some(cell) = block.cells[r][c].as_mut() else {
            return;
        };
        if cell.content.take().is_some() {
            block.nonempty_values_count -= 1;
        }
        if cell.is_empty() {
            block.cells[r][c] = None;
            if block.should_deallocate() {
                self.blocks[idx] = None;
            }
        }
    }

    /// Adds a dependant to a cell. Allocates block if needed.
    pub fn add_dependant(&mut self, id: &GridCellId, dependant: &AbsoluteCellId) {
        let idx = id.block_idx(self.stride);
        let block = self.blocks[idx].get_or_insert_with(Block::new);
        let (r, c) = id.local();
        let cell = block.cells[r][c].get_or_insert_with(Cell::new);
        let deps = cell.dependents.get_or_insert_with(Vec::new);
        let had_dependents = !deps.is_empty();
        deps.push(dependant.clone());
        if !had_dependents {
            block.dependants_count += 1;
        }
    }

    /// Returns a reference to the cell's dependents list, or `None`.
    pub fn get_dependents(&self, id: &GridCellId) -> Option<&Vec<AbsoluteCellId>> {
        let block = self.blocks[id.block_idx(self.stride)].as_ref()?;
        let (r, c) = id.local();
        block.cells[r][c].as_ref()?.dependents.as_ref()
    }

    // todo: probably remove increase_pending_dependency_count, decrease_pending_dependency_count
    // and get_pending_dependency_count with something shorter

    pub fn increase_pending_dependencies(&mut self, id: &GridCellId) {
        let idx = id.block_idx(self.stride);
        let Some(block) = self.blocks[idx].as_mut() else {
            return;
        };
        let (r, c) = id.local();
        if let Some(cell) = block.cells[r][c].as_mut() {
            if let Some(content) = cell.content.as_mut() {
                content.pending_dependencies += 1;
            }
        }
    }

    pub fn decrease_pending_dependencies(&mut self, id: &GridCellId) {
        let idx = id.block_idx(self.stride);
        let Some(block) = self.blocks[idx].as_mut() else {
            return;
        };
        let (r, c) = id.local();
        if let Some(cell) = block.cells[r][c].as_mut() {
            if let Some(content) = cell.content.as_mut() {
                content.pending_dependencies -= 1;
            }
        }
    }

    pub fn get_pending_dependencies(&self, id: &GridCellId) -> u32 {
        let Some(block) = self.blocks[id.block_idx(self.stride)].as_ref() else {
            return 0;
        };
        let (r, c) = id.local();
        block.cells[r][c]
            .as_ref()
            .and_then(|cell| cell.content.as_ref())
            .map_or(0, |c| c.pending_dependencies)
    }

    /// Removes a dependant from a cell. Deallocates block if empty.
    pub fn remove_dependant(&mut self, id: &GridCellId, dependant: &AbsoluteCellId) {
        let idx = id.block_idx(self.stride);
        let Some(block) = self.blocks[idx].as_mut() else {
            return;
        };
        let (r, c) = id.local();
        let Some(cell) = block.cells[r][c].as_mut() else {
            return;
        };
        if let Some(deps) = cell.dependents.as_mut() {
            deps.retain(|d| d != dependant);
            if deps.is_empty() {
                cell.dependents = None;
                block.dependants_count -= 1;
            }
        }
        if cell.is_empty() {
            block.cells[r][c] = None;
            if block.should_deallocate() {
                self.blocks[idx] = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(row: u32, col: u32) -> GridCellId {
        GridCellId { row, col }
    }

    fn content(val: CellValue) -> CellContent {
        CellContent {
            defined_by_formula: None,
            dependencies: None,
            val,
            pending_dependencies: 0,
        }
    }

    #[test]
    fn insert_and_get() {
        let mut grid = Grid::default();
        grid.insert_content(&id(0, 0), content(CellValue::Text("hello".into())));
        grid.insert_content(&id(100, 200), content(CellValue::Number(Decimal::from(42))));

        assert!(
            matches!(grid.get_content(&id(0, 0)).map(|c| &c.val), Some(CellValue::Text(s)) if s == "hello")
        );
        assert!(matches!(
            grid.get_content(&id(100, 200)).map(|c| &c.val),
            Some(CellValue::Number(_))
        ));
    }

    #[test]
    fn get_empty_returns_none() {
        let grid = Grid::default();
        assert!(grid.get_content(&id(0, 0)).is_none());
        assert!(grid.get_content(&id(500, 500)).is_none());
    }

    #[test]
    fn remove_cell() {
        let mut grid = Grid::default();
        grid.insert_content(&id(3, 3), content(CellValue::Error(String::new())));
        assert!(grid.get_content(&id(3, 3)).is_some());
        grid.remove_content(&id(3, 3));
        assert!(grid.get_content(&id(3, 3)).is_none());
    }

    #[test]
    fn remove_frees_empty_block() {
        let mut grid = Grid::default();
        grid.insert_content(&id(0, 0), content(CellValue::Error(String::new())));
        let idx = id(0, 0).block_idx(grid.stride);
        assert!(grid.blocks[idx].is_some());
        grid.remove_content(&id(0, 0));
        assert!(grid.blocks[idx].is_none());
    }

    #[test]
    fn block_not_freed_while_cells_remain() {
        let mut grid = Grid::default();
        grid.insert_content(&id(0, 0), content(CellValue::Error(String::new())));
        grid.insert_content(&id(1, 1), content(CellValue::Error(String::new())));
        grid.remove_content(&id(0, 0));
        let idx = id(0, 0).block_idx(grid.stride);
        assert!(grid.blocks[idx].is_some());
    }

    #[test]
    fn insert_overwrites_existing() {
        let mut grid = Grid::default();
        grid.insert_content(&id(0, 0), content(CellValue::Text("first".into())));
        grid.insert_content(&id(0, 0), content(CellValue::Text("second".into())));
        assert!(
            matches!(grid.get_content(&id(0, 0)).map(|c| &c.val), Some(CellValue::Text(s)) if s == "second")
        );
        let idx = id(0, 0).block_idx(grid.stride);
        assert_eq!(grid.blocks[idx].as_ref().unwrap().nonempty_values_count, 1);
    }

    #[test]
    fn cells_across_block_boundaries() {
        let mut grid = Grid::default();
        grid.insert_content(&id(31, 31), content(CellValue::Text("a".into())));
        grid.insert_content(&id(32, 32), content(CellValue::Text("b".into())));
        let idx_a = id(31, 31).block_idx(grid.stride);
        let idx_b = id(32, 32).block_idx(grid.stride);
        assert_ne!(idx_a, idx_b);
        assert!(
            matches!(grid.get_content(&id(31, 31)).map(|c| &c.val), Some(CellValue::Text(s)) if s == "a")
        );
        assert!(
            matches!(grid.get_content(&id(32, 32)).map(|c| &c.val), Some(CellValue::Text(s)) if s == "b")
        );
    }

    #[test]
    fn block_not_freed_while_dependants_exist() {
        let mut grid = Grid::default();
        let dep = AbsoluteCellId {
            sheet_id: 0,
            row: 5,
            col: 5,
        };
        grid.add_dependant(&id(0, 0), &dep);
        let idx = id(0, 0).block_idx(grid.stride);
        assert!(grid.blocks[idx].is_some());
        // Cell has no content but has dependant
        assert!(grid.get_content(&id(0, 0)).is_none());
        assert!(grid.has_dependants(&id(0, 0)));
        // Remove dependant should free block
        grid.remove_dependant(&id(0, 0), &dep);
        assert!(grid.blocks[idx].is_none());
    }

    #[test]
    fn dependants_preserved_on_value_update() {
        let mut grid = Grid::default();
        let dep = AbsoluteCellId {
            sheet_id: 0,
            row: 5,
            col: 5,
        };
        grid.insert_content(&id(0, 0), content(CellValue::Text("old".into())));
        grid.add_dependant(&id(0, 0), &dep);
        grid.insert_content(&id(0, 0), content(CellValue::Text("new".into())));
        // Dependant should still be there
        assert!(grid.has_dependants(&id(0, 0)));
    }
}
