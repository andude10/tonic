use fastnum::D256;
use serde::{Deserialize, Serialize};

use crate::storage::types::{AbsoluteCellId, FormulaId};

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct GridCellId {
    pub row: u32,
    pub col: u32,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Cell {
    dependents: Option<Vec<AbsoluteCellId>>,
    defined_by_formula: Option<FormulaId>,
    val: CellValue,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum CellValue {
    Text(String),
    Number(D256),
    Error,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
struct Block {
    cells: Box<[[Option<Cell>; 32]; 32]>,
    nonempty_cell_count: u32,
}

impl Block {
    fn new() -> Self {
        Self {
            cells: Box::new(std::array::from_fn(|_| std::array::from_fn(|_| None))),
            nonempty_cell_count: 0,
        }
    }
}

/// Sparse infinite grid backed by 32×32 blocks.
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

impl Grid {
    /// Creates a new sparse grid that can hold cells in the range
    /// `[0..max_rows, 0..max_cols]`.
    pub fn new(max_rows: u32, max_cols: u32) -> Self {
        let block_rows = ((max_rows as usize + 31) >> 5) as usize;
        let block_cols = ((max_cols as usize + 31) >> 5) as usize;
        let total = block_rows * block_cols;
        let mut blocks = Vec::with_capacity(total);
        blocks.resize_with(total, || None);
        Self {
            blocks,
            stride: block_cols,
        }
    }

    /// Returns a reference to the cell, or `None` if the cell is empty.
    /// Panics if `id` exceeds the grid dimensions passed to `new()`.
    pub fn get(&self, id: &GridCellId) -> Option<&Cell> {
        let block = self.blocks[id.block_idx(self.stride)].as_ref()?;
        let (r, c) = id.local();
        block.cells[r][c].as_ref()
    }

    /// Returns a mutable reference to the cell, or `None` if the cell is empty.
    /// Panics if `id` exceeds the grid dimensions passed to `new()`.
    pub fn get_mut(&mut self, id: &GridCellId) -> Option<&mut Cell> {
        let block = self.blocks[id.block_idx(self.stride)].as_mut()?;
        let (r, c) = id.local();
        block.cells[r][c].as_mut()
    }

    /// Inserts a cell value at the given position. Allocates the block of a cell if it doesn't exist.
    /// Panics if `id` exceeds the grid dimensions passed to `new()`.
    pub fn insert(&mut self, id: &GridCellId, value: Cell) {
        let idx = id.block_idx(self.stride);
        let block = self.blocks[idx].get_or_insert_with(Block::new);
        let (r, c) = id.local();
        if block.cells[r][c].is_none() {
            block.nonempty_cell_count += 1;
        }
        block.cells[r][c] = Some(value);
    }

    /// Removes the cell at the given position. Frees the block if it becomes entirely empty after removal.
    /// Panics if `id` exceeds the grid dimensions passed to `new()`.
    pub fn remove(&mut self, id: &GridCellId) {
        let idx = id.block_idx(self.stride);
        let Some(block) = self.blocks[idx].as_mut() else {
            return;
        };
        let (r, c) = id.local();
        if block.cells[r][c].take().is_some() {
            block.nonempty_cell_count -= 1;
            if block.nonempty_cell_count == 0 {
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

    fn cell(val: CellValue) -> Cell {
        Cell {
            dependents: None,
            val,
            defined_by_formula: None,
        }
    }

    #[test]
    fn insert_and_get() {
        let mut grid = Grid::new(1024, 1024);
        grid.insert(&id(0, 0), cell(CellValue::Text("hello".into())));
        grid.insert(&id(100, 200), cell(CellValue::Number(D256::from(42))));

        assert!(
            matches!(grid.get(&id(0, 0)).map(|c| &c.val), Some(CellValue::Text(s)) if s == "hello")
        );
        assert!(matches!(
            grid.get(&id(100, 200)).map(|c| &c.val),
            Some(CellValue::Number(_))
        ));
    }

    #[test]
    fn get_empty_returns_none() {
        let grid = Grid::new(1024, 1024);
        assert!(grid.get(&id(0, 0)).is_none());
        assert!(grid.get(&id(500, 500)).is_none());
    }

    #[test]
    fn get_mut_modifies_cell() {
        let mut grid = Grid::new(1024, 1024);
        grid.insert(&id(5, 5), cell(CellValue::Text("old".into())));
        if let Some(c) = grid.get_mut(&id(5, 5)) {
            c.val = CellValue::Text("new".into());
        }
        assert!(
            matches!(grid.get(&id(5, 5)).map(|c| &c.val), Some(CellValue::Text(s)) if s == "new")
        );
    }

    #[test]
    fn remove_cell() {
        let mut grid = Grid::new(1024, 1024);
        grid.insert(&id(3, 3), cell(CellValue::Error));
        assert!(grid.get(&id(3, 3)).is_some());
        grid.remove(&id(3, 3));
        assert!(grid.get(&id(3, 3)).is_none());
    }

    #[test]
    fn remove_frees_empty_block() {
        let mut grid = Grid::new(1024, 1024);
        grid.insert(&id(0, 0), cell(CellValue::Error));
        let idx = id(0, 0).block_idx(grid.stride);
        assert!(grid.blocks[idx].is_some());
        grid.remove(&id(0, 0));
        assert!(grid.blocks[idx].is_none());
    }

    #[test]
    fn block_not_freed_while_cells_remain() {
        let mut grid = Grid::new(1024, 1024);
        grid.insert(&id(0, 0), cell(CellValue::Error));
        grid.insert(&id(1, 1), cell(CellValue::Error));
        grid.remove(&id(0, 0));
        let idx = id(0, 0).block_idx(grid.stride);
        assert!(grid.blocks[idx].is_some());
    }

    #[test]
    fn insert_overwrites_existing() {
        let mut grid = Grid::new(1024, 1024);
        grid.insert(&id(0, 0), cell(CellValue::Text("first".into())));
        grid.insert(&id(0, 0), cell(CellValue::Text("second".into())));
        assert!(
            matches!(grid.get(&id(0, 0)).map(|c| &c.val), Some(CellValue::Text(s)) if s == "second")
        );
        let idx = id(0, 0).block_idx(grid.stride);
        assert_eq!(grid.blocks[idx].as_ref().unwrap().nonempty_cell_count, 1);
    }

    #[test]
    fn cells_across_block_boundaries() {
        let mut grid = Grid::new(1024, 1024);
        grid.insert(&id(31, 31), cell(CellValue::Text("a".into())));
        grid.insert(&id(32, 32), cell(CellValue::Text("b".into())));
        let idx_a = id(31, 31).block_idx(grid.stride);
        let idx_b = id(32, 32).block_idx(grid.stride);
        assert_ne!(idx_a, idx_b);
        assert!(
            matches!(grid.get(&id(31, 31)).map(|c| &c.val), Some(CellValue::Text(s)) if s == "a")
        );
        assert!(
            matches!(grid.get(&id(32, 32)).map(|c| &c.val), Some(CellValue::Text(s)) if s == "b")
        );
    }
}
