//! Native size requests are independent of compositor position and focus.
use ribbon_core::{Engine, Placement, Rect, WindowId};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Default)]
pub struct NativeSizes {
    accepted: BTreeMap<WindowId, (i32, f64, f64)>,
    retried: BTreeSet<WindowId>,
    pub requests: u64,
    pub pending: Option<SizeSettlement>,
}
pub struct SizeSettlement {
    pub window: WindowId,
    pub size: (i32, f64, f64),
    pub transaction: ribbon_macos::PendingResize,
}
impl NativeSizes {
    pub fn needs_resize(&self, id: WindowId, pid: i32, frame: Rect) -> bool {
        self.get(&id).is_none_or(|&(owner, width, height)| {
            owner != pid || (width - frame.width).abs() > 2.0 || (height - frame.height).abs() > 2.0
        })
    }
    /// A stacked column changes presentation atomically after every row accepts
    /// its new size. Other columns/monitors can continue animating meanwhile.
    pub fn unsettled_columns(
        &self,
        engine: &Engine,
        plans: &[Placement],
        owners: &BTreeMap<WindowId, i32>,
    ) -> BTreeSet<WindowId> {
        let mut waiting: BTreeSet<_> = plans
            .iter()
            .filter(|p| {
                owners
                    .get(&p.window)
                    .is_some_and(|&pid| self.needs_resize(p.window, pid, p.frame))
            })
            .map(|p| p.window)
            .collect();
        if let Some(job) = &self.pending {
            waiting.insert(job.window);
        }
        for monitor in engine.monitors.values().filter(|m| !m.suspended) {
            for column in &monitor.layout().columns {
                if column.windows.iter().any(|id| waiting.contains(id)) {
                    waiting.extend(column.windows.iter().copied());
                }
            }
        }
        waiting
    }
    pub fn get(&self, id: &WindowId) -> Option<&(i32, f64, f64)> {
        self.accepted.get(id)
    }
    pub fn insert(&mut self, id: WindowId, size: (i32, f64, f64)) {
        self.retried.remove(&id);
        self.accepted.insert(id, size);
    }
    pub fn remove(&mut self, id: &WindowId) {
        self.retried.remove(id);
        if self.pending.as_ref().is_some_and(|p| p.window == *id) {
            self.pending.take();
        }
        self.accepted.remove(id);
    }
    pub fn cancel_settlement(&mut self) {
        // AX acknowledged this size before entering settlement. A mouse press,
        // lock, or context switch cancels waiting, never schedules a rebase.
        if let Some(p) = self.pending.take() {
            self.insert(p.window, p.size);
        }
    }
    pub fn retain(&mut self, mut keep: impl FnMut(&WindowId, &(i32, f64, f64)) -> bool) {
        self.accepted.retain(|id, size| keep(id, size));
        if self
            .pending
            .as_ref()
            .is_some_and(|p| !keep(&p.window, &p.size))
        {
            self.pending.take();
        }
    }
    pub fn retry_once(&mut self, id: WindowId) -> bool {
        self.retried.insert(id)
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
    fn stacked_rows_wait_together_without_holding_independent_columns() {
        let mut engine = Engine::new(Settings::default()).unwrap();
        engine
            .update_monitor(
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
        for id in 1..=3 {
            engine
                .add_window("screen", WindowId(id), Some(800.0))
                .unwrap();
        }
        let mut sizes = NativeSizes::default();
        frame(&engine, &mut sizes);
        let owners = (1..=3).map(|id| (WindowId(id), 42)).collect();
        engine
            .apply(
                "screen",
                &Action::Stack {
                    target: ribbon_core::WindowTarget::First,
                },
            )
            .unwrap();
        let plans = engine.placements();
        let column: BTreeSet<_> = [WindowId(1), WindowId(3)].into_iter().collect();
        assert_eq!(sizes.unsettled_columns(&engine, &plans, &owners), column);
        let first = plans.iter().find(|p| p.window == WindowId(1)).unwrap();
        sizes.insert(first.window, (42, first.frame.width, first.frame.height));
        assert_eq!(sizes.unsettled_columns(&engine, &plans, &owners), column);
        // Even the first acknowledged row stays at its committed presentation
        // until the second row accepts; unrelated window 2 never joins the hold.
        let second = plans.iter().find(|p| p.window == WindowId(3)).unwrap();
        sizes.insert(second.window, (42, second.frame.width, second.frame.height));
        assert!(sizes.unsettled_columns(&engine, &plans, &owners).is_empty());
        sizes.remove(&WindowId(3));
        engine.remove_window(WindowId(3)).unwrap();
        frame(&engine, &mut sizes);
        assert!(
            sizes
                .unsettled_columns(&engine, &engine.placements(), &owners)
                .is_empty()
        );
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
