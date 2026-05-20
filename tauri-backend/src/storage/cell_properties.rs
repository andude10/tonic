use serde::{Deserialize, Serialize};

use crate::storage::grid::GridCellId;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
struct TextColor {
    cell: GridCellId,
    color: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq)]
pub struct CellProperties {
    #[serde(default)]
    bold: Vec<GridCellId>,
    #[serde(default)]
    italic: Vec<GridCellId>,
    #[serde(default)]
    strikethrough: Vec<GridCellId>,
    #[serde(default)]
    text_colors: Vec<TextColor>,
}

impl CellProperties {
    pub fn cell_bold(&self, cell: GridCellId) -> bool {
        self.bold.binary_search(&cell).is_ok()
    }

    pub fn set_cell_bold(&mut self, cell: GridCellId, value: bool) {
        // keep sparse bold cells sorted for binary search.
        match (self.bold.binary_search(&cell), value) {
            (Ok(idx), false) => {
                self.bold.remove(idx);
            }
            (Err(idx), true) => self.bold.insert(idx, cell),
            _ => {}
        }
    }

    pub fn cell_italic(&self, cell: GridCellId) -> bool {
        self.italic.binary_search(&cell).is_ok()
    }

    pub fn set_cell_italic(&mut self, cell: GridCellId, value: bool) {
        // keep sparse italic cells sorted for binary search.
        match (self.italic.binary_search(&cell), value) {
            (Ok(idx), false) => {
                self.italic.remove(idx);
            }
            (Err(idx), true) => self.italic.insert(idx, cell),
            _ => {}
        }
    }

    pub fn cell_strikethrough(&self, cell: GridCellId) -> bool {
        self.strikethrough.binary_search(&cell).is_ok()
    }

    pub fn set_cell_strikethrough(&mut self, cell: GridCellId, value: bool) {
        // keep sparse strikethrough cells sorted for binary search.
        match (self.strikethrough.binary_search(&cell), value) {
            (Ok(idx), false) => {
                self.strikethrough.remove(idx);
            }
            (Err(idx), true) => self.strikethrough.insert(idx, cell),
            _ => {}
        }
    }

    pub fn get_cell_color(&self, cell: GridCellId) -> Option<&str> {
        self.text_colors
            .binary_search_by_key(&cell, |entry| entry.cell)
            .ok()
            .map(|idx| self.text_colors[idx].color.as_str())
    }

    pub fn set_cell_color(&mut self, cell: GridCellId, color: Option<String>) {
        // keep sparse color overrides sorted for binary search.
        match (
            self.text_colors
                .binary_search_by_key(&cell, |entry| entry.cell),
            color,
        ) {
            (Ok(idx), Some(color)) => self.text_colors[idx].color = color,
            (Ok(idx), None) => {
                self.text_colors.remove(idx);
            }
            (Err(idx), Some(color)) => self.text_colors.insert(idx, TextColor { cell, color }),
            (Err(_), None) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(row: u32, col: u32) -> GridCellId {
        GridCellId { row, col }
    }

    #[test]
    fn set_and_get_cell_properties() {
        let mut props = CellProperties::default();
        props.set_cell_bold(id(2, 3), true);
        props.set_cell_italic(id(2, 3), true);
        props.set_cell_color(id(2, 3), Some("#ff0000".to_string()));

        assert!(props.cell_bold(id(2, 3)));
        assert!(props.cell_italic(id(2, 3)));
        assert!(!props.cell_strikethrough(id(2, 3)));
        assert_eq!(props.get_cell_color(id(2, 3)), Some("#ff0000"));
        assert!(!props.cell_bold(id(2, 4)));
    }

    #[test]
    fn removing_cell_properties_clears_sparse_entries() {
        let mut props = CellProperties::default();
        props.set_cell_bold(id(1, 1), true);
        props.set_cell_color(id(1, 1), Some("#00ff00".to_string()));
        props.set_cell_bold(id(1, 1), false);
        props.set_cell_color(id(1, 1), None);

        assert_eq!(props, CellProperties::default());
    }
}
