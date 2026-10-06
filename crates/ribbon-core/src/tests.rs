use super::*;
use proptest::prelude::*;

fn rect(x: f64, width: f64) -> Rect {
    Rect {
        x,
        y: 40.0,
        width,
        height: 700.0,
    }
}
fn engine() -> Engine {
    let mut e = Engine::new(Settings::default()).unwrap();
    e.update_monitor("left", rect(0.0, 1200.0), NativeSpaceId(3), false)
        .unwrap();
    e
}
fn add(e: &mut Engine, id: u32) {
    e.add_window("left", WindowId(id), None).unwrap();
}

#[test]
fn full_width_toggle_restores_each_columns_width_and_is_space_local() {
    let mut e = paper_engine();
    e.add_window("left", WindowId(1), Some(710.0)).unwrap();
    e.apply("left", &Action::ToggleFullWidth {}).unwrap();
    assert_eq!(e.monitors["left"].layout().columns[0].width, 1160.0);
    e.update_monitor("left", rect(0.0, 1200.0), NativeSpaceId(4), false)
        .unwrap();
    e.add_window("left", WindowId(2), Some(880.0)).unwrap();
    e.apply("left", &Action::ToggleFullWidth {}).unwrap();
    e.apply("left", &Action::ToggleFullWidth {}).unwrap();
    assert_eq!(e.monitors["left"].layout().columns[0].width, 880.0);
    e.update_monitor("left", rect(0.0, 1200.0), NativeSpaceId(3), false)
        .unwrap();
    e.apply("left", &Action::ToggleFullWidth {}).unwrap();
    assert_eq!(e.monitors["left"].layout().columns[0].width, 710.0);
    assert_eq!(e.focused_window("left"), Some(WindowId(1)));
}

#[test]
fn width_cycle_respects_padding_wraps_and_clears_an_old_zoom_restore() {
    let mut e = paper_engine();
    e.add_window("left", WindowId(1), Some(580.0)).unwrap();
    e.apply("left", &Action::CycleWidth {}).unwrap();
    assert!((e.monitors["left"].layout().columns[0].width - 1160.0 * 2.0 / 3.0).abs() < 0.001);
    e.apply("left", &Action::ToggleFullWidth {}).unwrap();
    e.apply("left", &Action::CycleWidth {}).unwrap();
    assert_eq!(e.monitors["left"].layout().columns[0].width, 580.0);
    e.apply("left", &Action::ToggleFullWidth {}).unwrap();
    assert_eq!(e.monitors["left"].layout().columns[0].width, 1160.0);
    e.apply("left", &Action::Resize { width: 900.0 }).unwrap();
    e.apply("left", &Action::ToggleFullWidth {}).unwrap();
    e.apply("left", &Action::ToggleFullWidth {}).unwrap();
    assert_eq!(e.monitors["left"].layout().columns[0].width, 900.0);
    e.apply("left", &Action::ToggleFullWidth {}).unwrap();
    e.observe_width(WindowId(1), 820.0).unwrap();
    e.apply("left", &Action::ToggleFullWidth {}).unwrap();
    e.apply("left", &Action::ToggleFullWidth {}).unwrap();
    assert_eq!(e.monitors["left"].layout().columns[0].width, 820.0);
}

#[test]
fn mirroring_is_reversible_and_preserves_focus_and_row_weights() {
    let mut e = paper_engine();
    for id in 1..=3 {
        add(&mut e, id);
    }
    e.apply("left", &Action::StackLeft {}).unwrap();
    e.resize_row("left", WindowId(3), 300.0).unwrap();
    let before: Vec<_> = e.monitors["left"]
        .layout()
        .columns
        .iter()
        .map(|c| c.windows.clone())
        .collect();
    let weights = e.row_weights.clone();
    for action in [Action::MirrorColumns {}, Action::MirrorRows {}] {
        e.apply("left", &action).unwrap();
        assert_eq!(e.focused_window("left"), Some(WindowId(3)));
        assert_eq!(e.row_weights, weights);
        e.apply("left", &action).unwrap();
        let after: Vec<_> = e.monitors["left"]
            .layout()
            .columns
            .iter()
            .map(|c| c.windows.clone())
            .collect();
        assert_eq!(before, after);
        assert_eq!(e.focused_window("left"), Some(WindowId(3)));
    }
}

fn paper_engine() -> Engine {
    let mut e = Engine::new(Settings {
        horizontal_margin: 20.0,
        vertical_margin: 20.0,
        center_content: true,
        preserve_window_width: true,
        ..Settings::default()
    })
    .unwrap();
    e.update_monitor("left", rect(0.0, 1200.0), NativeSpaceId(3), false)
        .unwrap();
    e
}

#[test]
fn paper_compact_strip_centers_with_vertical_margins_and_animates_insertion() {
    let mut e = paper_engine();
    e.add_window("left", WindowId(1), Some(400.0)).unwrap();
    assert_eq!(e.monitors["left"].layout().scroll.target, -400.0);
    assert_eq!(e.monitors["left"].layout().scroll.position, 0.0);
    e.tick(2.0);
    assert_eq!(
        e.placements()[0].frame,
        Rect {
            x: 400.0,
            y: 60.0,
            width: 400.0,
            height: 660.0
        }
    );
    e.add_window("left", WindowId(2), Some(500.0)).unwrap();
    assert_eq!(e.monitors["left"].layout().scroll.position, -400.0);
    e.tick(2.0);
    let plans = e.placements();
    assert_eq!(plans[0].frame.x, 142.0);
    assert_eq!(plans[1].frame.x, 558.0);
    assert_eq!(plans[1].frame.width, 500.0);
}

#[test]
fn paper_native_focus_is_the_anchor_for_new_columns() {
    let mut e = paper_engine();
    for id in 1..=4 {
        add(&mut e, id);
    }
    assert!(e.observe_focus(WindowId(2), true).unwrap());
    add(&mut e, 9);
    assert_eq!(
        e.monitors["left"]
            .layout()
            .columns
            .iter()
            .map(|c| c.windows[0].0)
            .collect::<Vec<_>>(),
        vec![1, 2, 9, 3, 4]
    );
}

#[test]
fn repeated_native_focus_does_not_undo_manual_scroll_or_space_offset() {
    let mut e = paper_engine();
    for id in 1..=4 {
        add(&mut e, id);
    }
    e.observe_focus(WindowId(2), true).unwrap();
    e.apply("left", &Action::Scroll { delta: 250.0 }).unwrap();
    let before = e.monitors["left"].layout().scroll;
    assert!(!e.observe_focus(WindowId(2), true).unwrap());
    assert_eq!(e.monitors["left"].layout().scroll.target, before.target);
    assert!(e.observe_focus(WindowId(3), false).unwrap());
    assert!(!e.observe_focus(WindowId(3), true).unwrap());
    assert_eq!(e.monitors["left"].layout().scroll.target, before.target);
}

#[test]
fn automatic_native_focus_preserves_manual_centering_and_its_scroll_bounds() {
    let mut e = paper_engine();
    for id in 1..=4 {
        add(&mut e, id);
    }
    e.focus_window(WindowId(1)).unwrap();
    e.apply("left", &Action::Center {}).unwrap();
    e.tick(2.0);
    let before = e.monitors["left"].layout().scroll;
    assert!(e.monitors["left"].layout().centered);
    e.observe_focus(WindowId(3), false).unwrap();
    e.tick(2.0);
    let after = e.monitors["left"].layout().scroll;
    assert!(e.monitors["left"].layout().centered);
    assert_eq!(after.position, before.position);
    assert_eq!(after.target, before.target);
    e.focus_window(WindowId(3)).unwrap();
    assert!(!e.monitors["left"].layout().centered);
}

#[test]
fn closing_a_column_left_of_focus_preserves_its_displayed_position() {
    let mut e = paper_engine();
    for id in 1..=5 {
        add(&mut e, id);
    }
    e.focus_window(WindowId(3)).unwrap();
    e.tick(2.0);
    let before = e
        .placements()
        .into_iter()
        .find(|p| p.window == WindowId(3))
        .unwrap()
        .frame;
    e.remove_window(WindowId(1)).unwrap();
    let after = e
        .placements()
        .into_iter()
        .find(|p| p.window == WindowId(3))
        .unwrap()
        .frame;
    assert_eq!(before, after);
    assert_eq!(e.focused_window("left"), Some(WindowId(3)));
    assert_eq!(e.window_ids().len(), 4);
}

#[test]
fn closing_focused_window_selects_most_recent_adjacent_neighbor() {
    let mut e = paper_engine();
    for id in 1..=4 {
        add(&mut e, id);
    }
    e.focus_window(WindowId(2)).unwrap();
    e.focus_window(WindowId(4)).unwrap();
    e.focus_window(WindowId(3)).unwrap();
    e.remove_window(WindowId(3)).unwrap();
    assert_eq!(e.focused_window("left"), Some(WindowId(4)));
}

#[test]
fn wide_column_centers_and_normal_focus_scrolls_only_as_far_as_needed() {
    let mut e = paper_engine();
    e.add_window("left", WindowId(1), Some(1040.0)).unwrap();
    e.add_window("left", WindowId(2), Some(500.0)).unwrap();
    assert_eq!(e.monitors["left"].layout().scroll.target, 376.0);
    e.focus_window(WindowId(1)).unwrap();
    assert_eq!(e.monitors["left"].layout().scroll.target, -80.0);
    e.apply("left", &Action::Resize { width: 1100.0 }).unwrap();
    assert_eq!(e.monitors["left"].layout().scroll.target, -50.0);
}

#[test]
fn centered_focus_can_center_both_edge_columns() {
    let mut e = Engine::new(Settings {
        focus_alignment: FocusAlignment::Center,
        ..Settings::default()
    })
    .unwrap();
    e.update_monitor("left", rect(0.0, 1200.0), NativeSpaceId(3), false)
        .unwrap();
    for id in 1..=3 {
        add(&mut e, id);
    }
    e.focus_window(WindowId(1)).unwrap();
    e.tick(2.0);
    assert_eq!(e.placements()[0].frame.x, 280.0);
    e.focus_window(WindowId(3)).unwrap();
    e.tick(2.0);
    assert_eq!(e.placements()[2].frame.x, 280.0);
}

#[test]
fn nonfinite_margins_are_rejected() {
    assert!(
        Engine::new(Settings {
            horizontal_margin: f64::NAN,
            ..Settings::default()
        })
        .is_err()
    );
    assert!(
        Engine::new(Settings {
            vertical_margin: f64::INFINITY,
            ..Settings::default()
        })
        .is_err()
    );
}

#[test]
fn asymmetric_padding_centers_in_the_remaining_area_and_bounds_stacks() {
    let mut e = Engine::new(Settings {
        horizontal_margin: 20.0,
        vertical_margin: 20.0,
        padding_left: Some(80.0),
        padding_right: Some(20.0),
        padding_top: Some(30.0),
        padding_bottom: Some(70.0),
        center_content: true,
        ..Settings::default()
    })
    .unwrap();
    e.update_monitor("left", rect(0.0, 1200.0), NativeSpaceId(3), false)
        .unwrap();
    e.add_window("left", WindowId(1), Some(400.0)).unwrap();
    e.tick(2.0);
    assert_eq!(
        e.placements()[0].frame,
        Rect {
            x: 430.0,
            y: 70.0,
            width: 400.0,
            height: 600.0
        }
    );
    e.add_window("left", WindowId(2), Some(400.0)).unwrap();
    e.apply("left", &Action::StackLeft {}).unwrap();
    e.tick(2.0);
    let p = e.placements();
    assert_eq!(p[0].frame.y, 70.0);
    assert_eq!(p[0].frame.height, 292.0);
    assert_eq!(p[1].frame.y, 378.0);
    assert_eq!(p[1].frame.y + p[1].frame.height, 670.0);
}

#[test]
fn asymmetric_padding_reveals_both_strip_edges_and_centers_wide_columns() {
    let mut e = Engine::new(Settings {
        padding_left: Some(80.0),
        padding_right: Some(20.0),
        ..Settings::default()
    })
    .unwrap();
    e.update_monitor("left", rect(0.0, 1200.0), NativeSpaceId(3), false)
        .unwrap();
    for id in 1..=3 {
        add(&mut e, id);
    }
    e.focus_window(WindowId(1)).unwrap();
    e.tick(2.0);
    assert_eq!(e.placements()[0].frame.x, 80.0);
    e.focus_window(WindowId(3)).unwrap();
    e.tick(2.0);
    let last = e.placements()[2].frame;
    assert_eq!(last.x + last.width, 1180.0);
    e.apply("left", &Action::Resize { width: 1100.0 }).unwrap();
    e.tick(2.0);
    assert_eq!(e.placements()[2].frame.x, 80.0);
}

#[test]
fn per_edge_padding_defaults_to_legacy_margins_and_zero_overrides_them() {
    let settings = Settings {
        horizontal_margin: 20.0,
        vertical_margin: 30.0,
        padding_left: Some(0.0),
        padding_bottom: Some(0.0),
        ..Settings::default()
    };
    assert_eq!(settings.left_margin(), 0.0);
    assert_eq!(settings.right_margin(), 20.0);
    assert_eq!(settings.top_margin(), 30.0);
    assert_eq!(settings.bottom_margin(), 0.0);
    for invalid in [f64::NAN, f64::INFINITY, -1.0, 201.0] {
        for edge in 0..4 {
            let mut s = settings.clone();
            match edge {
                0 => s.padding_left = Some(invalid),
                1 => s.padding_right = Some(invalid),
                2 => s.padding_top = Some(invalid),
                _ => s.padding_bottom = Some(invalid),
            }
            assert!(Engine::new(s).is_err());
        }
    }
}

#[test]
fn insertion_preserves_other_widths() {
    let mut e = engine();
    add(&mut e, 1);
    e.apply("left", &Action::Resize { width: 780.0 }).unwrap();
    add(&mut e, 2);
    let widths: Vec<_> = e.monitors["left"]
        .layout()
        .columns
        .iter()
        .map(|c| c.width)
        .collect();
    assert_eq!(widths, vec![780.0, 640.0]);
}

#[test]
fn extended_focus_traverses_stacks_and_mru_and_rejects_inactive_contexts() {
    let mut e = engine();
    for id in 1..=3 {
        add(&mut e, id);
    }
    e.apply("left", &Action::StackLeft {}).unwrap();
    for (target, id) in [
        (WindowTarget::First, 1),
        (WindowTarget::Last, 3),
        (WindowTarget::StackFirst, 2),
        (WindowTarget::Next, 3),
        (WindowTarget::Recent, 2),
        (WindowTarget::StackIndex(2), 3),
    ] {
        e.apply("left", &Action::Focus { target }).unwrap();
        assert_eq!(e.focused_window("left"), Some(WindowId(id)));
    }
    assert!(
        e.apply(
            "left",
            &Action::Focus {
                target: WindowTarget::Next
            }
        )
        .is_err()
    );
    e.update_monitor("left", rect(0.0, 1200.0), NativeSpaceId(9), false)
        .unwrap();
    assert!(
        e.resolve_target("left", &WindowTarget::Id(WindowId(1)))
            .is_err()
    );
}

#[test]
fn swap_warp_and_stack_preserve_window_identity_and_selection() {
    let mut e = engine();
    for id in 1..=3 {
        add(&mut e, id);
    }
    e.apply(
        "left",
        &Action::Swap {
            target: WindowTarget::First,
        },
    )
    .unwrap();
    assert_eq!(
        e.monitors["left"].layout().columns[0].windows,
        vec![WindowId(3)]
    );
    assert_eq!(e.focused_window("left"), Some(WindowId(3)));
    e.apply(
        "left",
        &Action::Move {
            target: WindowTarget::Last,
        },
    )
    .unwrap();
    assert_eq!(
        e.monitors["left"].layout().columns[2].windows,
        vec![WindowId(3)]
    );
    e.apply(
        "left",
        &Action::Stack {
            target: WindowTarget::First,
        },
    )
    .unwrap();
    assert_eq!(
        e.monitors["left"].layout().columns[0].windows,
        vec![WindowId(2), WindowId(3)]
    );
    e.apply(
        "left",
        &Action::Move {
            target: WindowTarget::Up,
        },
    )
    .unwrap();
    assert_eq!(
        e.monitors["left"].layout().columns[0].windows,
        vec![WindowId(3), WindowId(2)]
    );
    assert_eq!(
        e.window_ids(),
        [WindowId(1), WindowId(2), WindowId(3)]
            .into_iter()
            .collect()
    );
}

#[test]
fn observed_resize_is_idempotent_preserves_focus_and_updates_the_shared_column() {
    let mut e = paper_engine();
    for id in 1..=3 {
        add(&mut e, id);
    }
    e.apply("left", &Action::StackLeft {}).unwrap();
    assert!(e.observe_width(WindowId(2), 800.0).unwrap());
    assert!(!e.observe_width(WindowId(2), 801.0).unwrap());
    assert_eq!(e.focused_window("left"), Some(WindowId(3)));
    let p = e.placements();
    assert_eq!(p[1].frame.width, 800.0);
    assert_eq!(p[2].frame.width, 800.0);
    e.update_monitor("left", rect(0.0, 1200.0), NativeSpaceId(9), false)
        .unwrap();
    assert!(e.observe_width(WindowId(2), 900.0).is_err());
}

#[test]
fn relative_ratio_and_row_resize_keep_geometry_valid_and_can_balance() {
    let mut e = paper_engine();
    add(&mut e, 1);
    e.apply("left", &Action::ResizeBy { delta: 80.0 }).unwrap();
    assert_eq!(e.placements()[0].frame.width, 720.0);
    e.apply("left", &Action::ResizeRatio { ratio: 0.5 })
        .unwrap();
    assert_eq!(e.placements()[0].frame.width, 580.0);
    add(&mut e, 2);
    e.apply("left", &Action::StackLeft {}).unwrap();
    e.apply("left", &Action::ResizeHeight { height: 400.0 })
        .unwrap();
    let p = e.placements();
    assert_eq!(p[0].frame.height, 244.0);
    assert_eq!(p[1].frame.height, 400.0);
    assert_eq!(p[1].frame.y + p[1].frame.height, 720.0);
    let before = e.placements();
    assert!(
        e.apply("left", &Action::ResizeHeight { height: 600.0 })
            .is_err()
    );
    assert_eq!(e.placements()[1].frame, before[1].frame);
    e.apply("left", &Action::Balance {}).unwrap();
    assert_eq!(e.placements()[0].frame.height, 322.0);
}

#[test]
fn center_command_can_center_the_edge_column_until_focus_changes() {
    let mut e = paper_engine();
    for id in 1..=3 {
        add(&mut e, id);
    }
    e.focus_window(WindowId(1)).unwrap();
    e.apply("left", &Action::Center {}).unwrap();
    e.tick(2.0);
    assert_eq!(e.placements()[0].frame.x, 280.0);
    e.focus_window(WindowId(3)).unwrap();
    e.tick(2.0);
    assert_eq!(
        e.placements()[2].frame.x + e.placements()[2].frame.width,
        1180.0
    );
}

#[test]
fn live_settings_validation_is_atomic_and_keeps_columns_and_space_identity() {
    let mut e = engine();
    for id in 1..=3 {
        add(&mut e, id);
    }
    let mut settings = e.settings.clone();
    settings.padding_top = Some(30.0);
    settings.gap = 24.0;
    e.update_settings(settings).unwrap();
    assert_eq!(e.placements()[0].frame.y, 70.0);
    let mut bad = e.settings.clone();
    bad.gap = f64::NAN;
    assert!(e.update_settings(bad).is_err());
    assert_eq!(e.settings.gap, 24.0);
    assert_eq!(e.monitors["left"].native_space, NativeSpaceId(3));
    assert_eq!(e.window_ids().len(), 3);
}

#[test]
fn monitor_focus_history_tracks_returning_to_an_already_selected_window() {
    let mut e = engine();
    add(&mut e, 1);
    e.update_monitor("right", rect(1200.0, 1000.0), NativeSpaceId(7), false)
        .unwrap();
    e.add_window("right", WindowId(20), None).unwrap();
    assert!(!e.observe_focus(WindowId(1), true).unwrap());
    assert_eq!(e.focused_monitor(), Some("left"));
    assert!(!e.observe_focus(WindowId(20), true).unwrap());
    assert_eq!(e.focused_monitor(), Some("right"));
    assert_eq!(e.recent_monitor("right"), Some("left"));
    e.remove_window(WindowId(1)).unwrap();
    assert_eq!(e.recent_monitor("right"), None);
    e.update_monitor("right", rect(1200.0, 1000.0), NativeSpaceId(9), false)
        .unwrap();
    assert_eq!(e.focused_monitor(), None);
}

#[test]
fn size_and_pointer_selectors_use_active_windows_and_their_visible_clips() {
    let mut e = engine();
    for (id, width) in [(1, 400.0), (2, 800.0), (3, 600.0)] {
        e.add_window("left", WindowId(id), Some(width)).unwrap();
    }
    e.tick(2.0);
    assert_eq!(
        e.resolve_target("left", &WindowTarget::Largest).unwrap(),
        WindowId(2)
    );
    assert_eq!(
        e.resolve_target("left", &WindowTarget::Smallest).unwrap(),
        WindowId(1)
    );
    let p = e.placements()[2].frame;
    assert_eq!(
        e.resolve_target(
            "left",
            &WindowTarget::Point {
                x: p.x + 10.0,
                y: p.y + 10.0
            }
        )
        .unwrap(),
        WindowId(3)
    );
    assert!(
        e.resolve_target("left", &WindowTarget::Point { x: -100.0, y: p.y })
            .is_err()
    );
}
#[test]
fn native_context_switch_and_inventory_refresh_preserve_manual_scroll() {
    let mut e = engine();
    for id in 1..=4 {
        add(&mut e, id);
    }
    e.apply("left", &Action::Scroll { delta: 120.0 }).unwrap();
    let before = e.monitors["left"].layout().scroll;
    e.update_monitor("left", rect(0.0, 1200.0), NativeSpaceId(3), false)
        .unwrap();
    e.update_monitor("left", rect(0.0, 1200.0), NativeSpaceId(9), false)
        .unwrap();
    e.update_monitor("left", rect(0.0, 1200.0), NativeSpaceId(3), false)
        .unwrap();
    let after = e.monitors["left"].layout().scroll;
    assert_eq!(before.position, after.position);
    assert_eq!(before.target, after.target);
}
#[test]
fn each_native_space_has_exactly_one_layout() {
    let mut e = engine();
    add(&mut e, 1);
    e.update_monitor("left", rect(0.0, 1200.0), NativeSpaceId(9), false)
        .unwrap();
    add(&mut e, 2);
    assert_eq!(e.monitors["left"].contexts.len(), 2);
    assert_eq!(
        e.monitors["left"].layout().focused_window(),
        Some(WindowId(2))
    );
}
#[test]
fn native_space_switch_keeps_existing_compositor_placements() {
    let mut e = engine();
    add(&mut e, 1);
    e.update_monitor("left", rect(0.0, 1200.0), NativeSpaceId(9), false)
        .unwrap();
    add(&mut e, 2);
    assert_eq!(e.placements().len(), 2);
    assert_eq!(e.placements()[0].window, WindowId(1));
    assert!(!e.placements()[0].focused);
    assert!(e.placements()[0].clip.is_some());
    assert_eq!(e.window_ids().len(), 2);
}
#[test]
fn monitors_scroll_and_switch_native_spaces_independently() {
    let mut e = engine();
    e.update_monitor("right", rect(1200.0, 1000.0), NativeSpaceId(7), false)
        .unwrap();
    for i in 1..=4 {
        add(&mut e, i);
    }
    e.add_window("right", WindowId(20), None).unwrap();
    let right_before = e.monitors["right"].native_space;
    e.update_monitor("left", rect(0.0, 1200.0), NativeSpaceId(9), false)
        .unwrap();
    e.tick(0.3);
    assert_eq!(right_before, e.monitors["right"].native_space);
    assert_eq!(e.monitors["right"].layout().scroll.position, 0.0);
    for p in e.placements() {
        if let Some(c) = p.clip {
            let v = e.monitors[&p.monitor].viewport;
            assert!(c.x >= v.x && c.x + c.width <= v.x + v.width);
        }
    }
}
#[test]
fn three_display_clips_handle_adjacent_edges_and_negative_coordinates() {
    let mut e = Engine::new(Settings::default()).unwrap();
    for (name, viewport, space, window, width) in [
        (
            "studio",
            Rect {
                x: 0.0,
                y: 30.0,
                width: 2560.0,
                height: 1410.0,
            },
            3,
            1,
            3200.0,
        ),
        (
            "right",
            Rect {
                x: 2560.0,
                y: 0.0,
                width: 2560.0,
                height: 1440.0,
            },
            7672,
            2,
            3200.0,
        ),
        (
            "builtin",
            Rect {
                x: -1187.0,
                y: 1472.0,
                width: 1512.0,
                height: 950.0,
            },
            5551,
            3,
            2000.0,
        ),
    ] {
        e.update_monitor(name, viewport, NativeSpaceId(space), false)
            .unwrap();
        e.add_window(name, WindowId(window), Some(width)).unwrap();
    }
    e.apply("builtin", &Action::Scroll { delta: 420.0 })
        .unwrap();
    let plans = e.placements();
    assert_eq!(plans.len(), 3);
    for p in &plans {
        let clip = p.clip.unwrap();
        assert_eq!(
            clip.intersection(e.monitors[&p.monitor].viewport),
            Some(clip)
        );
    }
    let builtin = plans.iter().find(|p| p.window == WindowId(3)).unwrap();
    assert_eq!(builtin.frame.x, -1607.0);
    assert_eq!(builtin.clip.unwrap().x, -1187.0);
    let studio = plans.iter().find(|p| p.window == WindowId(1)).unwrap();
    assert_eq!(
        studio
            .clip
            .unwrap()
            .intersection(e.monitors["right"].viewport),
        None
    );
    assert_eq!(e.monitors["studio"].layout().scroll.position, 0.0);
}
#[test]
fn native_spaces_retain_separate_ribbon_contexts() {
    let mut e = engine();
    add(&mut e, 1);
    let original = e.monitors["left"].native_space;
    e.update_monitor("left", rect(0.0, 1200.0), NativeSpaceId(9), false)
        .unwrap();
    add(&mut e, 2);
    assert!(
        e.placements()
            .iter()
            .any(|p| p.window == WindowId(1) && !p.focused)
    );
    e.update_monitor("left", rect(0.0, 1200.0), NativeSpaceId(3), false)
        .unwrap();
    assert_eq!(original, e.monitors["left"].native_space);
    assert_eq!(e.focused_window("left"), Some(WindowId(1)));
}
#[test]
fn fullscreen_suspends_only_its_monitor() {
    let mut e = engine();
    add(&mut e, 1);
    e.update_monitor("left", rect(0.0, 1200.0), NativeSpaceId(3), true)
        .unwrap();
    assert!(e.placements().iter().all(|p| !p.focused));
    assert_eq!(e.placements().len(), 1);
    e.update_monitor("left", rect(0.0, 1200.0), NativeSpaceId(3), false)
        .unwrap();
    assert_eq!(e.placements().len(), 1);
}
#[test]
fn observed_native_space_move_preserves_identity_and_width() {
    let mut e = engine();
    add(&mut e, 1);
    add(&mut e, 2);
    e.apply("left", &Action::Resize { width: 850.0 }).unwrap();
    e.update_monitor("left", rect(0.0, 1200.0), NativeSpaceId(9), false)
        .unwrap();
    e.relocate_window("left", WindowId(2)).unwrap();
    assert_eq!(
        e.monitors["left"].layout().focused_window(),
        Some(WindowId(2))
    );
    assert_eq!(e.monitors["left"].layout().columns[0].width, 850.0);
    assert_eq!(e.window_ids().len(), 2);
    assert!(
        e.focus_window(WindowId(1)).is_err(),
        "Cannot focus a window in an inactive native Space"
    );
}
#[test]
fn stacking_and_unstacking_preserve_all_windows() {
    let mut e = engine();
    add(&mut e, 1);
    add(&mut e, 2);
    e.apply("left", &Action::StackLeft {}).unwrap();
    assert_eq!(e.monitors["left"].layout().columns.len(), 1);
    e.apply("left", &Action::Unstack {}).unwrap();
    assert_eq!(e.monitors["left"].layout().columns.len(), 2);
    assert_eq!(e.focused_window("left"), Some(WindowId(2)));
}
#[test]
fn focus_reveals_a_column_after_animation() {
    let mut e = engine();
    for i in 1..=5 {
        add(&mut e, i);
    }
    e.focus_window(WindowId(3)).unwrap();
    e.tick(2.0);
    let p = e
        .placements()
        .into_iter()
        .find(|p| p.window == WindowId(3))
        .unwrap();
    assert_eq!(p.clip, Some(p.frame));
}
#[test]
fn exact_animation_is_frame_rate_independent() {
    let mut a = Scroll {
        target: 1000.0,
        ..Scroll::default()
    };
    let mut b = a;
    for _ in 0..60 {
        a.advance(1.0 / 60.0, 14.0);
    }
    for _ in 0..120 {
        b.advance(1.0 / 120.0, 14.0);
    }
    assert!((a.position - b.position).abs() < 1e-8);
    assert!((a.velocity - b.velocity).abs() < 1e-8);
}
#[test]
fn timed_animation_finishes_in_a_quarter_second_at_multiple_refresh_rates() {
    for hz in [60, 120, 144] {
        let mut scroll = Scroll {
            target: 1000.0,
            ..Scroll::default()
        };
        for _ in 0..(hz / 4) {
            scroll.advance_eased(1.0 / hz as f64, 0.25, AnimationCurve::EaseInOut);
        }
        assert!((scroll.position - 1000.0).abs() < 1e-8);
        scroll.advance_eased(1.0 / hz as f64, 0.25, AnimationCurve::EaseInOut);
        assert_eq!(scroll.position, 1000.0);
        assert_eq!(scroll.velocity, 0.0);
    }
}
#[test]
fn timed_animation_retargets_from_the_displayed_position_without_overshoot() {
    let mut scroll = Scroll {
        target: 1000.0,
        ..Scroll::default()
    };
    scroll.advance_eased(0.125, 0.25, AnimationCurve::EaseInOut);
    assert_eq!(scroll.position, 500.0);
    scroll.target = -200.0;
    for _ in 0..30 {
        scroll.advance_eased(1.0 / 120.0, 0.25, AnimationCurve::EaseInOut);
        assert!((-200.0..=500.0).contains(&scroll.position));
    }
    scroll.advance_eased(0.01, 0.25, AnimationCurve::EaseInOut);
    assert_eq!(scroll.position, -200.0);
    scroll.target = 700.0;
    scroll.advance_eased(0.01, 0.0, AnimationCurve::EaseInOut);
    assert_eq!(scroll.position, 700.0);
}
#[test]
fn width_presets_and_animation_settings_are_validated() {
    let mut settings = Settings {
        cycle_width_ratios: vec![0.38195, 0.5, 0.61804],
        ..Settings::default()
    };
    assert!(settings.validate().is_ok());
    for ratios in [
        vec![],
        vec![0.5, 0.5],
        vec![0.8, 0.4],
        vec![f64::NAN],
        vec![1.1],
    ] {
        settings.cycle_width_ratios = ratios;
        assert!(settings.validate().is_err());
    }
    settings.cycle_width_ratios = vec![0.5];
    settings.animation_duration = f64::INFINITY;
    assert!(settings.validate().is_err());
}
#[test]
fn invalid_inputs_do_not_poison_geometry() {
    assert!(
        Engine::new(Settings {
            gap: f64::NAN,
            ..Settings::default()
        })
        .is_err()
    );
    let mut e = engine();
    add(&mut e, 1);
    assert!(
        e.apply(
            "left",
            &Action::Scroll {
                delta: f64::INFINITY
            }
        )
        .is_err()
    );
    assert!(e.add_window("left", WindowId(1), None).is_err());
    assert!(e.add_window("left", WindowId(2), Some(-10.0)).is_err());
    assert!(e.placements().iter().all(|p| p.frame.valid()));
}
#[test]
fn oversized_column_has_a_bounded_clip() {
    let mut e = engine();
    e.add_window("left", WindowId(1), Some(2000.0)).unwrap();
    let p = &e.placements()[0];
    assert_eq!(p.frame.width, 2000.0);
    assert_eq!(p.clip.unwrap().width, 1200.0);
}
proptest! {
    #[test]
    fn command_sequences_preserve_identity_focus_and_clips(ops in prop::collection::vec(0u8..17,0..200)) {
        let mut e=engine(); for i in 1..=8 {add(&mut e,i);}
        for op in ops {
            let action=match op {
                0=>Action::FocusColumn {direction:Direction::Previous},
                1=>Action::FocusColumn {direction:Direction::Next},
                2=>Action::FocusStack {direction:Direction::Next},
                3=>Action::FocusStack {direction:Direction::Previous},
                4=>Action::Resize {width:800.0},
                5=>Action::Resize {width:500.0},
                6=>Action::StackLeft {}, 7=>Action::Unstack {}, 8=>Action::Scroll {delta:450.0},
                9=>Action::Swap { target:WindowTarget::Next },
                10=>Action::Move { target:WindowTarget::First },
                11=>Action::Stack { target:WindowTarget::Last },
                12=>Action::ResizeHeight { height:250.0 },
                13=>Action::Balance {}, 14=>Action::Center {},
                15=>Action::ResizeBy { delta:-50.0 },
                _=>Action::Focus { target:WindowTarget::Recent },
            };
            let _=e.apply("left",&action); e.tick(0.02);
            prop_assert_eq!(e.window_ids().len(),8);
            let plans=e.placements();
            prop_assert_eq!(plans.len(),8);
            let ids:BTreeSet<_>=plans.iter().map(|p|p.window).collect();
            prop_assert_eq!(ids.len(),8);
            for p in plans {
                prop_assert!(p.frame.valid());
                if let Some(c)=p.clip {
                    prop_assert!(c.valid()); prop_assert_eq!(c.intersection(e.monitors["left"].viewport),Some(c));
                }
            }
            for w in e.monitors["left"].contexts.values() {
                prop_assert!(w.scroll.position.is_finite());
                prop_assert!(w.columns.is_empty() || w.focused_window().is_some());
            }
        }
    }
}
