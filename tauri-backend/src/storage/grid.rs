use std::fmt;

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::storage::types::FormulaId;

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct GridCellId {
    pub row: u32,
    pub col: u32,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Cell {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub defined_by_formula: Option<FormulaId>,
    pub val: CellValue,
    #[serde(skip)]
    pub pending_dependencies: u32,
}

impl Cell {
    pub fn text(s: String) -> Self {
        Self {
            defined_by_formula: None,
            val: CellValue::Text(s),
            pending_dependencies: 0,
        }
    }

    pub fn number(n: Decimal) -> Self {
        Self {
            defined_by_formula: None,
            val: CellValue::Number(n),
            pending_dependencies: 0,
        }
    }

    pub fn error(msg: String) -> Self {
        Self {
            defined_by_formula: None,
            val: CellValue::Error(msg),
            pending_dependencies: 0,
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum CellValue {
    Text(String),
    Number(Decimal),
    Error(String),
}

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
    nonempty_cells_count: u32,
}

impl Block {
    fn new() -> Self {
        Self {
            cells: Box::new(std::array::from_fn(|_| std::array::from_fn(|_| None))),
            nonempty_cells_count: 0,
        }
    }

    fn should_deallocate(&self) -> bool {
        self.nonempty_cells_count == 0
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
    pub fn find_biggest_row(&self) -> u32 {
        let mut biggest_row = 0;
        for (block_idx, block) in self.blocks.iter().enumerate() {
            let Some(block) = block.as_ref() else {
                continue;
            };
            let block_row = block_idx / self.stride;
            for row in (0..32).rev() {
                if block.cells[row].iter().any(|cell| cell.is_some()) {
                    biggest_row = biggest_row.max(block_row as u32 * 32 + row as u32);
                    break;
                }
            }
        }
        biggest_row
    }

    pub fn find_biggest_column(&self) -> u32 {
        let mut biggest_col = 0;
        for (block_idx, block) in self.blocks.iter().enumerate() {
            let Some(block) = block.as_ref() else {
                continue;
            };
            let block_col = block_idx % self.stride;
            for col in (0..32).rev() {
                if (0..32).any(|row| block.cells[row][col].is_some()) {
                    biggest_col = biggest_col.max(block_col as u32 * 32 + col as u32);
                    break;
                }
            }
        }
        biggest_col
    }

    pub fn get_cell(&self, id: &GridCellId) -> Option<&Cell> {
        let idx = id.block_idx(self.stride);
        let block = self.blocks.get(idx)?.as_ref()?;
        let (r, c) = id.local();
        block.cells[r][c].as_ref()
    }

    pub fn get_value(&self, id: &GridCellId) -> Option<&CellValue> {
        self.get_cell(id).map(|cell| &cell.val)
    }

    pub fn set_value(&mut self, id: &GridCellId, val: CellValue) {
        let idx = id.block_idx(self.stride);
        let block = self.blocks[idx].get_or_insert_with(Block::new);
        let (r, c) = id.local();
        match block.cells[r][c].as_mut() {
            Some(cell) => cell.val = val,
            None => {
                block.cells[r][c] = Some(Cell {
                    defined_by_formula: None,
                    val,
                    pending_dependencies: 0,
                });
                block.nonempty_cells_count += 1;
            }
        }
    }

    pub fn insert_cell(&mut self, id: &GridCellId, cell: Cell) {
        let idx = id.block_idx(self.stride);
        let block = self.blocks[idx].get_or_insert_with(Block::new);
        let (r, c) = id.local();
        if block.cells[r][c].is_none() {
            block.nonempty_cells_count += 1;
        }
        block.cells[r][c] = Some(cell);
    }

    pub fn remove_cell(&mut self, id: &GridCellId) {
        let idx = id.block_idx(self.stride);
        let Some(block) = self.blocks[idx].as_mut() else {
            return;
        };
        let (r, c) = id.local();
        if block.cells[r][c].take().is_some() {
            block.nonempty_cells_count -= 1;
        }
        if block.should_deallocate() {
            self.blocks[idx] = None;
        }
    }

    pub fn increase_pending_dependencies(&mut self, id: &GridCellId) {
        let idx = id.block_idx(self.stride);
        let Some(block) = self.blocks[idx].as_mut() else {
            return;
        };
        let (r, c) = id.local();
        if let Some(cell) = block.cells[r][c].as_mut() {
            cell.pending_dependencies += 1;
        }
    }

    pub fn decrease_pending_dependencies(&mut self, id: &GridCellId) {
        let idx = id.block_idx(self.stride);
        let Some(block) = self.blocks[idx].as_mut() else {
            return;
        };
        let (r, c) = id.local();
        if let Some(cell) = block.cells[r][c].as_mut() {
            cell.pending_dependencies -= 1;
        }
    }

    pub fn get_pending_dependencies(&self, id: &GridCellId) -> u32 {
        self.get_cell(id)
            .map_or(0, |cell| cell.pending_dependencies)
    }

    pub fn reset_pending_dependencies(&mut self, id: &GridCellId) {
        let idx = id.block_idx(self.stride);
        let Some(block) = self.blocks[idx].as_mut() else {
            return;
        };
        let (r, c) = id.local();
        if let Some(cell) = block.cells[r][c].as_mut() {
            cell.pending_dependencies = 0;
        }
    }

    pub fn for_each_cell<F>(&self, mut f: F)
    where
        F: FnMut(GridCellId, &Cell),
    {
        for (block_idx, block) in self.blocks.iter().enumerate() {
            let Some(block) = block.as_ref() else {
                continue;
            };
            let block_row = block_idx / self.stride;
            let block_col = block_idx % self.stride;
            for row in 0..32 {
                for col in 0..32 {
                    let Some(cell) = block.cells[row][col].as_ref() else {
                        continue;
                    };
                    f(
                        GridCellId {
                            row: (block_row as u32 * 32) + row as u32,
                            col: (block_col as u32 * 32) + col as u32,
                        },
                        cell,
                    );
                }
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
            defined_by_formula: None,
            val,
            pending_dependencies: 0,
        }
    }

    #[test]
    fn insert_and_get() {
        let mut grid = Grid::default();
        grid.insert_cell(&id(0, 0), cell(CellValue::Text("hello".into())));
        grid.insert_cell(&id(100, 200), cell(CellValue::Number(Decimal::from(42))));

        assert!(
            matches!(grid.get_cell(&id(0, 0)).map(|c| &c.val), Some(CellValue::Text(s)) if s == "hello")
        );
        assert!(matches!(
            grid.get_cell(&id(100, 200)).map(|c| &c.val),
            Some(CellValue::Number(_))
        ));
    }

    #[test]
    fn get_empty_returns_none() {
        let grid = Grid::default();
        assert!(grid.get_cell(&id(0, 0)).is_none());
        assert!(grid.get_cell(&id(500, 500)).is_none());
    }

    #[test]
    fn remove_cell() {
        let mut grid = Grid::default();
        grid.insert_cell(&id(3, 3), cell(CellValue::Error(String::new())));
        assert!(grid.get_cell(&id(3, 3)).is_some());
        grid.remove_cell(&id(3, 3));
        assert!(grid.get_cell(&id(3, 3)).is_none());
    }

    #[test]
    fn remove_frees_empty_block() {
        let mut grid = Grid::default();
        grid.insert_cell(&id(0, 0), cell(CellValue::Error(String::new())));
        let idx = id(0, 0).block_idx(grid.stride);
        assert!(grid.blocks[idx].is_some());
        grid.remove_cell(&id(0, 0));
        assert!(grid.blocks[idx].is_none());
    }

    #[test]
    fn block_not_freed_while_cells_remain() {
        let mut grid = Grid::default();
        grid.insert_cell(&id(0, 0), cell(CellValue::Error(String::new())));
        grid.insert_cell(&id(1, 1), cell(CellValue::Error(String::new())));
        grid.remove_cell(&id(0, 0));
        let idx = id(0, 0).block_idx(grid.stride);
        assert!(grid.blocks[idx].is_some());
    }

    #[test]
    fn insert_overwrites_existing() {
        let mut grid = Grid::default();
        grid.insert_cell(&id(0, 0), cell(CellValue::Text("first".into())));
        grid.insert_cell(&id(0, 0), cell(CellValue::Text("second".into())));
        assert!(
            matches!(grid.get_cell(&id(0, 0)).map(|c| &c.val), Some(CellValue::Text(s)) if s == "second")
        );
        let idx = id(0, 0).block_idx(grid.stride);
        assert_eq!(grid.blocks[idx].as_ref().unwrap().nonempty_cells_count, 1);
    }

    #[test]
    fn cells_across_block_boundaries() {
        let mut grid = Grid::default();
        grid.insert_cell(&id(31, 31), cell(CellValue::Text("a".into())));
        grid.insert_cell(&id(32, 32), cell(CellValue::Text("b".into())));
        let idx_a = id(31, 31).block_idx(grid.stride);
        let idx_b = id(32, 32).block_idx(grid.stride);
        assert_ne!(idx_a, idx_b);
        assert!(
            matches!(grid.get_cell(&id(31, 31)).map(|c| &c.val), Some(CellValue::Text(s)) if s == "a")
        );
        assert!(
            matches!(grid.get_cell(&id(32, 32)).map(|c| &c.val), Some(CellValue::Text(s)) if s == "b")
        );
    }

    #[test]
    fn set_value_preserves_formula_metadata() {
        let mut grid = Grid::default();
        grid.insert_cell(
            &id(0, 0),
            Cell {
                defined_by_formula: Some(7),
                val: CellValue::Text("old".into()),
                pending_dependencies: 3,
            },
        );

        grid.set_value(&id(0, 0), CellValue::Text("new".into()));

        let cell = grid.get_cell(&id(0, 0)).unwrap();
        assert_eq!(cell.defined_by_formula, Some(7));
        assert_eq!(cell.pending_dependencies, 3);
        assert!(matches!(&cell.val, CellValue::Text(s) if s == "new"));
    }
}
