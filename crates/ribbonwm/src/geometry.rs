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
    /// Commit the owner's final mouse geometry before a layout frame can issue
    /// another size request. Inventory snapshots may predate mouse-up.
    pub fn finish_user_resize(
        &mut self,
        engine: &mut Engine,
        window: WindowId,
        pid: i32,
        surface: Rect,
    ) -> anyhow::Result<()> {
        if !surface.valid() || !(100.0..=10000.0).contains(&surface.width) {
            return Ok(());
        }
        let Some(plan) = engine.placements().into_iter().find(|p| {
            p.window == window
                && !engine.monitors[&p.monitor].suspended
                && engine.monitors[&p.monitor].native_space == p.native_space
        }) else {
            return Ok(());
        };
        let Some(&(owner, width, height)) = self.get(&window) else {
            return Ok(());
        };
        if owner != pid {
            return Ok(());
        }
        if (surface.width - width).abs() > 2.0 {
            engine.observe_width(window, surface.width)?;
        }
        if (surface.height - height).abs() > 2.0 {
            // A single tiled row retains full height. Stacked rows can adopt
            // the owner's height without changing their column's total height.
            let _ = engine.resize_row(&plan.monitor, window, surface.height);
        }
        self.insert(window, (pid, surface.width, surface.height));
        Ok(())
    }
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
        self.waiting_columns(
            engine,
            plans,
            owners,
            true,
            self.pending.as_ref().map(|p| (p.window, p.size)),
        )
    }
    /// AX acceptance and temporal settlement are different milestones. Once
    /// the owner has accepted a size, displaying the old width would leave a
    /// hole beside it until the settlement timer expires.
    pub fn unaccepted_columns(
        &self,
        engine: &Engine,
        plans: &[Placement],
        owners: &BTreeMap<WindowId, i32>,
    ) -> BTreeSet<WindowId> {
        self.waiting_columns(
            engine,
            plans,
            owners,
            false,
            self.pending.as_ref().map(|p| (p.window, p.size)),
        )
    }
    fn waiting_columns(
        &self,
        engine: &Engine,
        plans: &[Placement],
        owners: &BTreeMap<WindowId, i32>,
        settling: bool,
        pending: Option<(WindowId, (i32, f64, f64))>,
    ) -> BTreeSet<WindowId> {
        let mut waiting: BTreeSet<_> = plans
            .iter()
            .filter(|p| {
                owners.get(&p.window).is_some_and(|&pid| {
                    self.needs_resize(p.window, pid, p.frame)
                        && (settling
                            || !pending.is_some_and(|(window, size)| {
                                window == p.window
                                    && size.0 == pid
                                    && (size.1 - p.frame.width).abs() <= 2.0
                                    && (size.2 - p.frame.height).abs() <= 2.0
                            }))
                })
            })
            .map(|p| p.window)
            .collect();
        if settling && let Some((window, _)) = pending {
            waiting.insert(window);
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

/// Place neighbours using the widths actually being presented in this frame,
/// not future widths that an owner has yet to accept. Scroll remains shared by
/// every column, including a held column, and other contexts remain independent.
pub fn reflow_presented_columns(engine: &Engine, desired: &[Placement], shown: &mut [Placement]) {
    for monitor in engine.monitors.values() {
        for layout in monitor.contexts.values() {
            let Some(first) = layout.columns.first().and_then(|c| c.windows.first()) else {
                continue;
            };
            let Some(mut x) = desired
                .iter()
                .find(|p| p.window == *first)
                .map(|p| p.frame.x)
            else {
                continue;
            };
            for column in &layout.columns {
                let width = shown
                    .iter()
                    .filter(|p| column.windows.contains(&p.window))
                    .map(|p| p.frame.width)
                    .reduce(f64::max)
                    .unwrap_or(column.width);
                for p in shown
                    .iter_mut()
                    .filter(|p| column.windows.contains(&p.window))
                {
                    p.frame.x = x;
                    p.clip = p.frame.intersection(monitor.viewport);
                }
                x += width + engine.settings.gap;
            }
        }
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
    fn mouse_release_adopts_final_width_before_the_next_size_request() {
        let mut engine = Engine::new(Settings::default()).unwrap();
        engine
            .update_monitor(
                "main",
                Rect {
                    x: 0.0,
                    y: 30.0,
                    width: 1500.0,
                    height: 900.0,
                },
                NativeSpaceId(1),
                false,
            )
            .unwrap();
        engine.add_window("main", WindowId(1), Some(800.0)).unwrap();
        let mut sizes = NativeSizes::default();
        frame(&engine, &mut sizes);
        let original = engine.placements()[0].frame;
        for width in [940.0, 620.0, 1020.0, 520.0] {
            let final_frame = Rect { width, ..original };
            // The release sample is newer than the last asynchronous inventory.
            sizes
                .finish_user_resize(&mut engine, WindowId(1), 42, final_frame)
                .unwrap();
            assert_eq!(engine.placements()[0].frame.width, width);
            assert!(
                frame(&engine, &mut sizes).is_empty(),
                "release must not undo a user resize with an AX write"
            );
        }
        sizes
            .finish_user_resize(&mut engine, WindowId(1), 99, original)
            .unwrap();
        assert_eq!(
            engine.placements()[0].frame.width,
            520.0,
            "a reused ID cannot change the owner's size"
        );
        engine
            .update_monitor(
                "main",
                Rect {
                    x: 0.0,
                    y: 30.0,
                    width: 1500.0,
                    height: 900.0,
                },
                NativeSpaceId(2),
                false,
            )
            .unwrap();
        sizes
            .finish_user_resize(&mut engine, WindowId(1), 42, original)
            .unwrap();
        assert_eq!(
            sizes.get(&WindowId(1)).unwrap().1,
            520.0,
            "inactive native Space is untouched"
        );
    }
    #[test]
    fn resizing_columns_never_publish_future_neighbour_spacing() {
        for width in [500.0, 1100.0] {
            let mut engine = Engine::new(Settings::default()).unwrap();
            for (name, x) in [("main", 0.0), ("other", 1500.0)] {
                engine
                    .update_monitor(
                        name,
                        Rect {
                            x,
                            y: 30.0,
                            width: 1500.0,
                            height: 900.0,
                        },
                        NativeSpaceId(1),
                        false,
                    )
                    .unwrap();
            }
            for id in 1..=3 {
                engine
                    .add_window("main", WindowId(id), Some(800.0))
                    .unwrap();
            }
            engine
                .add_window("other", WindowId(4), Some(700.0))
                .unwrap();
            engine.focus_window(WindowId(1)).unwrap();
            engine.tick(2.0);
            let mut sizes = NativeSizes::default();
            frame(&engine, &mut sizes);
            let committed = engine.placements();
            let owners = (1..=4).map(|id| (WindowId(id), 42)).collect();
            engine.apply("main", &Action::Resize { width }).unwrap();
            engine
                .apply("main", &Action::Scroll { delta: 100.0 })
                .unwrap();
            let desired = engine.placements();
            let mut shown = desired.clone();
            shown[0] = committed[0].clone();
            reflow_presented_columns(&engine, &desired, &mut shown);
            for pair in shown[..3].windows(2) {
                assert!(
                    (pair[1].frame.x - pair[0].frame.x - pair[0].frame.width - engine.settings.gap)
                        .abs()
                        < 1e-6
                );
            }
            assert_eq!(shown[0].frame.x, desired[0].frame.x); // Held column shares scrolling.
            assert_eq!(shown[3].frame, desired[3].frame); // Independent monitor.
            assert_eq!(shown[0].frame.width, 800.0);
            let pending = Some((WindowId(1), (42, width, desired[0].frame.height)));
            assert!(
                sizes
                    .waiting_columns(&engine, &desired, &owners, false, pending)
                    .is_empty()
            );
            assert_eq!(
                sizes.waiting_columns(&engine, &desired, &owners, true, pending),
                BTreeSet::from([WindowId(1)])
            );
            // Acceptance publishes the width and its neighbours together;
            // the transaction can still be under temporal observation.
            shown.clone_from(&desired);
            reflow_presented_columns(&engine, &desired, &mut shown);
            assert_eq!(shown[0].frame.width, width);
            assert!(
                (shown[1].frame.x - shown[0].frame.x - width - engine.settings.gap).abs() < 1e-6
            );
        }
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
