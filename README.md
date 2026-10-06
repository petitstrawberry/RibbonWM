# RibbonWM

Rust製のmacOS向けスクロール型ウィンドウマネージャー。PaperWM / Niriのように
ウィンドウを横方向の列へ並べ、縦スタックと滑らかなスクロールで操作します。
レイアウトとスクロール位置は**ネイティブmacOS Space × モニター**ごとに保持します。

実ウィンドウのWindowServer変換・クリップを使い、隣のモニターへの描画と入力の
はみ出しを抑えます。Rustがレイアウト・フォーカス・IPCを担当し、Dock内の小さな
バックエンドが特権操作を担当します。

現在は実験段階です。Apple Silicon / macOS 26.6.2で検証しています。
Dock注入には必要なSIP制限の解除が必要です。SIP設定はプログラムから変更しません。
[検証範囲と制限](docs/verification.md)を参照してください。

## Nixでビルド

```sh
git clone https://github.com/petitstrawberry/RibbonWM.git
cd RibbonWM
nix develop -c sh scripts/check.sh
nix build
./result/bin/ribbonwm doctor
```

Rust **1.91.1**・Clang **21**をflakeで固定しています。開発時もambient rustupを
使いません。`nix build`にはRust CLI・Dock loader・署名済みバックエンドを含みます。
ビルドだけではサービスを登録しません。

通常のWMと干渉しない、自前ウィンドウのデモ:

```sh
nix develop -c cargo run --locked -- demo
nix develop -c cargo run --locked -- demo --test
```

## nix-darwinサービス

システムflakeにinputを追加します。RibbonWM自身のビルド用nixpkgs/toolchainを
維持するため、ホストのnixpkgsへ `follows` する必要はありません。

```nix
inputs.ribbonwm.url = "github:petitstrawberry/RibbonWM";
```

`darwinSystem.modules`へ追加:

```nix
modules = [
  inputs.ribbonwm.darwinModules.default
  {
    services.yabai.enable = false;
    services.ribbonwm = {
      enable = true;
      user = "YOUR_LOGIN_NAME";
      enableDockInjection = true;
      settings = {
        padding_top = 24.0;
        padding_bottom = 24.0;
        padding_left = 24.0;
        padding_right = 24.0;
        gap = 6.0;
        preserve_window_width = true;
        center_content = true;
        focus_alignment = "visible";
        animation_curve = "ease_in_out";
        animation_duration = 0.25;
        cycle_width_ratios = [ 0.38195 0.5 0.61804 ];
        frame_rate = 120;
      };
      excludeApps = [
        "com.openai.*" "ChatGPT*"
        "com.apple.systempreferences" "com.apple.calculator"
        "Raycast" "CLIP STUDIO PAINT*"
      ];
    };
  }
];
```

システムflakeを更新・ビルドしてから `sudo darwin-rebuild switch --flake …` で反映します。
`rebuild`だけでは`flake.lock`に固定されたRibbonWMのrevisionは更新されません。
更新時は先にシステムflakeのディレクトリで`nix flake update ribbonwm`を実行します。
同時にyabaiを有効にするとmoduleのassertionで拒否します。

ユーザーLaunchAgent `org.nixos.ribbonwm` は `bin/ribbonwm service …` を直接起動します。
初回・パッケージ更新後に未許可なら、プログラムからmacOSへAccessibility許可を
要求し、承認を待ちます。`ribbonwm request-permissions` は明示的な要求、
`ribbonwm doctor` は画面を出さない確認です。ターミナルの許可はサービスと別です。
案内が出ない場合は待機ログにあるバイナリをmacOSのアクセシビリティ設定で許可します。

`enableDockInjection = true` はroot LaunchDaemon `org.nixos.ribbonwm-backend` を追加します。
指定ユーザーのDockを監視し、必要なときにパッケージのバックエンドをロードします。
実行中WMのleaseは奪いません。Dock再起動後の自動復旧はまだ実機未検証です。
サービスは同じパッケージのpayload build名を確認してから管理を開始します。
`doctor`には実行ファイルと期待するpayload名が出ます。待機理由はログで確認できます。

ログは `~/Library/Logs/RibbonWM/wm.log` と `/var/log/ribbonwm-backend.log`。
`ribbonwm quit` は正常終了し、そのまま停止します。異常終了はlaunchdが再起動します。

## 手動live起動

既存WMを停止して、バックエンドをロードします。初回はsudo認証が必要です。

```sh
nix develop -c sh scripts/load-backend.sh
nix develop -c cargo run --locked -- run --all --config config/live.toml \
  --exclude-app 'com.openai.*' --exclude-app 'ChatGPT*' \
  --exclude-app com.apple.systempreferences
```

`--windows ID,ID` で対象を限定できます。`--dry-run` は配置・IPCだけを動かし、
アプリのサイズやWindowServerの変換を変更しません。

## 操作

| コマンド | 動作 |
| --- | --- |
| `focus left/right/up/down` | 隣の列・スタック行を選択 |
| `focus prev/next/first/last/recent/largest/smallest/mouse/ID` | 順序・履歴・サイズ・ポインター・IDで選択 |
| `focus stack.next/stack.prev/stack.first/stack.last/stack.N` | スタック内を選択（Nは1始まり） |
| `focus-monitor east/west/north/south/next/prev/N/UUID` | モニターの選択ウィンドウへ移動 |
| `move SELECTOR` / `swap SELECTOR` | 同じSpace/モニター内で移動・交換 |
| `stack SELECTOR` / `unstack` / `balance` | 縦スタック・分離・高さ等分割 |
| `resize WIDTH` / `resize --by DELTA` / `resize --ratio RATIO` | 列幅を変更 |
| `resize-height HEIGHT` / `resize-height --by DELTA` | スタック行の高さを変更 |
| `toggle-full-width` | 元の列幅とモニターの使用可能な全幅を往復 |
| `cycle-width` | `cycle_width_ratios` の幅を循環（service既定は約38%・50%・62%） |
| `mirror columns` / `mirror rows` | 列順・スタック行順を反転。選択ウィンドウを維持 |
| `center` / `scroll DELTA` | 選択列を中央へ表示・横スクロール |
| `float on/off/toggle` / `sticky on/off/toggle` | タイルから離す・モニター内のnative Spacesへ表示 |
| `config gap VALUE` / `config padding_top VALUE` | 実行中のスペーシングを変更 |
| `status` / `windows` / `displays` / `backend-status` | 状態を確認 |
| `quit` | 復元して正常終了 |

CLIの `--monitor UUID` で対象を指定できます。省略時はポインター下の画面です。
以下の互換構文は**キーボードフォーカスのある画面**を基準にします。

## yabai構文 / skhd

`ribbonwm -m …` と `ribbonwm yabai -m …` をサポートします。

```sh
ribbonwm -m window --focus east
ribbonwm -m window --warp west
ribbonwm -m window --swap next
ribbonwm -m window --toggle float
ribbonwm -m window --toggle sticky
ribbonwm -m window --toggle zoom-fullscreen
ribbonwm -m window --resize cycle
ribbonwm -m window --center
ribbonwm -m space --mirror y-axis
ribbonwm -m window --focus stack.next
```

`--toggle zoom-fullscreen` は列の全幅トグル、`space --mirror y-axis` は列順、
`x-axis` はスタック行順の反転です。`--center` と `--resize cycle` はRibbonWMの拡張です。
ほかに `--stack`、`--resize edge:dx:dy`、`space --balance`、`display --focus`、
`config *_padding/window_gap`、`query --windows/--displays` を扱います。

BSPの親ズーム・90度回転、float時の `--grid`、モニター間ウィンドウ転送、
Space作成/削除、rule/signalなどの全機能互換はありません。未対応構文はエラーを返します。
floatは元の幾何へ戻し、stickyは通常レベルです。sticky解除後はmacOSが決めた
現在のSpaceへ再タイルします。最前面固定や元のSpace所属までの復元は行いません。

[skhd設定例](config/skhdrc)は方向操作のキー配置を維持し、BSP回転を幅循環、
親ズームを中央表示へ置き換えています。ネイティブfullscreenはskhdからアプリの
標準Ctrl+Cmd+Fショートカットを送ります。

## 構成とライセンス

- `ribbon-core`: OSに依存しないレイアウト、スクロール、選択・幅・スタック操作。
- `ribbon-macos`: AX/AppKitとDockバックエンドの薄い境界。
- `ribbonwm`: コマンド、IPC、ウィンドウ追跡、ライフサイクル、サービス。

[アーキテクチャ](docs/architecture.md) / [検証記録](docs/verification.md)。
MITライセンス。Mach loaderにはyabai由来のコードと元の著作権表示・帰属を保持し、
[元のライセンス](native/vendor/yabai-LICENSE.txt)を同梱しています。
PaperWMは挙動の参考で、Rust実装は独立しています。
