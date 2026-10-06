//! Explicit yabai message syntax adapter. Unsupported semantics fail closed.
use crate::ipc::{self, Request};
use anyhow::{Context, Result, bail};
use ribbon_core::{Action, WindowId, WindowTarget};
use serde_json::{Value, json};

pub fn target(value: &str) -> Result<WindowTarget> {
    Ok(match value {
        "left" | "west" => WindowTarget::Left,
        "right" | "east" => WindowTarget::Right,
        "up" | "north" => WindowTarget::Up,
        "down" | "south" => WindowTarget::Down,
        "prev" | "previous" => WindowTarget::Previous,
        "next" => WindowTarget::Next,
        "first" => WindowTarget::First,
        "last" => WindowTarget::Last,
        "recent" => WindowTarget::Recent,
        "largest" => WindowTarget::Largest,
        "smallest" => WindowTarget::Smallest,
        "mouse" => {
            let (x, y) = ribbon_macos::pointer();
            WindowTarget::Point { x, y }
        }
        "stack.prev" => WindowTarget::StackPrevious,
        "stack.next" => WindowTarget::StackNext,
        "stack.first" => WindowTarget::StackFirst,
        "stack.last" => WindowTarget::StackLast,
        "stack.recent" => WindowTarget::StackRecent,
        v if v.starts_with("stack.") => {
            let index = v[6..]
                .parse::<usize>()
                .context("Expected a one-based stack index")?;
            if index == 0 {
                bail!("Stack indices start at 1");
            }
            WindowTarget::StackIndex(index)
        }
        v => {
            let id = v.parse::<u32>().context("Unknown window selector")?;
            if id == 0 {
                bail!("Window ID must be positive");
            }
            WindowTarget::Id(WindowId(id))
        }
    })
}

enum Command {
    Request(Request),
    Windows,
    Displays,
}
fn parse(args: &[String]) -> Result<Command> {
    let mut args = args;
    if args.first().is_some_and(|s| s == "yabai") {
        args = &args[1..];
    }
    if args.first().is_some_and(|s| s == "-m" || s == "--message") {
        args = &args[1..];
    }
    let words: Vec<_> = args.iter().map(String::as_str).collect();
    let request = match words.as_slice() {
        ["config", name] | ["config", name, _] => {
            let name = match *name {
                "window_gap" => "gap",
                "top_padding" => "padding_top",
                "bottom_padding" => "padding_bottom",
                "left_padding" => "padding_left",
                "right_padding" => "padding_right",
                _ => bail!("Unsupported yabai config: {name}"),
            };
            let value = words
                .get(2)
                .map(|s| s.parse::<f64>())
                .transpose()
                .context("Expected numeric spacing")?;
            Request::Configure {
                name: name.into(),
                value,
            }
        }
        ["query", "--windows"] => return Ok(Command::Windows),
        ["query", "--displays"] => return Ok(Command::Displays),
        ["display", "--focus", selector] => Request::FocusMonitor {
            target: (*selector).into(),
        },
        ["window", "--center"] => Request::Apply {
            monitor: None,
            action: Action::Center {},
        },
        ["window", "--toggle", "zoom-fullscreen"] => Request::Apply {
            monitor: None,
            action: Action::ToggleFullWidth {},
        },
        ["window", "--resize", "cycle"] => Request::Apply {
            monitor: None,
            action: Action::CycleWidth {},
        },
        ["space", "--mirror", axis] => Request::Apply {
            monitor: None,
            action: match *axis {
                "y-axis" => Action::MirrorColumns {},
                "x-axis" => Action::MirrorRows {},
                _ => bail!("Expected x-axis or y-axis"),
            },
        },
        ["window", "--toggle", "float"] => Request::SetMode {
            window: None,
            kind: crate::modes::ModeKind::Float,
            enabled: None,
        },
        ["window", "--toggle", "sticky"] => Request::SetMode {
            window: None,
            kind: crate::modes::ModeKind::Sticky,
            enabled: None,
        },
        ["window", operation, selector] => {
            let action = match *operation {
                "--focus" => {
                    let selected = target(selector)?;
                    if let WindowTarget::Id(window) = selected {
                        return Ok(Command::Request(Request::FocusWindow { window }));
                    }
                    Action::Focus { target: selected }
                }
                "--swap" => Action::Swap {
                    target: target(selector)?,
                },
                "--warp" => Action::Move {
                    target: target(selector)?,
                },
                "--stack" => Action::Stack {
                    target: target(selector)?,
                },
                "--resize" => resize(selector)?,
                _ => bail!("Unsupported yabai window operation: {operation}"),
            };
            Request::Apply {
                monitor: None,
                action,
            }
        }
        ["space", "--balance"] => Request::Apply {
            monitor: None,
            action: Action::Balance {},
        },
        _ => bail!("Unsupported yabai syntax; see README.md's compatibility table"),
    };
    Ok(Command::Request(request))
}
fn resize(value: &str) -> Result<Action> {
    let parts: Vec<_> = value.split(':').collect();
    let [edge, x, y] = parts.as_slice() else {
        bail!("Resize requires edge:dx:dy");
    };
    let x = x.parse::<f64>().context("Invalid resize x")?;
    let y = y.parse::<f64>().context("Invalid resize y")?;
    if !x.is_finite() || !y.is_finite() {
        bail!("Resize must be finite");
    }
    Ok(match *edge {
        "left" if y == 0.0 => Action::ResizeBy { delta: -x },
        "right" if y == 0.0 => Action::ResizeBy { delta: x },
        "top" if x == 0.0 => Action::ResizeHeightBy { delta: -y },
        "bottom" if x == 0.0 => Action::ResizeHeightBy { delta: y },
        "abs" if y == 0.0 => Action::Resize { width: x },
        _ => {
            bail!("This resize edge is unsupported; resize a column width or a stacked row height")
        }
    })
}
pub fn execute(args: &[String]) -> Result<()> {
    let value = match parse(args)? {
        Command::Request(mut request) => {
            if let Request::Apply { monitor, .. } = &mut request {
                // yabai's window commands are relative to keyboard focus,
                // even when the pointer is left on another monitor.
                let status = ipc::send(Request::Status {})?;
                let id = status["native_focused_window"]
                    .as_u64()
                    .context("No native focused managed window")?;
                *monitor = status["state"]["monitors"]
                    .as_object()
                    .context("Missing monitors")?
                    .iter()
                    .find_map(|(mid, m)| {
                        let sid = m["native_space"].to_string();
                        m["contexts"][&sid]["columns"]
                            .as_array()?
                            .iter()
                            .any(|c| {
                                c["windows"]
                                    .as_array()
                                    .is_some_and(|ids| ids.iter().any(|w| w.as_u64() == Some(id)))
                            })
                            .then(|| mid.clone())
                    });
                if monitor.is_none() {
                    bail!("Focused window is outside the active managed Space");
                }
            }
            let config = matches!(request, Request::Configure { .. });
            let value = ipc::send(request)?;
            if config {
                value["value"].clone()
            } else {
                value
            }
        }
        Command::Displays => {
            let status = ipc::send(Request::Status {})?;
            let focused = status["native_focused_window"].as_u64();
            let rows:Vec<_>=ribbon_macos::displays()?.into_iter().enumerate().map(|(i,d)| {
                let has_focus=focused.is_some_and(|id| status["placements"].as_array().is_some_and(|rows| rows.iter().any(|p| p["window"]==id && p["monitor"]==d.id)));
                json!({"id":d.display_id,"uuid":d.id,"index":i+1,"frame":d.frame,"spaces":[d.native_space],"has-focus":has_focus})
            }).collect();
            Value::Array(rows)
        }
        Command::Windows => {
            let status = ipc::send(Request::Status {})?;
            let originals = status["original_geometry"]
                .as_object()
                .context("Missing managed geometry")?;
            let inventory = ribbon_macos::windows()?;
            let mut rows = Vec::new();
            for w in inventory {
                if originals
                    .get(&w.id.0.to_string())
                    .is_none_or(|v| v["pid"] != w.pid)
                {
                    continue;
                }
                let placement = status["placements"]
                    .as_array()
                    .and_then(|rows| rows.iter().find(|p| p["window"] == w.id.0));
                let mode = &status["window_modes"][w.id.0.to_string()];
                let frame = placement
                    .map(|p| p["frame"].clone())
                    .unwrap_or(serde_json::to_value(w.bounds)?);
                rows.push(
                    json!({"id":w.id,"pid":w.pid,"app":w.app,"title":w.title,"frame":frame,
                    "has-focus":status["native_focused_window"]==w.id.0,"is-floating":mode["floating"],"is-sticky":mode["sticky"],"is-tiled":placement.is_some()}),
                );
            }
            Value::Array(rows)
        }
    };
    crate::print(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn words(s: &str) -> Vec<String> {
        s.split_whitespace().map(str::to_owned).collect()
    }
    #[test]
    fn selectors_and_message_prefixes_share_the_core_actions() {
        for command in [
            "-m window --focus east",
            "--message window --focus east",
            "yabai -m window --focus east",
        ] {
            assert!(matches!(
                parse(&words(command)).unwrap(),
                Command::Request(Request::Apply {
                    action: Action::Focus {
                        target: WindowTarget::Right
                    },
                    ..
                })
            ));
        }
        assert!(matches!(
            parse(&words("-m window --focus 42")).unwrap(),
            Command::Request(Request::FocusWindow {
                window: WindowId(42)
            })
        ));
        assert!(matches!(
            target("stack.2").unwrap(),
            WindowTarget::StackIndex(2)
        ));
        assert!(target("stack.0").is_err());
    }
    #[test]
    fn spacing_maps_to_live_configuration_and_resize_preserves_edge_signs() {
        assert!(
            matches!(parse(&words("-m config top_padding 30")).unwrap(),Command::Request(Request::Configure { name,value:Some(30.0) }) if name=="padding_top")
        );
        assert!(matches!(
            resize("left:-80:0").unwrap(),
            Action::ResizeBy { delta: 80.0 }
        ));
        assert!(matches!(
            resize("bottom:0:50").unwrap(),
            Action::ResizeHeightBy { delta: 50.0 }
        ));
    }
    #[test]
    fn floating_and_sticky_toggles_use_tracked_window_modes() {
        for (command, kind) in [
            ("-m window --toggle float", crate::modes::ModeKind::Float),
            ("-m window --toggle sticky", crate::modes::ModeKind::Sticky),
        ] {
            let Command::Request(Request::SetMode {
                window: None,
                kind: actual,
                enabled: None,
            }) = parse(&words(command)).unwrap()
            else {
                panic!("Expected a focused-window toggle");
            };
            assert_eq!(
                std::mem::discriminant(&actual),
                std::mem::discriminant(&kind)
            );
        }
    }
    #[test]
    fn migrated_zoom_and_mirror_bindings_use_scroll_layout_actions() {
        for (syntax, expected) in [
            ("-m window --toggle zoom-fullscreen", "toggle_full_width"),
            ("-m window --resize cycle", "cycle_width"),
            ("-m window --center", "center"),
            ("-m space --mirror x-axis", "mirror_rows"),
            ("-m space --mirror y-axis", "mirror_columns"),
        ] {
            let Command::Request(Request::Apply { action, .. }) = parse(&words(syntax)).unwrap()
            else {
                panic!("Expected a layout command");
            };
            assert_eq!(serde_json::to_value(action).unwrap()["action"], expected);
        }
        assert!(parse(&words("-m space --mirror diagonal")).is_err());
    }
    #[test]
    fn unsupported_or_ambiguous_commands_fail_before_any_ipc() {
        for command in [
            "-m window --toggle unsupported",
            "-m config layout bsp",
            "-m window --focus east extra",
            "-m window --resize right:NaN:0",
            "-m window --resize bottom_right:40:30",
            "-m space --focus next",
        ] {
            assert!(parse(&words(command)).is_err(), "{command}");
        }
    }
}
