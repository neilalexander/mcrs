pub trait Storage {
    fn read(&mut self, key: &str, buffer: &mut [u8]) -> Result<usize, Error>;
    fn write_atomic(&mut self, key: &str, data: &[u8]) -> Result<(), Error>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    NotAvailable,
    NotFound,
    BufferTooSmall,
    InvalidKey,
    Corrupt,
    Io,
}

#[derive(Clone, Copy)]
pub struct Layout {
    pub partition_label: &'static str,
    pub partition_size: usize,
    pub max_file_size: usize,
}
