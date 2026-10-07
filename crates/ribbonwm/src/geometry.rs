//! Native size requests are independent of compositor position and focus.
use ribbon_core::{Rect, WindowId};
use std::collections::BTreeMap;

#[derive(Default)]
pub struct NativeSizes {
    accepted: BTreeMap<WindowId, (i32, f64, f64)>,
    pub requests: u64,
}
impl NativeSizes {
    pub fn needs_resize(&self, id: WindowId, pid: i32, frame: Rect) -> bool {
        self.get(&id).is_none_or(|&(owner, width, height)| {
            owner != pid || (width - frame.width).abs() > 2.0 || (height - frame.height).abs() > 2.0
        })
    }
    pub fn get(&self, id: &WindowId) -> Option<&(i32, f64, f64)> {
        self.accepted.get(id)
    }
    pub fn insert(&mut self, id: WindowId, size: (i32, f64, f64)) {
        self.accepted.insert(id, size);
    }
    pub fn remove(&mut self, id: &WindowId) {
        self.accepted.remove(id);
    }
    pub fn retain(&mut self, mut keep: impl FnMut(&WindowId, &(i32, f64, f64)) -> bool) {
        self.accepted.retain(|id, size| keep(id, size));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ribbon_core::{Action, Engine, NativeSpaceId, Settings};

    fn frame(engine: &Engine, sizes: &mut NativeSizes) -> Vec<WindowId> {
        let mut writes = Vec::new();
        for p in engine.placements() {
            if sizes.needs_resize(p.window, 42, p.frame) {
                writes.push(p.window);
                sizes.insert(p.window, (42, p.frame.width, p.frame.height));
            }
        }
        writes
    }
    #[test]
    fn focus_scroll_and_native_move_never_enqueue_native_resizes() {
        let mut e = Engine::new(Settings::default()).unwrap();
        e.update_monitor(
            "screen",
            Rect {
                x: 0.0,
                y: 30.0,
                width: 2560.0,
                height: 1410.0,
            },
            NativeSpaceId(1),
            false,
        )
        .unwrap();
        for id in 1..=5 {
            e.add_window("screen", WindowId(id), Some(800.0)).unwrap();
        }
        let mut sizes = NativeSizes::default();
        assert_eq!(frame(&e, &mut sizes).len(), 5);
        for id in [1, 5, 2, 4, 3, 1] {
            e.focus_window(WindowId(id)).unwrap();
            for _ in 0..30 {
                e.tick(1.0 / 120.0);
                assert!(frame(&e, &mut sizes).is_empty());
            }
        }
        for delta in [900.0, -1200.0, 500.0] {
            e.apply("screen", &Action::Scroll { delta }).unwrap();
            assert!(frame(&e, &mut sizes).is_empty());
        }
        // Releasing a native move changes origin only; it cannot create a
        // native size request, even for a partially clipped column.
        for mut p in e.placements() {
            p.frame.x += 372.0;
            p.frame.y += 54.0;
            assert!(!sizes.needs_resize(p.window, 42, p.frame));
        }
        // A genuine new layout width must still be applied once.
        e.observe_width(WindowId(1), 900.0).unwrap();
        assert_eq!(frame(&e, &mut sizes), vec![WindowId(1)]);
        assert!(frame(&e, &mut sizes).is_empty());
    }
}
