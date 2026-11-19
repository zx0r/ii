//! Zero-serde `.ipynb` cell descriptor and extraction.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum CellKind {
    Code = 0,
    Markdown = 1,
    Raw = 2,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(C)]
pub struct Cell {
    pub source_start: u32,
    pub source_end: u32,
    pub outputs_start: u32,
    pub outputs_end: u32,
    pub cell_type: u32,
    pub execution_count: u32,
    pub cell_index: u32,
    pub output_count: u32,
}

const _: () = assert!(size_of::<Cell>() == 32);
const _: () = assert!(align_of::<Cell>() == 4);
