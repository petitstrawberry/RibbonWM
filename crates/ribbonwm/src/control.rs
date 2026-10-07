//! Geometry writer policy. AppKit and Dock ownership take precedence over
//! layout, including the interval before a mouse press has a known target.
use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlPhase {
    Inactive,
    Overview,
    Pointer,
    Reconciling,
    Layout,
}
impl ControlPhase {
    pub fn new(active: bool, overview: bool, pointer: bool, reconciling: bool) -> Self {
        if !active {
            Self::Inactive
        } else if overview {
            Self::Overview
        } else if pointer {
            Self::Pointer
        } else if reconciling {
            Self::Reconciling
        } else {
            Self::Layout
        }
    }
    pub fn owns_geometry(self) -> bool {
        self == Self::Layout
    }
    pub fn presents_layout(self) -> bool {
        matches!(self, Self::Layout | Self::Reconciling)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn competing_writers_cannot_receive_layout_permission() {
        for active in [false, true] {
            for overview in [false, true] {
                for pointer in [false, true] {
                    for reconcile in [false, true] {
                        let phase = ControlPhase::new(active, overview, pointer, reconcile);
                        assert_eq!(
                            phase.owns_geometry(),
                            active && !overview && !pointer && !reconcile
                        );
                        assert_eq!(phase.presents_layout(), active && !overview && !pointer);
                    }
                }
            }
        }
        assert_eq!(
            ControlPhase::new(true, true, true, false),
            ControlPhase::Overview
        );
        assert_eq!(
            ControlPhase::new(true, false, true, true),
            ControlPhase::Pointer
        );
    }
}
