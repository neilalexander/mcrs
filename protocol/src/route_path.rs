use crate::{Path, TracePath};

#[derive(Debug, Clone, PartialEq, Eq, defmt::Format)]
pub enum RoutePath {
    Normal(Path),
    Trace(TracePath),
}
