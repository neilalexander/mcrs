use crate::{Path, TracePath};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoutePath {
    Normal(Path),
    Trace(TracePath),
}
