{ ribbonwmPackage }:
{ config, lib, pkgs, ... }:
let
  cfg = config.services.ribbonwm;
  format = pkgs.formats.toml { };
  defaultSettings = {
    column_width = 960.0; gap = 16.0;
    horizontal_margin = 20.0; vertical_margin = 20.0;
    preserve_window_width = true; center_content = true;
    focus_alignment = "visible";
  };
  configFile = format.generate "ribbonwm.toml" (defaultSettings // cfg.settings);
in {
  options.services.ribbonwm = {
    enable = lib.mkEnableOption "RibbonWM scrolling window manager";
    package = lib.mkOption { type = lib.types.package; default = ribbonwmPackage; };
    user = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = config.system.primaryUser or null;
      description = "Login user whose desktop RibbonWM manages.";
    };
    settings = lib.mkOption {
      type = format.type;
      default = { };
      description = "RibbonWM settings written to an immutable TOML file.";
    };
    excludeApps = lib.mkOption {
      type = lib.types.listOf lib.types.str;
      default = [ "com.openai.*" "ChatGPT*" ];
      description = "Bundle IDs or app names excluded before Accessibility queries.";
    };
    enableDockInjection = lib.mkEnableOption "root Dock backend loader, including Dock restart recovery (requires relaxed SIP)";
  };
  config = lib.mkIf cfg.enable {
    assertions = [
      { assertion = cfg.user != null && cfg.user != "root"; message = "services.ribbonwm.user must be a non-root login user."; }
      { assertion = !(config.services.yabai.enable or false); message = "Disable services.yabai before enabling RibbonWM."; }
      { assertion = !cfg.enableDockInjection || pkgs.stdenv.hostPlatform.isAarch64; message = "Automatic RibbonWM Dock injection currently supports Apple Silicon only."; }
    ];
    environment.systemPackages = [ cfg.package ];
    launchd.user.agents.ribbonwm = {
      managedBy = "services.ribbonwm.enable";
      serviceConfig = {
        ProgramArguments = [ "${cfg.package}/bin/ribbonwm" "service" "--user" (if cfg.user == null then "" else cfg.user) "--config" "${configFile}" ]
          ++ lib.concatMap (app: [ "--exclude-app" app ]) cfg.excludeApps;
        RunAtLoad = true;
        KeepAlive.SuccessfulExit = false;
        ThrottleInterval = 10;
        ExitTimeOut = 15;
        LimitLoadToSessionType = "Aqua";
        ProcessType = "Interactive";
      };
    };
    launchd.daemons.ribbonwm-backend = lib.mkIf cfg.enableDockInjection {
      serviceConfig = {
        ProgramArguments = [ "${cfg.package}/bin/ribbonwm-load-backend" "--watch" "--user" (if cfg.user == null then "" else cfg.user) ];
        RunAtLoad = true;
        KeepAlive = true;
        ThrottleInterval = 10;
        StandardOutPath = "/var/log/ribbonwm-backend.log";
        StandardErrorPath = "/var/log/ribbonwm-backend.log";
      };
    };
  };
}
