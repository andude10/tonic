use std::fmt;
use std::sync::atomic::{AtomicU32, Ordering};

use cold_string::ColdString;
use parking_lot::RwLock;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::storage::types::FormulaId;

const BLOCK_SHIFT: usize = 4;
const BLOCK_DIM: usize = 1 << BLOCK_SHIFT; // 16
const BLOCK_MASK: usize = BLOCK_DIM - 1; // 0xF

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct GridCellId {
    pub row: u32,
    pub col: u32,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Cell {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub defined_by_formula: Option<FormulaId>,
    pub val: CellValue,
    // atomic so parallel eval can decrement without write-locking the cell
    #[serde(skip)]
    pub pending_dependencies: AtomicU32,
}

impl Clone for Cell {
    fn clone(&self) -> Self {
        Self {
            defined_by_formula: self.defined_by_formula,
            val: self.val.clone(),
            pending_dependencies: AtomicU32::new(self.pending_dependencies.load(Ordering::Relaxed)),
        }
    }
}

impl PartialEq for Cell {
    fn eq(&self, other: &Self) -> bool {
        self.defined_by_formula == other.defined_by_formula && self.val == other.val
    }
}

impl Cell {
    pub fn text(s: String) -> Self {
        Self {
            defined_by_formula: None,
            val: CellValue::Text(s.into()),
            pending_dependencies: AtomicU32::new(0),
        }
    }

    pub fn number(n: Decimal) -> Self {
        Self {
            defined_by_formula: None,
            val: CellValue::Number(n),
            pending_dependencies: AtomicU32::new(0),
        }
    }

    pub fn error(msg: String) -> Self {
        Self {
            defined_by_formula: None,
            val: CellValue::Error(msg.into()),
            pending_dependencies: AtomicU32::new(0),
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum CellValue {
    Text(ColdString),
    Number(Decimal),
    Error(ColdString),
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

// per-cell RwLock: parallel eval tasks can read/write individual cells without
// locking the entire block. the lock satisfies rust's aliasing rules —
// the TACO topological ordering guarantees no actual read/write races.
type CellSlot = RwLock<Option<Cell>>;

// custom serde: serialize the inner Option<Cell>, skip the RwLock wrapper
mod cell_slot_serde {
    use super::*;
    use serde::ser::SerializeSeq;

    pub fn serialize<S: serde::Serializer>(
        cells: &Box<[[CellSlot; BLOCK_DIM]; BLOCK_DIM]>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        let mut seq = serializer.serialize_seq(Some(BLOCK_DIM))?;
        for row in cells.iter() {
            let row_data: Vec<Option<Cell>> = row.iter().map(|slot| slot.read().clone()).collect();
            seq.serialize_element(&row_data)?;
        }
        seq.end()
    }

    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Box<[[CellSlot; BLOCK_DIM]; BLOCK_DIM]>, D::Error> {
        let rows: Vec<Vec<Option<Cell>>> = serde::Deserialize::deserialize(deserializer)?;
        let mut cells: Box<[[CellSlot; BLOCK_DIM]; BLOCK_DIM]> =
            Box::new(std::array::from_fn(|_| {
                std::array::from_fn(|_| RwLock::new(None))
            }));
        for (r, row) in rows.into_iter().enumerate().take(BLOCK_DIM) {
            for (c, cell) in row.into_iter().enumerate().take(BLOCK_DIM) {
                *cells[r][c].get_mut() = cell;
            }
        }
        Ok(cells)
    }
}

#[derive(Serialize, Deserialize)]
struct Block {
    #[serde(with = "cell_slot_serde")]
    cells: Box<[[CellSlot; BLOCK_DIM]; BLOCK_DIM]>,
    nonempty_cells_count: u32,
}

impl Clone for Block {
    fn clone(&self) -> Self {
        let cells = Box::new(std::array::from_fn(|r| {
            std::array::from_fn(|c| RwLock::new(self.cells[r][c].read().clone()))
        }));
        Self {
            cells,
            nonempty_cells_count: self.nonempty_cells_count,
        }
    }
}

impl PartialEq for Block {
    fn eq(&self, other: &Self) -> bool {
        if self.nonempty_cells_count != other.nonempty_cells_count {
            return false;
        }
        for r in 0..BLOCK_DIM {
            for c in 0..BLOCK_DIM {
                if *self.cells[r][c].read() != *other.cells[r][c].read() {
                    return false;
                }
            }
        }
        true
    }
}

impl Block {
    fn new() -> Self {
        Self {
            cells: Box::new(std::array::from_fn(|_| {
                std::array::from_fn(|_| RwLock::new(None))
            })),
            nonempty_cells_count: 0,
        }
    }

    fn should_deallocate(&self) -> bool {
        self.nonempty_cells_count == 0
    }
}

/// Sparse infinite grid backed by BLOCK_DIM×BLOCK_DIM blocks.
#[derive(Serialize, Deserialize)]
pub struct Grid {
    blocks: Vec<Option<Block>>,
    stride: usize,
    #[serde(default)]
    max_row: u32,
    #[serde(default)]
    max_col: u32,
}

impl GridCellId {
    fn block_idx(&self, stride: usize) -> usize {
        (self.row as usize >> BLOCK_SHIFT) * stride + (self.col as usize >> BLOCK_SHIFT)
    }

    fn local(&self) -> (usize, usize) {
        (
            self.row as usize & BLOCK_MASK,
            self.col as usize & BLOCK_MASK,
        )
    }
}

impl Default for Grid {
    fn default() -> Self {
        // todo: show errors to user when exceeding max size
        // todo: allow setting max cols/rows?

        // stride=44 block-columns -> 44*16 = 704 columns (covers A–ZZ = 702)
        // 1_250_000 row-blocks -> 1_250_000*16 = 20_000_000 rows
        const STRIDE: usize = 704 / BLOCK_DIM;
        const MAX_ROW_BLOCKS: usize = 20_000_000 / BLOCK_DIM;
        const MAX_BLOCKS: usize = MAX_ROW_BLOCKS * STRIDE;
        let mut blocks = Vec::with_capacity(MAX_BLOCKS);
        blocks.resize_with(MAX_BLOCKS, || None);
        Self {
            blocks,
            stride: STRIDE,
            max_row: 0,
            max_col: 0,
        }
    }
}

impl Grid {
    pub fn find_biggest_row(&self) -> u32 {
        self.max_row
    }

    pub fn find_biggest_column(&self) -> u32 {
        self.max_col
    }

    pub(crate) fn refresh_bounds(&mut self) {
        self.max_row = 0;
        self.max_col = 0;
        for (block_idx, block) in self.blocks.iter().enumerate() {
            let Some(block) = block.as_ref() else {
                continue;
            };
            let block_row = block_idx / self.stride;
            let block_col = block_idx % self.stride;
            for row in (0..BLOCK_DIM).rev() {
                if block.cells[row].iter().any(|slot| slot.read().is_some()) {
                    self.max_row = self
                        .max_row
                        .max(block_row as u32 * BLOCK_DIM as u32 + row as u32);
                    break;
                }
            }
            for col in (0..BLOCK_DIM).rev() {
                if (0..BLOCK_DIM).any(|row| block.cells[row][col].read().is_some()) {
                    self.max_col = self
                        .max_col
                        .max(block_col as u32 * BLOCK_DIM as u32 + col as u32);
                    break;
                }
            }
        }
    }

    fn update_bounds(&mut self, id: &GridCellId) {
        self.max_row = self.max_row.max(id.row);
        self.max_col = self.max_col.max(id.col);
    }

    // returns cloned cell through read lock — safe for concurrent access
    pub fn get_cell(&self, id: &GridCellId) -> Option<Cell> {
        let idx = id.block_idx(self.stride);
        let block = self.blocks.get(idx)?.as_ref()?;
        let (r, c) = id.local();
        block.cells[r][c].read().clone()
    }

    pub fn get_value(&self, id: &GridCellId) -> Option<CellValue> {
        self.get_cell(id).map(|cell| cell.val)
    }

    // exclusive access: can create blocks. used during normal mutation path.
    pub fn set_value_and_create_block(&mut self, id: &GridCellId, val: CellValue) {
        let idx = id.block_idx(self.stride);
        let block = self.blocks[idx].get_or_insert_with(Block::new);
        let (r, c) = id.local();
        let slot = block.cells[r][c].get_mut();
        match slot.as_mut() {
            Some(cell) => cell.val = val,
            None => {
                *slot = Some(Cell {
                    defined_by_formula: None,
                    val,
                    pending_dependencies: AtomicU32::new(0),
                });
                block.nonempty_cells_count += 1;
            }
        }
        self.update_bounds(id);
    }

    // shared access: acquires per-cell write lock.
    // only writes to existing cells (block must already exist).
    // used during parallel eval to write computed values.
    pub fn set_value(&self, id: &GridCellId, val: CellValue) {
        let idx = id.block_idx(self.stride);
        let Some(block) = self.blocks.get(idx).and_then(|b| b.as_ref()) else {
            return;
        };
        let (r, c) = id.local();
        let mut guard = block.cells[r][c].write();
        if let Some(cell) = guard.as_mut() {
            cell.val = val;
        }
    }

    pub fn insert_cell(&mut self, id: &GridCellId, cell: Cell) {
        let idx = id.block_idx(self.stride);
        let block = self.blocks[idx].get_or_insert_with(Block::new);
        let (r, c) = id.local();
        let slot = block.cells[r][c].get_mut();
        if slot.is_none() {
            block.nonempty_cells_count += 1;
        }
        *slot = Some(cell);
        self.update_bounds(id);
    }

    pub fn remove_cell(&mut self, id: &GridCellId) {
        let idx = id.block_idx(self.stride);
        let Some(block) = self.blocks[idx].as_mut() else {
            return;
        };
        let (r, c) = id.local();
        if block.cells[r][c].get_mut().take().is_some() {
            block.nonempty_cells_count -= 1;
        }
        if block.should_deallocate() {
            self.blocks[idx] = None;
        }
    }

    // atomic: safe to call from multiple threads during parallel eval
    pub fn increase_pending_dependencies(&self, id: &GridCellId) {
        let idx = id.block_idx(self.stride);
        let Some(block) = self.blocks.get(idx).and_then(|b| b.as_ref()) else {
            return;
        };
        let (r, c) = id.local();
        let guard = block.cells[r][c].read();
        if let Some(cell) = guard.as_ref() {
            cell.pending_dependencies.fetch_add(1, Ordering::Relaxed);
        }
    }

    // atomic: returns the value AFTER decrement
    pub fn decrease_pending_dependencies(&self, id: &GridCellId) -> u32 {
        let idx = id.block_idx(self.stride);
        let Some(block) = self.blocks.get(idx).and_then(|b| b.as_ref()) else {
            return 0;
        };
        let (r, c) = id.local();
        let guard = block.cells[r][c].read();
        if let Some(cell) = guard.as_ref() {
            cell.pending_dependencies.fetch_sub(1, Ordering::Relaxed) - 1
        } else {
            0
        }
    }

    pub fn get_pending_dependencies(&self, id: &GridCellId) -> u32 {
        let idx = id.block_idx(self.stride);
        let Some(block) = self.blocks.get(idx).and_then(|b| b.as_ref()) else {
            return 0;
        };
        let (r, c) = id.local();
        let guard = block.cells[r][c].read();
        guard
            .as_ref()
            .map_or(0, |cell| cell.pending_dependencies.load(Ordering::Relaxed))
    }

    pub(crate) fn for_each_cell_in_range<F>(
        &self,
        start_row: u32,
        start_col: u32,
        end_row: u32,
        end_col: u32,
        mut f: F,
    ) where
        F: FnMut(GridCellId, &Cell),
    {
        if start_row > end_row || start_col > end_col {
            return;
        }

        let block_row_start = start_row as usize >> BLOCK_SHIFT;
        let block_row_end = end_row as usize >> BLOCK_SHIFT;
        let block_col_start = start_col as usize >> BLOCK_SHIFT;
        let block_col_end = end_col as usize >> BLOCK_SHIFT;

        for block_row in block_row_start..=block_row_end {
            for block_col in block_col_start..=block_col_end {
                let block_idx = block_row * self.stride + block_col;
                let Some(block) = self.blocks.get(block_idx).and_then(|b| b.as_ref()) else {
                    continue;
                };

                let row_start = if block_row == block_row_start {
                    start_row as usize & BLOCK_MASK
                } else {
                    0
                };
                let row_end = if block_row == block_row_end {
                    end_row as usize & BLOCK_MASK
                } else {
                    BLOCK_DIM - 1
                };
                let col_start = if block_col == block_col_start {
                    start_col as usize & BLOCK_MASK
                } else {
                    0
                };
                let col_end = if block_col == block_col_end {
                    end_col as usize & BLOCK_MASK
                } else {
                    BLOCK_DIM - 1
                };

                for row in row_start..=row_end {
                    for col in col_start..=col_end {
                        let guard = block.cells[row][col].read();
                        let Some(cell) = guard.as_ref() else {
                            continue;
                        };
                        f(
                            GridCellId {
                                row: (block_row as u32 * BLOCK_DIM as u32) + row as u32,
                                col: (block_col as u32 * BLOCK_DIM as u32) + col as u32,
                            },
                            cell,
                        );
                    }
                }
            }
        }
    }

    // iterate values block-by-block instead of cell-by-cell.
    // each block's cells array is contiguous in memory, so once the block is in L1
    // the inner row-major scan hits every cache line sequentially.
    // also avoids re-computing block index for every cell — one lookup per block.
    pub fn for_each_value_in_range<F>(
        &self,
        start_row: u32,
        start_col: u32,
        end_row: u32,
        end_col: u32,
        mut f: F,
    ) where
        F: FnMut(&CellValue),
    {
        self.for_each_cell_in_range(start_row, start_col, end_row, end_col, |_, cell| {
            f(&cell.val)
        });
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
            for row in 0..BLOCK_DIM {
                for col in 0..BLOCK_DIM {
                    let guard = block.cells[row][col].read();
                    let Some(cell) = guard.as_ref() else {
                        continue;
                    };
                    f(
                        GridCellId {
                            row: (block_row as u32 * BLOCK_DIM as u32) + row as u32,
                            col: (block_col as u32 * BLOCK_DIM as u32) + col as u32,
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
            pending_dependencies: AtomicU32::new(0),
        }
    }

    #[test]
    fn insert_and_get() {
        let mut grid = Grid::default();
        grid.insert_cell(&id(0, 0), cell(CellValue::Text("hello".into())));
        grid.insert_cell(&id(100, 200), cell(CellValue::Number(Decimal::from(42))));

        assert!(
            matches!(grid.get_cell(&id(0, 0)).map(|c| c.val), Some(CellValue::Text(s)) if s == "hello")
        );
        assert!(matches!(
            grid.get_cell(&id(100, 200)).map(|c| c.val),
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
    fn remove_cell_test() {
        let mut grid = Grid::default();
        grid.insert_cell(&id(3, 3), cell(CellValue::Error("".into())));
        assert!(grid.get_cell(&id(3, 3)).is_some());
        grid.remove_cell(&id(3, 3));
        assert!(grid.get_cell(&id(3, 3)).is_none());
    }

    #[test]
    fn remove_frees_empty_block() {
        let mut grid = Grid::default();
        grid.insert_cell(&id(0, 0), cell(CellValue::Error("".into())));
        let idx = id(0, 0).block_idx(grid.stride);
        assert!(grid.blocks[idx].is_some());
        grid.remove_cell(&id(0, 0));
        assert!(grid.blocks[idx].is_none());
    }

    #[test]
    fn block_not_freed_while_cells_remain() {
        let mut grid = Grid::default();
        grid.insert_cell(&id(0, 0), cell(CellValue::Error("".into())));
        grid.insert_cell(&id(1, 1), cell(CellValue::Error("".into())));
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
            matches!(grid.get_cell(&id(0, 0)).map(|c| c.val), Some(CellValue::Text(s)) if s == "second")
        );
        let idx = id(0, 0).block_idx(grid.stride);
        assert_eq!(grid.blocks[idx].as_ref().unwrap().nonempty_cells_count, 1);
    }

    #[test]
    fn cells_across_block_boundaries() {
        let mut grid = Grid::default();
        let boundary = BLOCK_DIM as u32;
        grid.insert_cell(
            &id(boundary - 1, boundary - 1),
            cell(CellValue::Text("a".into())),
        );
        grid.insert_cell(&id(boundary, boundary), cell(CellValue::Text("b".into())));
        let idx_a = id(boundary - 1, boundary - 1).block_idx(grid.stride);
        let idx_b = id(boundary, boundary).block_idx(grid.stride);
        assert_ne!(idx_a, idx_b);
        assert!(
            matches!(grid.get_cell(&id(boundary - 1, boundary - 1)).map(|c| c.val), Some(CellValue::Text(s)) if s == "a")
        );
        assert!(
            matches!(grid.get_cell(&id(boundary, boundary)).map(|c| c.val), Some(CellValue::Text(s)) if s == "b")
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
                pending_dependencies: AtomicU32::new(3),
            },
        );

        grid.set_value(&id(0, 0), CellValue::Text("new".into()));

        let cell = grid.get_cell(&id(0, 0)).unwrap();
        assert_eq!(cell.defined_by_formula, Some(7));
        assert_eq!(cell.pending_dependencies.load(Ordering::Relaxed), 3);
        assert!(matches!(&cell.val, CellValue::Text(s) if s == "new"));
    }
}
