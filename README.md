# RibbonWM

Rust製のmacOS向けスクロール型ウィンドウマネージャー。PaperWM / Niriのように
ウィンドウを横方向の列へ並べ、縦スタックと滑らかなスクロールで操作します。
レイアウトとスクロール位置は**ネイティブmacOS Space × モニター**ごとに保持します。
Space切り替えや未管理アプリからの復帰による自動フォーカスでは、保存した
スクロール位置を動かしません。クリックとWMのフォーカスコマンドでは選択した
列を表示します。リサイズ追従には変形前のウィンドウサイズを使います。

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
        animation_curve = "ease_out";
        animation_duration = 0.10;
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

サービスは権限待ちでも終了せず、ネイティブのイベントを処理しながら待ちます。
許可後は同じプロセスで管理を開始します。AXの信頼状態が古い場合は、画面や
ウィンドウを出さない専用の子プロセスに対する実際のAX読み取りで確認します。
ジェスチャー用のイベントタップも作成を再試行し、後からの許可に追従します。
アニメーションの秒数は `ribbonwm config animation_duration 0.10` で実行中にも
変更できます。`0` で即時移動です。永続設定はNix/TOMLへ書いてください。

## トラックパッドの横スクロール（実験的）

Nix の `services.ribbonwm.settings` または TOML で設定します。
修飾キーを省略するとキーなしです。標準では無効で、指の本数は3本です。

```toml
gesture_scroll = true
gesture_fingers = 2        # 2 / 3 / 4
gesture_modifier = "alt"   # 推奨: Option＋2本指。alt / ctrl / super / shift
gesture_sensitivity = 1.0 # 0.1〜5.0
gesture_reverse = false
gesture_momentum = true
```

通常のアプリの横スクロールと競合させないため、Option＋2本指を推奨します。
修飾キーは開始時に判定し、指やキーを離した後のmacOSの慣性も同じスクロールへ
引き継ぎます。キーなしにする場合は `gesture_modifier` を省略してください。

2本指モードはmacOSの精密スクロールイベントを使います。イベントに指の本数は
入らないため、Magic Mouse等の同じ精密スクロール形式も対象になります。
3・4本指モードはタッチのidentityを追跡し、指が動いている間は直接追従します。
横方向の意図が確定するまでは入力を通し、縦方向・指定外の本数・通常のホイールは
通します。開始したモニターとSpaceを固定し、Space変更・画面構成変更・
マウスドラッグで中断します。除外アプリが前面の間とネイティブ全画面では無効です。

慣性はmacOSの `NSEvent.momentumPhase` が届いた場合だけ使い、独自の減速を
重ねません。手元の3本指計測では慣性イベントが届かず、2本指では届きました。
4本指は未確認です。
届かなければ指を離した時点で止まります。無効にした場合は捕捉した慣性を
アプリへ漏らさず捨てます。

同じ指の本数をSpacesの横移動にも割り当てると競合します。例えばRibbonWMを
3本、macOSの「フルスクリーンアプリケーション間をスワイプ」を4本に分けます。
OS側の設定は自動変更しません。2本指モードはアプリの横スクロール／ページ移動と
競合し得ます。縦のMission ControlやSpacesとの共存も物理ジェスチャーでの確認が必要です。

有効にしたサービスはプログラムから「入力監視」の許可を要求します。
未許可でもキーボード操作によるWMは起動し、許可後にジェスチャーだけ再試行します。
明示的な要求は `ribbonwm request-permissions --input-monitoring`、状態は `doctor`。
設定変更はサービス再起動で反映します。

`ribbonwm gesture-monitor --seconds 30 --fingers 2` は主画面へ計測ウィンドウを出します。
「計測開始」を押してから記録し、結果は閉じるまで表示します。入力を消費せず、
配置も変えずにtouch / scroll のphase、momentum、接触数、横スクロール判定を
JSON行で記録します。`--fingers 3` / `4` も指定できます。
これは3・4本指でネイティブ慣性が届くかを調べるための観測用コマンドです。

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
| `quit` | 使える画面内の位置へ管理を解除して正常終了 |

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
floatは現在表示されている位置とサイズで管理を解除します。画面外・一部だけ
表示された列は作業領域の内側へ戻します。正常終了も同じ扱いで、起動時の
古い配置へは戻しません。stickyは通常レベルです。sticky解除後はmacOSが決めた
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
