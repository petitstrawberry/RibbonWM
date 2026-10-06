use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModeKind {
    Float,
    Sticky,
}

#[derive(Clone, Copy, Debug, Default, Serialize)]
pub struct WindowMode {
    pub floating: bool,
    pub sticky: bool,
    #[serde(skip)]
    pub sticky_leased: bool,
}
impl WindowMode {
    pub fn wants_tile(self) -> bool {
        !self.floating && !self.sticky
    }
    pub fn change(self, kind: ModeKind, enabled: Option<bool>) -> Self {
        let mut next = self;
        let flag = match kind {
            ModeKind::Float => &mut next.floating,
            ModeKind::Sticky => &mut next.sticky,
        };
        *flag = enabled.unwrap_or(!*flag);
        next
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn disabling_sticky_preserves_explicit_float_and_vice_versa() {
        let initial = WindowMode::default();
        let both = initial
            .change(ModeKind::Float, None)
            .change(ModeKind::Sticky, None);
        assert!(!both.wants_tile());
        let floating = both.change(ModeKind::Sticky, Some(false));
        assert!(floating.floating && !floating.sticky && !floating.wants_tile());
        let sticky = both.change(ModeKind::Float, Some(false));
        assert!(sticky.sticky && !sticky.floating && !sticky.wants_tile());
        assert!(floating.change(ModeKind::Float, None).wants_tile());
    }
    #[test]
    fn explicit_values_are_idempotent() {
        let initial = WindowMode::default().change(ModeKind::Float, Some(true));
        assert!(initial.change(ModeKind::Float, Some(true)).floating);
        assert!(!initial.change(ModeKind::Sticky, Some(false)).sticky);
    }
}
