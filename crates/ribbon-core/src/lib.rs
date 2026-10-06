//! Platform-independent scrollable layout. Coordinates are logical points.
//! Every native macOS Space owns one horizontal scroll layout per monitor.
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct WindowId(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NativeSpaceId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[repr(C)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    pub fn valid(self) -> bool {
        [self.x, self.y, self.width, self.height]
            .iter()
            .all(|v| v.is_finite())
            && self.width > 0.0
            && self.height > 0.0
    }
    pub fn intersection(self, other: Self) -> Option<Self> {
        let x = self.x.max(other.x);
        let y = self.y.max(other.y);
        let right = (self.x + self.width).min(other.x + other.width);
        let bottom = (self.y + self.height).min(other.y + other.height);
        (right > x && bottom > y).then_some(Self {
            x,
            y,
            width: right - x,
            height: bottom - y,
        })
    }
    pub fn contains(self, x: f64, y: f64) -> bool {
        x >= self.x && y >= self.y && x < self.x + self.width && y < self.y + self.height
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FocusAlignment {
    #[default]
    Visible,
    Center,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    pub column_width: f64,
    pub gap: f64,
    pub focus_alignment: FocusAlignment,
    pub animation_frequency: f64,
    pub frame_rate: u32,
    pub horizontal_margin: f64,
    pub vertical_margin: f64,
    pub padding_left: Option<f64>,
    pub padding_right: Option<f64>,
    pub padding_top: Option<f64>,
    pub padding_bottom: Option<f64>,
    pub center_content: bool,
    pub preserve_window_width: bool,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            column_width: 640.0,
            gap: 16.0,
            focus_alignment: FocusAlignment::Visible,
            animation_frequency: 14.0,
            frame_rate: 60,
            horizontal_margin: 0.0,
            vertical_margin: 0.0,
            padding_left: None,
            padding_right: None,
            padding_top: None,
            padding_bottom: None,
            center_content: false,
            preserve_window_width: false,
        }
    }
}
impl Settings {
    pub fn validate(&self) -> Result<(), LayoutError> {
        if !self.column_width.is_finite()
            || !(100.0..=10000.0).contains(&self.column_width)
            || !self.gap.is_finite()
            || !(0.0..=100.0).contains(&self.gap)
            || !self.animation_frequency.is_finite()
            || !(1.0..=100.0).contains(&self.animation_frequency)
            || !(1..=240).contains(&self.frame_rate)
            || !self.horizontal_margin.is_finite()
            || !(0.0..=200.0).contains(&self.horizontal_margin)
            || !self.vertical_margin.is_finite()
            || !(0.0..=200.0).contains(&self.vertical_margin)
            || [
                self.padding_left,
                self.padding_right,
                self.padding_top,
                self.padding_bottom,
            ]
            .into_iter()
            .flatten()
            .any(|v| !v.is_finite() || !(0.0..=200.0).contains(&v))
        {
            return Err(LayoutError::InvalidSettings);
        }
        Ok(())
    }
    pub fn left_margin(&self) -> f64 {
        self.padding_left.unwrap_or(self.horizontal_margin)
    }
    pub fn right_margin(&self) -> f64 {
        self.padding_right.unwrap_or(self.horizontal_margin)
    }
    pub fn top_margin(&self) -> f64 {
        self.padding_top.unwrap_or(self.vertical_margin)
    }
    pub fn bottom_margin(&self) -> f64 {
        self.padding_bottom.unwrap_or(self.vertical_margin)
    }
    fn center_offset(&self) -> f64 {
        (self.right_margin() - self.left_margin()) / 2.0
    }
}

#[derive(Debug, PartialEq)]
pub enum LayoutError {
    InvalidSettings,
    InvalidGeometry,
    UnknownMonitor,
    UnknownWindow,
    DuplicateWindow,
    NoFocusedWindow,
}
impl fmt::Display for LayoutError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for LayoutError {}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Previous,
    Next,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowTarget {
    Left,
    Right,
    Up,
    Down,
    Previous,
    Next,
    First,
    Last,
    Recent,
    Largest,
    Smallest,
    Point { x: f64, y: f64 },
    StackPrevious,
    StackNext,
    StackFirst,
    StackLast,
    StackRecent,
    StackIndex(usize),
    Id(WindowId),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    Focus { target: WindowTarget },
    Swap { target: WindowTarget },
    Move { target: WindowTarget },
    Stack { target: WindowTarget },
    FocusColumn { direction: Direction },
    FocusStack { direction: Direction },
    Scroll { delta: f64 },
    Resize { width: f64 },
    ResizeBy { delta: f64 },
    ResizeRatio { ratio: f64 },
    ToggleFullWidth {},
    CycleWidth {},
    MirrorColumns {},
    MirrorRows {},
    ResizeHeight { height: f64 },
    ResizeHeightBy { delta: f64 },
    Balance {},
    Center {},
    StackLeft {},
    Unstack {},
}

#[derive(Clone, Copy, Debug, Default, Serialize)]
pub struct Scroll {
    pub position: f64,
    pub target: f64,
    pub velocity: f64,
}
impl Scroll {
    fn clamp(&mut self, min: f64, max: f64) {
        self.target = self.target.clamp(min, max);
        // A changed strip width may put the displayed offset outside its new
        // bounds. Animate toward the bounded target instead of teleporting the
        // entire compact strip when a window opens or closes.
    }
    fn advance(&mut self, dt: f64, frequency: f64) {
        if !dt.is_finite() || dt <= 0.0 {
            return;
        }
        // Exact critically damped solution; independent of frame rate and stable after stalls.
        let displacement = self.position - self.target;
        let coefficient = self.velocity + frequency * displacement;
        let decay = (-frequency * dt).exp();
        self.position = self.target + (displacement + coefficient * dt) * decay;
        self.velocity = (self.velocity - frequency * coefficient * dt) * decay;
        if (self.position - self.target).abs() < 0.01 && self.velocity.abs() < 0.01 {
            self.position = self.target;
            self.velocity = 0.0;
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Column {
    pub width: f64,
    /// Width to restore after leaving full-width mode; belongs to this column.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub normal_width: Option<f64>,
    pub windows: Vec<WindowId>,
}

#[derive(Clone, Debug, Serialize)]
pub struct SpaceLayout {
    pub id: NativeSpaceId,
    pub columns: Vec<Column>,
    pub focused_column: usize,
    pub focused_row: usize,
    pub scroll: Scroll,
    #[serde(skip)]
    focus_history: Vec<WindowId>,
    #[serde(skip)]
    centered: bool,
}
impl SpaceLayout {
    fn new(id: NativeSpaceId) -> Self {
        Self {
            id,
            columns: Vec::new(),
            focused_column: 0,
            focused_row: 0,
            scroll: Scroll::default(),
            focus_history: Vec::new(),
            centered: false,
        }
    }
    pub fn focused_window(&self) -> Option<WindowId> {
        self.columns
            .get(self.focused_column)?
            .windows
            .get(self.focused_row)
            .copied()
    }
    pub fn strip_width(&self, gap: f64) -> f64 {
        self.columns.iter().map(|c| c.width).sum::<f64>()
            + gap * self.columns.len().saturating_sub(1) as f64
    }
    fn scroll_bounds(&self, viewport: Rect, settings: &Settings) -> (f64, f64) {
        let width = self.strip_width(settings.gap);
        if self.columns.is_empty() {
            return (0.0, 0.0);
        }
        if settings.center_content
            && width <= viewport.width - settings.left_margin() - settings.right_margin()
        {
            let centered = (width - viewport.width) / 2.0 + settings.center_offset();
            return (centered, centered);
        }
        if settings.focus_alignment == FocusAlignment::Center || self.centered {
            let first = self.columns[0].width;
            let last = self.columns.last().unwrap().width;
            return (
                (first - viewport.width) / 2.0 + settings.center_offset(),
                width - (last + viewport.width) / 2.0 + settings.center_offset(),
            );
        }
        let first = self.columns[0].width;
        let last = self.columns.last().unwrap().width;
        let min = if Self::wide_column(first, viewport, settings) {
            (first - viewport.width) / 2.0 + settings.center_offset()
        } else {
            -settings.left_margin()
        };
        let max = if Self::wide_column(last, viewport, settings) {
            width - (last + viewport.width) / 2.0 + settings.center_offset()
        } else {
            width - viewport.width + settings.right_margin()
        };
        (min, max.max(min))
    }
    fn wide_column(width: f64, viewport: Rect, settings: &Settings) -> bool {
        settings.left_margin() + settings.right_margin() > 0.0
            && width
                > viewport.width * 0.9
                    - settings.left_margin()
                    - settings.right_margin()
                    - 2.0 * settings.gap
    }
    fn column_x(&self, index: usize, gap: f64) -> f64 {
        self.columns.iter().take(index).map(|c| c.width + gap).sum()
    }
    fn remember_focus(&mut self) {
        if let Some(id) = self.focused_window() {
            self.focus_history.retain(|w| *w != id);
            self.focus_history.push(id);
        }
    }
    fn repair(&mut self, viewport: Rect, settings: &Settings) {
        self.columns.retain(|c| !c.windows.is_empty());
        self.focused_column = self
            .focused_column
            .min(self.columns.len().saturating_sub(1));
        self.focused_row = self.columns.get(self.focused_column).map_or(0, |c| {
            self.focused_row.min(c.windows.len().saturating_sub(1))
        });
        let (min, max) = self.scroll_bounds(viewport, settings);
        self.scroll.clamp(min, max);
    }
    fn reveal_focus(&mut self, viewport: Rect, settings: &Settings) {
        self.repair(viewport, settings);
        let Some(column) = self.columns.get(self.focused_column) else {
            return;
        };
        let x = self.column_x(self.focused_column, settings.gap);
        let alignment = if self.centered {
            FocusAlignment::Center
        } else {
            settings.focus_alignment
        };
        let target = match alignment {
            FocusAlignment::Center => {
                x + (column.width - viewport.width) / 2.0 + settings.center_offset()
            }
            FocusAlignment::Visible if Self::wide_column(column.width, viewport, settings) => {
                x + (column.width - viewport.width) / 2.0 + settings.center_offset()
            }
            FocusAlignment::Visible
                if column.width >= viewport.width
                    || x - settings.left_margin() < self.scroll.target =>
            {
                x - settings.left_margin()
            }
            FocusAlignment::Visible
                if x + column.width + settings.right_margin()
                    > self.scroll.target + viewport.width =>
            {
                x + column.width + settings.right_margin() - viewport.width
            }
            FocusAlignment::Visible => self.scroll.target,
        };
        let (min, max) = self.scroll_bounds(viewport, settings);
        self.scroll.target = target.clamp(min, max);
    }
    fn insert(&mut self, id: WindowId, width: f64, viewport: Rect, settings: &Settings) {
        self.centered = false;
        let index = if self.columns.is_empty() {
            0
        } else {
            self.focused_column + 1
        };
        self.columns.insert(
            index,
            Column {
                width,
                normal_width: None,
                windows: vec![id],
            },
        );
        self.focused_column = index;
        self.focused_row = 0;
        self.remember_focus();
        self.reveal_focus(viewport, settings);
    }
    fn remove(&mut self, id: WindowId, viewport: Rect, settings: &Settings) -> Option<f64> {
        let index = self.columns.iter().position(|c| c.windows.contains(&id))?;
        let focused = self.focused_window();
        let old_x = self.column_x(self.focused_column, settings.gap);
        let neighbors: Vec<_> = self
            .columns
            .iter()
            .flat_map(|c| &c.windows)
            .copied()
            .collect();
        let order = neighbors.iter().position(|w| *w == id).unwrap();
        let previous = order.checked_sub(1).map(|i| neighbors[i]);
        let next = neighbors.get(order + 1).copied();
        let width = self.columns[index].width;
        let row = self.columns[index].windows.iter().position(|w| *w == id)?;
        self.columns[index].windows.remove(row);
        if index == self.focused_column && row < self.focused_row {
            self.focused_row -= 1;
        }
        if self.columns[index].windows.is_empty() {
            self.columns.remove(index);
            if index < self.focused_column {
                self.focused_column -= 1;
            }
        }
        self.focus_history.retain(|w| *w != id);
        let selected = if focused == Some(id) {
            self.focus_history
                .iter()
                .rev()
                .copied()
                .find(|w| Some(*w) == previous || Some(*w) == next)
                .or(previous)
                .or(next)
        } else {
            focused
        };
        if let Some(selected) = selected {
            for (ci, column) in self.columns.iter().enumerate() {
                if let Some(ri) = column.windows.iter().position(|w| *w == selected) {
                    self.focused_column = ci;
                    self.focused_row = ri;
                    break;
                }
            }
            if focused != Some(id) {
                let shift = self.column_x(self.focused_column, settings.gap) - old_x;
                self.scroll.position += shift;
                self.scroll.target += shift;
            }
        }
        self.remember_focus();
        self.reveal_focus(viewport, settings);
        Some(width)
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Monitor {
    pub id: String,
    pub viewport: Rect,
    pub native_space: NativeSpaceId,
    pub suspended: bool,
    /// One independent horizontal layout for each native macOS Space.
    pub contexts: BTreeMap<NativeSpaceId, SpaceLayout>,
}
impl Monitor {
    pub fn layout(&self) -> &SpaceLayout {
        &self.contexts[&self.native_space]
    }
    fn layout_mut(&mut self) -> &mut SpaceLayout {
        self.contexts
            .get_mut(&self.native_space)
            .expect("native Space context invariant")
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Placement {
    pub window: WindowId,
    pub monitor: String,
    pub native_space: NativeSpaceId,
    pub frame: Rect,
    /// None means an empty drawing AND input clip, not an off-screen coordinate hack.
    pub clip: Option<Rect>,
    pub focused: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct Engine {
    pub settings: Settings,
    pub monitors: BTreeMap<String, Monitor>,
    pub row_weights: BTreeMap<WindowId, f64>,
    #[serde(skip)]
    global_focus_history: Vec<WindowId>,
}
impl Engine {
    pub fn new(settings: Settings) -> Result<Self, LayoutError> {
        settings.validate()?;
        Ok(Self {
            settings,
            monitors: BTreeMap::new(),
            row_weights: BTreeMap::new(),
            global_focus_history: Vec::new(),
        })
    }
    pub fn update_monitor(
        &mut self,
        id: &str,
        viewport: Rect,
        native_space: NativeSpaceId,
        suspended: bool,
    ) -> Result<(), LayoutError> {
        if !viewport.valid() {
            return Err(LayoutError::InvalidGeometry);
        }
        let monitor = self.monitors.entry(id.into()).or_insert_with(|| Monitor {
            id: id.into(),
            viewport,
            native_space,
            suspended,
            contexts: BTreeMap::new(),
        });
        let changed = monitor.viewport != viewport;
        monitor.viewport = viewport;
        monitor.native_space = native_space;
        monitor.suspended = suspended;
        monitor
            .contexts
            .entry(native_space)
            .or_insert_with(|| SpaceLayout::new(native_space));
        for layout in monitor.contexts.values_mut() {
            if changed {
                layout.reveal_focus(viewport, &self.settings);
            } else {
                layout.repair(viewport, &self.settings);
            }
        }
        Ok(())
    }
    pub fn window_ids(&self) -> BTreeSet<WindowId> {
        self.monitors
            .values()
            .flat_map(|m| m.contexts.values())
            .flat_map(|s| &s.columns)
            .flat_map(|c| &c.windows)
            .copied()
            .collect()
    }
    pub fn window_context(&self, id: WindowId) -> Option<(&str, NativeSpaceId)> {
        for m in self.monitors.values() {
            for (sid, s) in &m.contexts {
                if s.columns.iter().any(|c| c.windows.contains(&id)) {
                    return Some((&m.id, *sid));
                }
            }
        }
        None
    }
    pub fn add_window(
        &mut self,
        monitor: &str,
        window: WindowId,
        width: Option<f64>,
    ) -> Result<(), LayoutError> {
        if self.window_ids().contains(&window) {
            return Err(LayoutError::DuplicateWindow);
        }
        let width = width.unwrap_or(self.settings.column_width);
        if !width.is_finite() || !(100.0..=10000.0).contains(&width) {
            return Err(LayoutError::InvalidGeometry);
        }
        let m = self
            .monitors
            .get_mut(monitor)
            .ok_or(LayoutError::UnknownMonitor)?;
        let viewport = m.viewport;
        m.layout_mut()
            .insert(window, width, viewport, &self.settings);
        Ok(())
    }
    pub fn remove_window(&mut self, window: WindowId) -> Result<(), LayoutError> {
        self.row_weights.remove(&window);
        self.global_focus_history.retain(|id| *id != window);
        for m in self.monitors.values_mut() {
            for s in m.contexts.values_mut() {
                if s.remove(window, m.viewport, &self.settings).is_some() {
                    return Ok(());
                }
            }
        }
        Err(LayoutError::UnknownWindow)
    }
    /// Observe an OS-driven move to another Space or display. This never changes native membership.
    pub fn relocate_window(&mut self, monitor: &str, window: WindowId) -> Result<(), LayoutError> {
        if !self.monitors.contains_key(monitor) {
            return Err(LayoutError::UnknownMonitor);
        }
        let mut width = None;
        for m in self.monitors.values_mut() {
            for s in m.contexts.values_mut() {
                if let Some(w) = s.remove(window, m.viewport, &self.settings) {
                    width = Some(w);
                }
            }
        }
        self.add_window(
            monitor,
            window,
            Some(width.ok_or(LayoutError::UnknownWindow)?),
        )
    }
    pub fn focus_window(&mut self, window: WindowId) -> Result<(), LayoutError> {
        for m in self.monitors.values_mut().filter(|m| !m.suspended) {
            let viewport = m.viewport;
            let s = m.layout_mut();
            for (ci, c) in s.columns.iter().enumerate() {
                if let Some(ri) = c.windows.iter().position(|id| *id == window) {
                    s.focused_column = ci;
                    s.focused_row = ri;
                    s.centered = false;
                    s.remember_focus();
                    s.reveal_focus(viewport, &self.settings);
                    self.global_focus_history.retain(|id| *id != window);
                    self.global_focus_history.push(window);
                    return Ok(());
                }
            }
        }
        Err(LayoutError::UnknownWindow)
    }
    /// Observe a native focus change without continuously undoing manual scroll.
    pub fn observe_focus(&mut self, window: WindowId, reveal: bool) -> Result<bool, LayoutError> {
        for m in self.monitors.values_mut().filter(|m| !m.suspended) {
            let viewport = m.viewport;
            let s = m.layout_mut();
            for (ci, column) in s.columns.iter().enumerate() {
                if let Some(ri) = column.windows.iter().position(|w| *w == window) {
                    self.global_focus_history.retain(|id| *id != window);
                    self.global_focus_history.push(window);
                    if s.focused_column == ci && s.focused_row == ri {
                        return Ok(false);
                    }
                    s.focused_column = ci;
                    s.focused_row = ri;
                    s.centered = false;
                    s.remember_focus();
                    if reveal {
                        s.reveal_focus(viewport, &self.settings);
                    }
                    return Ok(true);
                }
            }
        }
        Err(LayoutError::UnknownWindow)
    }
    pub fn recent_monitor(&self, current: &str) -> Option<&str> {
        self.global_focus_history.iter().rev().find_map(|id| {
            let (mid, space) = self.window_context(*id)?;
            let m = &self.monitors[mid];
            (mid != current && !m.suspended && m.native_space == space).then_some(mid)
        })
    }
    pub fn focused_monitor(&self) -> Option<&str> {
        self.global_focus_history.iter().rev().find_map(|id| {
            let (mid, space) = self.window_context(*id)?;
            let m = &self.monitors[mid];
            (!m.suspended && m.native_space == space).then_some(mid)
        })
    }
    pub fn resolve_target(
        &self,
        monitor: &str,
        target: &WindowTarget,
    ) -> Result<WindowId, LayoutError> {
        let m = self
            .monitors
            .get(monitor)
            .ok_or(LayoutError::UnknownMonitor)?;
        if m.suspended {
            return Err(LayoutError::NoFocusedWindow);
        }
        let s = m.layout();
        let current = s.focused_window().ok_or(LayoutError::NoFocusedWindow)?;
        let column = &s.columns[s.focused_column];
        let all: Vec<_> = s
            .columns
            .iter()
            .flat_map(|c| c.windows.iter().copied())
            .collect();
        let index = all.iter().position(|id| *id == current).unwrap();
        let id = match target {
            WindowTarget::Largest | WindowTarget::Smallest => {
                let plans: Vec<_> = self
                    .placements()
                    .into_iter()
                    .filter(|p| p.monitor == monitor && p.native_space == m.native_space)
                    .collect();
                let compare = |a: &&Placement, b: &&Placement| {
                    (a.frame.width * a.frame.height).total_cmp(&(b.frame.width * b.frame.height))
                };
                if matches!(target, WindowTarget::Largest) {
                    plans.iter().max_by(compare)
                } else {
                    plans.iter().min_by(compare)
                }
                .map(|p| p.window)
            }
            WindowTarget::Point { x, y } => self
                .placements()
                .iter()
                .find(|p| {
                    p.monitor == monitor
                        && p.native_space == m.native_space
                        && p.clip.is_some_and(|r| r.contains(*x, *y))
                })
                .map(|p| p.window),
            WindowTarget::Id(id) => all.contains(id).then_some(*id),
            WindowTarget::First => all.first().copied(),
            WindowTarget::Last => all.last().copied(),
            WindowTarget::Previous => index.checked_sub(1).and_then(|i| all.get(i)).copied(),
            WindowTarget::Next => all.get(index + 1).copied(),
            WindowTarget::Left => s
                .focused_column
                .checked_sub(1)
                .and_then(|i| s.columns.get(i))
                .and_then(|c| c.windows.get(s.focused_row.min(c.windows.len() - 1)))
                .copied(),
            WindowTarget::Right => s
                .columns
                .get(s.focused_column + 1)
                .and_then(|c| c.windows.get(s.focused_row.min(c.windows.len() - 1)))
                .copied(),
            WindowTarget::Up | WindowTarget::StackPrevious => s
                .focused_row
                .checked_sub(1)
                .and_then(|i| column.windows.get(i))
                .copied(),
            WindowTarget::Down | WindowTarget::StackNext => {
                column.windows.get(s.focused_row + 1).copied()
            }
            WindowTarget::StackFirst => column.windows.first().copied(),
            WindowTarget::StackLast => column.windows.last().copied(),
            WindowTarget::StackIndex(i) => i
                .checked_sub(1)
                .and_then(|i| column.windows.get(i))
                .copied(),
            WindowTarget::Recent => s
                .focus_history
                .iter()
                .rev()
                .find(|id| **id != current && all.contains(id))
                .copied(),
            WindowTarget::StackRecent => s
                .focus_history
                .iter()
                .rev()
                .find(|id| **id != current && column.windows.contains(id))
                .copied(),
        };
        id.ok_or(LayoutError::UnknownWindow)
    }
    /// Adopt an app/user resize only in its current native Space. This does not activate it.
    pub fn observe_width(&mut self, window: WindowId, width: f64) -> Result<bool, LayoutError> {
        if !width.is_finite() || !(100.0..=10000.0).contains(&width) {
            return Err(LayoutError::InvalidGeometry);
        }
        for m in self.monitors.values_mut().filter(|m| !m.suspended) {
            let viewport = m.viewport;
            let s = m.layout_mut();
            if let Some(c) = s.columns.iter_mut().find(|c| c.windows.contains(&window)) {
                if (c.width - width).abs() <= 2.0 {
                    return Ok(false);
                }
                c.width = width;
                c.normal_width = None;
                s.repair(viewport, &self.settings);
                return Ok(true);
            }
        }
        Err(LayoutError::UnknownWindow)
    }
    pub fn resize_row(
        &mut self,
        monitor: &str,
        window: WindowId,
        height: f64,
    ) -> Result<(), LayoutError> {
        if !height.is_finite() || height < 100.0 {
            return Err(LayoutError::InvalidGeometry);
        }
        let m = self
            .monitors
            .get(monitor)
            .ok_or(LayoutError::UnknownMonitor)?;
        let c = m
            .layout()
            .columns
            .iter()
            .find(|c| c.windows.contains(&window))
            .ok_or(LayoutError::UnknownWindow)?;
        if c.windows.len() < 2 {
            return Err(LayoutError::InvalidGeometry);
        }
        let available = (m.viewport.height
            - self.settings.top_margin()
            - self.settings.bottom_margin()
            - self.settings.gap * (c.windows.len() - 1) as f64)
            .max(1.0);
        if height > available - 100.0 * (c.windows.len() - 1) as f64 {
            return Err(LayoutError::InvalidGeometry);
        }
        // Preserve other rows' proportions while reserving a 100-point minimum.
        let other_total: f64 = c
            .windows
            .iter()
            .filter(|id| **id != window)
            .map(|id| self.row_weights.get(id).copied().unwrap_or(1.0))
            .sum();
        let extra = available - height - 100.0 * (c.windows.len() - 1) as f64;
        for id in &c.windows {
            let weight = if *id == window {
                height
            } else {
                100.0 + extra * self.row_weights.get(id).copied().unwrap_or(1.0) / other_total
            };
            self.row_weights.insert(*id, weight);
        }
        Ok(())
    }
    pub fn update_settings(&mut self, settings: Settings) -> Result<(), LayoutError> {
        settings.validate()?;
        self.settings = settings;
        for m in self.monitors.values_mut() {
            for s in m.contexts.values_mut() {
                s.reveal_focus(m.viewport, &self.settings);
            }
        }
        Ok(())
    }
    pub fn apply(&mut self, monitor: &str, action: &Action) -> Result<(), LayoutError> {
        if let Action::Focus { target } = action {
            return self.focus_window(self.resolve_target(monitor, target)?);
        }
        if let Action::ResizeBy { delta } = action {
            let m = self
                .monitors
                .get(monitor)
                .ok_or(LayoutError::UnknownMonitor)?;
            let width = m
                .layout()
                .columns
                .get(m.layout().focused_column)
                .ok_or(LayoutError::NoFocusedWindow)?
                .width
                + delta;
            return self.apply(monitor, &Action::Resize { width });
        }
        if let Action::ResizeRatio { ratio } = action {
            if !ratio.is_finite() || !(0.0..=1.0).contains(ratio) || *ratio == 0.0 {
                return Err(LayoutError::InvalidGeometry);
            }
            let m = self
                .monitors
                .get(monitor)
                .ok_or(LayoutError::UnknownMonitor)?;
            let width =
                (m.viewport.width - self.settings.left_margin() - self.settings.right_margin())
                    * ratio;
            return self.apply(monitor, &Action::Resize { width });
        }
        if matches!(
            action,
            Action::ResizeHeight { .. } | Action::ResizeHeightBy { .. }
        ) {
            let id = self
                .focused_window(monitor)
                .ok_or(LayoutError::NoFocusedWindow)?;
            let height = match action {
                Action::ResizeHeight { height } => *height,
                Action::ResizeHeightBy { delta } => {
                    self.placements()
                        .iter()
                        .find(|p| p.window == id)
                        .ok_or(LayoutError::UnknownWindow)?
                        .frame
                        .height
                        + delta
                }
                _ => unreachable!(),
            };
            return self.resize_row(monitor, id, height);
        }
        let target = match action {
            Action::Move { target } | Action::Swap { target } | Action::Stack { target } => {
                Some(self.resolve_target(monitor, target)?)
            }
            _ => None,
        };
        let m = self
            .monitors
            .get_mut(monitor)
            .ok_or(LayoutError::UnknownMonitor)?;
        let viewport = m.viewport;
        let w = m.layout_mut();
        if !matches!(action, Action::Center {} | Action::Scroll { .. }) {
            w.centered = false;
        }
        match action {
            Action::Move { .. } | Action::Swap { .. } | Action::Stack { .. } => {
                let id = w.focused_window().ok_or(LayoutError::NoFocusedWindow)?;
                let target = target.unwrap();
                if id == target {
                    return Err(LayoutError::InvalidGeometry);
                }
                let (ci, ri) = w
                    .columns
                    .iter()
                    .enumerate()
                    .find_map(|(ci, c)| {
                        c.windows
                            .iter()
                            .position(|id| *id == target)
                            .map(|ri| (ci, ri))
                    })
                    .ok_or(LayoutError::UnknownWindow)?;
                let (source, row) = (w.focused_column, w.focused_row);
                if matches!(action, Action::Swap { .. }) {
                    w.columns[source].windows[row] = target;
                    w.columns[ci].windows[ri] = id;
                    w.focused_column = ci;
                    w.focused_row = ri;
                } else if matches!(action, Action::Move { .. }) && ci == source {
                    w.columns[source].windows.remove(row);
                    w.columns[source].windows.insert(ri, id);
                    w.focused_row = ri;
                } else {
                    let width = w.columns[source].width;
                    w.columns[source].windows.remove(row);
                    if w.columns[source].windows.is_empty() {
                        w.columns.remove(source);
                    }
                    let destination = w
                        .columns
                        .iter()
                        .position(|c| c.windows.contains(&target))
                        .unwrap();
                    if matches!(action, Action::Stack { .. }) {
                        w.columns[destination].windows.push(id);
                        for id in &w.columns[destination].windows {
                            self.row_weights.remove(id);
                        }
                        w.focused_column = destination;
                        w.focused_row = w.columns[destination].windows.len() - 1;
                    } else {
                        let at = destination + usize::from(ci > source);
                        w.columns.insert(
                            at,
                            Column {
                                width,
                                normal_width: None,
                                windows: vec![id],
                            },
                        );
                        self.row_weights.remove(&id);
                        w.focused_column = at;
                        w.focused_row = 0;
                    }
                }
                w.reveal_focus(viewport, &self.settings);
            }
            Action::Balance {} => {
                for c in &w.columns {
                    for id in &c.windows {
                        self.row_weights.remove(id);
                    }
                }
            }
            Action::Center {} => {
                w.centered = true;
                let c = w
                    .columns
                    .get(w.focused_column)
                    .ok_or(LayoutError::NoFocusedWindow)?;
                w.scroll.target = w.column_x(w.focused_column, self.settings.gap)
                    + (c.width - viewport.width) / 2.0
                    + self.settings.center_offset();
                let (min, max) = w.scroll_bounds(viewport, &self.settings);
                w.scroll.target = w.scroll.target.clamp(min, max);
            }
            Action::Focus { .. }
            | Action::ResizeBy { .. }
            | Action::ResizeRatio { .. }
            | Action::ResizeHeight { .. }
            | Action::ResizeHeightBy { .. } => unreachable!(),
            Action::FocusColumn { direction } => {
                w.focused_column = match direction {
                    Direction::Previous => w.focused_column.saturating_sub(1),
                    Direction::Next => {
                        (w.focused_column + 1).min(w.columns.len().saturating_sub(1))
                    }
                };
                w.focused_row = 0;
                w.reveal_focus(viewport, &self.settings);
            }
            Action::FocusStack { direction } => {
                let count = w
                    .columns
                    .get(w.focused_column)
                    .map_or(0, |c| c.windows.len());
                w.focused_row = match direction {
                    Direction::Previous => w.focused_row.saturating_sub(1),
                    Direction::Next => (w.focused_row + 1).min(count.saturating_sub(1)),
                };
            }
            Action::Scroll { delta } => {
                if !delta.is_finite() {
                    return Err(LayoutError::InvalidGeometry);
                }
                w.scroll.position = (w.scroll.position + delta).clamp(
                    w.scroll_bounds(viewport, &self.settings).0,
                    w.scroll_bounds(viewport, &self.settings).1,
                );
                w.scroll.target = w.scroll.position;
                w.scroll.velocity = 0.0;
            }
            Action::Resize { width } => {
                if !width.is_finite() || !(100.0..=10000.0).contains(width) {
                    return Err(LayoutError::InvalidGeometry);
                }
                let column = w
                    .columns
                    .get_mut(w.focused_column)
                    .ok_or(LayoutError::NoFocusedWindow)?;
                column.width = *width;
                column.normal_width = None;
                w.reveal_focus(viewport, &self.settings);
            }
            Action::ToggleFullWidth {} | Action::CycleWidth {} => {
                let full =
                    (viewport.width - self.settings.left_margin() - self.settings.right_margin())
                        .clamp(100.0, 10000.0);
                let column = w
                    .columns
                    .get_mut(w.focused_column)
                    .ok_or(LayoutError::NoFocusedWindow)?;
                if matches!(action, Action::ToggleFullWidth {}) {
                    column.width = if let Some(width) = column.normal_width.take() {
                        width
                    } else {
                        column.normal_width = Some(column.width);
                        full
                    };
                } else {
                    let widths = [full * 0.5, full * (2.0 / 3.0), full];
                    column.width = widths
                        .into_iter()
                        .map(|width| width.max(100.0))
                        .find(|width| *width > column.width + 2.0)
                        .unwrap_or((full * 0.5).max(100.0));
                    column.normal_width = None;
                }
                w.reveal_focus(viewport, &self.settings);
            }
            Action::MirrorColumns {} => {
                if w.columns.is_empty() {
                    return Err(LayoutError::NoFocusedWindow);
                }
                w.columns.reverse();
                w.focused_column = w.columns.len() - 1 - w.focused_column;
                w.reveal_focus(viewport, &self.settings);
            }
            Action::MirrorRows {} => {
                let column = w
                    .columns
                    .get(w.focused_column)
                    .ok_or(LayoutError::NoFocusedWindow)?;
                w.focused_row = column.windows.len() - 1 - w.focused_row;
                for column in &mut w.columns {
                    column.windows.reverse();
                }
                w.reveal_focus(viewport, &self.settings);
            }
            Action::StackLeft {} => {
                if w.focused_column > 0 {
                    let id = w.focused_window().ok_or(LayoutError::NoFocusedWindow)?;
                    let source = w.focused_column;
                    w.columns[source].windows.remove(w.focused_row);
                    w.columns[source - 1].windows.push(id);
                    for id in &w.columns[source - 1].windows {
                        self.row_weights.remove(id);
                    }
                    if w.columns[source].windows.is_empty() {
                        w.columns.remove(source);
                    }
                    w.focused_column = source - 1;
                    w.focused_row = w.columns[source - 1].windows.len() - 1;
                    w.reveal_focus(viewport, &self.settings);
                }
            }
            Action::Unstack {} => {
                let source = w.focused_column;
                let c = w
                    .columns
                    .get_mut(source)
                    .ok_or(LayoutError::NoFocusedWindow)?;
                if c.windows.len() > 1 {
                    let id = c.windows.remove(w.focused_row);
                    self.row_weights.remove(&id);
                    let width = c.width;
                    w.columns.insert(
                        source + 1,
                        Column {
                            width,
                            normal_width: None,
                            windows: vec![id],
                        },
                    );
                    w.focused_column = source + 1;
                    w.focused_row = 0;
                    w.reveal_focus(viewport, &self.settings);
                }
            }
        }
        w.remember_focus();
        Ok(())
    }
    pub fn tick(&mut self, dt: f64) {
        for m in self.monitors.values_mut().filter(|m| !m.suspended) {
            let viewport = m.viewport;
            let s = m.layout_mut();
            s.scroll.advance(dt, self.settings.animation_frequency);
            let (min, max) = s.scroll_bounds(viewport, &self.settings);
            s.scroll.clamp(min, max);
        }
    }
    pub fn focused_window(&self, monitor: &str) -> Option<WindowId> {
        self.monitors.get(monitor)?.layout().focused_window()
    }
    pub fn placements(&self) -> Vec<Placement> {
        let mut placements = Vec::new();
        for m in self.monitors.values() {
            for s in m.contexts.values() {
                let mut x = m.viewport.x - s.scroll.position;
                for (ci, c) in s.columns.iter().enumerate() {
                    let available = (m.viewport.height
                        - self.settings.top_margin()
                        - self.settings.bottom_margin()
                        - self.settings.gap * c.windows.len().saturating_sub(1) as f64)
                        .max(1.0);
                    let total: f64 = c
                        .windows
                        .iter()
                        .map(|id| self.row_weights.get(id).copied().unwrap_or(1.0))
                        .sum();
                    let mut y = m.viewport.y + self.settings.top_margin();
                    for (ri, id) in c.windows.iter().enumerate() {
                        let height =
                            available * self.row_weights.get(id).copied().unwrap_or(1.0) / total;
                        let frame = Rect {
                            x,
                            y,
                            width: c.width,
                            height,
                        };
                        placements.push(Placement {
                            window: *id,
                            monitor: m.id.clone(),
                            native_space: s.id,
                            frame,
                            clip: frame.intersection(m.viewport),
                            focused: !m.suspended
                                && s.id == m.native_space
                                && ci == s.focused_column
                                && ri == s.focused_row,
                        });
                        y += height + self.settings.gap;
                    }
                    x += c.width + self.settings.gap;
                }
            }
        }
        placements
    }
}

#[cfg(test)]
mod tests;
