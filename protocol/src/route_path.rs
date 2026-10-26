use crate::{Path, TracePath};

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum RoutePath {
    Normal(Path),
    Trace(TracePath),
}
