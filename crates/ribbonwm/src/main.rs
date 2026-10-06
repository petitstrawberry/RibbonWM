mod compat;
mod daemon;
mod ipc;
mod modes;
mod service;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use ribbon_core::{Action, Settings, WindowId};
use serde_json::json;
use std::path::PathBuf;

#[derive(Parser)]
#[command(version, about = "RibbonWM — scrollable tiling for macOS")]
struct Cli {
    #[arg(
        long,
        global = true,
        help = "Monitor UUID; default is the monitor under the pointer"
    )]
    monitor: Option<String>,
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    /// Internal, windowless host for a live permission check.
    #[command(hide = true)]
    PermissionHost,
    /// Login service: wait for permissions/backend, then manage the desktop.
    Service {
        #[arg(long)]
        user: String,
        #[arg(long)]
        config: PathBuf,
        #[arg(long = "exclude-app")]
        exclude_apps: Vec<String>,
    },
    /// Leave or rejoin tiling. Float and sticky flags are independent.
    Float {
        #[arg(default_value = "toggle")]
        state: String,
        #[arg(long)]
        window: Option<u32>,
    },
    /// Show across native Spaces on the owning display, outside tiling.
    Sticky {
        #[arg(default_value = "toggle")]
        state: String,
        #[arg(long)]
        window: Option<u32>,
    },
    /// Permission, display context and optional Dock backend diagnostics. Does not prompt.
    Doctor,
    /// Ask macOS for Accessibility permission for this process. Approval is asynchronous.
    RequestPermissions {
        #[arg(long, help = "Also request Input Monitoring for trackpad gestures")]
        input_monitoring: bool,
    },
    /// Observe trackpad packets without consuming gestures or moving windows.
    GestureMonitor {
        #[arg(long, default_value_t = 15.0)]
        seconds: f64,
        #[arg(long, default_value_t = 3)]
        fingers: u32,
    },
    /// Inspect all WindowServer windows. Titles may be empty without Screen Recording.
    Windows,
    /// Running regular apps, without querying Accessibility elements.
    Applications,
    Displays,
    /// Interactive Rust-driven native-window demo. Arrows navigate; Esc exits.
    Demo {
        #[arg(long)]
        test: bool,
        #[arg(long, default_value_t = 0.0)]
        seconds: f64,
        #[arg(
            long,
            default_value_t = 40.0,
            help = "Offset from the screen's left edge, in points"
        )]
        left: f64,
        #[arg(
            long,
            default_value_t = 1050.0,
            help = "Maximum demo width, in points (at least 548)"
        )]
        width: f64,
    },
    /// Start the daemon with explicitly selected windows or --all.
    Run {
        #[arg(long, value_delimiter = ',', conflicts_with = "all")]
        windows: Vec<u32>,
        #[arg(long)]
        all: bool,
        #[arg(
            long = "exclude-app",
            value_delimiter = ',',
            help = "Exclude bundle ID or app name; trailing * matches a prefix"
        )]
        exclude_apps: Vec<String>,
        #[arg(long, help = "Run layout and IPC without changing any app window")]
        dry_run: bool,
        #[arg(long)]
        config: Option<PathBuf>,
    },
    Status,
    Focus {
        /// left/right/up/down, prev/next/first/last/recent, stack.* or window ID.
        target: String,
    },
    FocusWindow {
        window: u32,
    },
    FocusMonitor {
        target: String,
    },
    Scroll {
        #[arg(allow_hyphen_values = true)]
        delta: f64,
    },
    Resize {
        #[arg(required_unless_present_any=["by","ratio"],conflicts_with_all=["by","ratio"])]
        width: Option<f64>,
        #[arg(long, allow_hyphen_values = true, conflicts_with = "ratio")]
        by: Option<f64>,
        #[arg(long)]
        ratio: Option<f64>,
    },
    ResizeHeight {
        #[arg(required_unless_present = "by", conflicts_with = "by")]
        height: Option<f64>,
        #[arg(long, allow_hyphen_values = true)]
        by: Option<f64>,
    },
    Move {
        target: String,
    },
    Swap {
        target: String,
    },
    Stack {
        #[arg(default_value = "left")]
        target: String,
    },
    Unstack,
    Balance,
    Center,
    /// Toggle the focused column between its own width and the usable monitor width.
    ToggleFullWidth,
    /// Cycle the focused column through half, two-thirds and full usable width.
    CycleWidth,
    /// Reverse columns or stack rows while keeping the same focused window.
    Mirror {
        axis: String,
    },
    /// Read or update a running session's spacing. Values are logical points.
    Config {
        name: String,
        value: Option<f64>,
    },
    Quit,
    BackendStatus,
}
fn print(value: impl serde::Serialize) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}
fn doctor() -> Result<()> {
    let backend = match ribbon_macos::backend::Backend::connect() {
        Ok(b) => json!({"available":true,"status":b.status()?}),
        Err(e) => json!({"available":false,"reason":e.to_string()}),
    };
    print(
        json!({"name":"RibbonWM","version":env!("CARGO_PKG_VERSION"),"executable":std::env::current_exe()?,
        "expected_backend":ribbon_macos::backend::expected_build()?,"accessibility":ribbon_macos::accessibility_trusted(),
        "input_monitoring":ribbon_macos::input::trusted(),
        "displays":ribbon_macos::displays()?,"backend":backend,
        "workspace_policy":"one horizontal scroll layout per native macOS Space and monitor",
        "live_verification":"See docs/verification.md for actual runtime coverage"}),
    )
}
fn execute(cli: Cli) -> Result<()> {
    let float_command = matches!(&cli.command, Command::Float { .. });
    let action = match cli.command {
        Command::Service {
            user,
            config,
            exclude_apps,
        } => {
            return service::run(user, config, exclude_apps);
        }
        Command::Float { state, window } | Command::Sticky { state, window } => {
            let kind = if float_command {
                modes::ModeKind::Float
            } else {
                modes::ModeKind::Sticky
            };
            let enabled = match state.as_str() {
                "on" => Some(true),
                "off" => Some(false),
                "toggle" => None,
                _ => anyhow::bail!("Expected on, off or toggle"),
            };
            return print(ipc::send(ipc::Request::SetMode {
                window: window.map(WindowId),
                kind,
                enabled,
            })?);
        }
        Command::Doctor => return doctor(),
        Command::GestureMonitor { seconds, fingers } => {
            return ribbon_macos::input::monitor(seconds, fingers);
        }
        Command::RequestPermissions { input_monitoring } => {
            let requested = !ribbon_macos::accessibility_trusted();
            let accessibility = if requested {
                ribbon_macos::request_accessibility_permission()
            } else {
                true
            };
            let input_requested = input_monitoring && !ribbon_macos::input::trusted();
            if input_requested {
                ribbon_macos::input::request_permission();
            }
            return print(json!({
                "accessibility": accessibility,
                "prompt_requested": requested,
                "input_monitoring": ribbon_macos::input::trusted(),
                "input_prompt_requested": input_requested,
                "executable": std::env::current_exe()?,
            }));
        }
        Command::PermissionHost => return service::permission_host(),
        Command::Windows => return print(ribbon_macos::windows()?),
        Command::Applications => return print(ribbon_macos::applications()?),
        Command::Displays => return print(ribbon_macos::displays()?),
        Command::Demo {
            test,
            seconds,
            left,
            width,
        } => return ribbon_macos::demo::run(test, seconds, left, width),
        Command::BackendStatus => {
            return print(ribbon_macos::backend::Backend::connect()?.status()?);
        }
        Command::Run {
            windows,
            all,
            exclude_apps,
            dry_run,
            config,
        } => {
            let settings = match config {
                Some(file) => toml::from_str::<Settings>(
                    &std::fs::read_to_string(&file)
                        .with_context(|| format!("Reading {}", file.display()))?,
                )?,
                None => Settings::default(),
            };
            return daemon::run(daemon::Options {
                selected: windows,
                all,
                exclude_apps,
                dry_run,
                settings,
            });
        }
        Command::Status => return print(ipc::send(ipc::Request::Status {})?),
        Command::Quit => return print(ipc::send(ipc::Request::Quit {})?),
        Command::FocusWindow { window } => {
            return print(ipc::send(ipc::Request::FocusWindow {
                window: WindowId(window),
            })?);
        }
        Command::FocusMonitor { target } => {
            return print(ipc::send(ipc::Request::FocusMonitor { target })?);
        }
        Command::Focus { target } => {
            let target = compat::target(&target)?;
            if let ribbon_core::WindowTarget::Id(window) = target {
                return print(ipc::send(ipc::Request::FocusWindow { window })?);
            }
            Action::Focus { target }
        }
        Command::Scroll { delta } => Action::Scroll { delta },
        Command::ToggleFullWidth => Action::ToggleFullWidth {},
        Command::CycleWidth => Action::CycleWidth {},
        Command::Mirror { axis } => match axis.as_str() {
            "columns" => Action::MirrorColumns {},
            "rows" => Action::MirrorRows {},
            _ => anyhow::bail!("Expected columns or rows"),
        },
        Command::Resize { width, by, ratio } => {
            if let Some(width) = width {
                Action::Resize { width }
            } else if let Some(delta) = by {
                Action::ResizeBy { delta }
            } else {
                Action::ResizeRatio {
                    ratio: ratio.context("Missing resize value")?,
                }
            }
        }
        Command::ResizeHeight { height, by } => {
            if let Some(height) = height {
                Action::ResizeHeight { height }
            } else {
                Action::ResizeHeightBy {
                    delta: by.context("Missing height value")?,
                }
            }
        }
        Command::Move { target } => Action::Move {
            target: compat::target(&target)?,
        },
        Command::Swap { target } => Action::Swap {
            target: compat::target(&target)?,
        },
        Command::Stack { target } => Action::Stack {
            target: compat::target(&target)?,
        },
        Command::Unstack => Action::Unstack {},
        Command::Balance => Action::Balance {},
        Command::Center => Action::Center {},
        Command::Config { name, value } => {
            return print(ipc::send(ipc::Request::Configure { name, value })?["value"].clone());
        }
    };
    print(ipc::send(ipc::Request::Apply {
        monitor: cli.monitor,
        action,
    })?)
}
fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = if args
        .first()
        .is_some_and(|s| s == "-m" || s == "--message" || s == "yabai")
    {
        compat::execute(&args)
    } else {
        execute(Cli::parse())
    };
    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("RibbonWM: {e:#}");
            std::process::ExitCode::FAILURE
        }
    }
}
