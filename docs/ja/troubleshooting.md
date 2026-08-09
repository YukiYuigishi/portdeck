[English](../en/troubleshooting.md) | 日本語

# トラブルシューティング

OpenSSHの確認、portdeckのDEBUGログ、代表的な問題の切り分け方法を説明します。通常の操作については[使い方](usage.md)を参照してください。

## 最初に行う診断

portdeckが実行するOpenSSH Clientを確認します。

```console
portdeck --diagnose
ssh -V
```

`portdeck --diagnose`はPATH上の`ssh -V`を実行し、検出したOpenSSHのバージョンを表示して終了します。`portdeck --help`と`portdeck --version`も利用できます。

TUIでは`E`を押すと、直近のOpenSSH stderrまたは起動時診断を表示します。接続状態が古いように見える場合は`r`で専用ControlMasterへ`ssh -O check`を実行してください。

## DEBUGモード

障害を再現する際は、DEBUG levelの構造化ログを実行ごとのファイルへ保存できます。

```console
portdeck --debug
portdeck --debug --diagnose
```

ログの保存先は次のとおりです。

```text
$XDG_STATE_HOME/portdeck/debug-<timestamp>-<pid>.log
# XDG_STATE_HOMEが未設定の場合:
$HOME/.local/state/portdeck/debug-<timestamp>-<pid>.log
```

`XDG_STATE_HOME`を設定する場合は絶対パスである必要があります。設定されておらず`HOME`も利用できない場合、DEBUGログを安全に配置できないため起動に失敗します。

ログディレクトリはmode `0700`、ログファイルはmode `0600`です。実際のログパスはalternate screenへ入る前に端末へ1行表示され、TUIの起動Statusと`E`で開く詳細からも確認できます。

過去のログは新しいものを最大10件保持するよう、1回の起動につき最大64件まで整理します。整理対象は`debug-<数値>-<数値>.log`へ厳密に一致するportdeck所有名だけです。同じディレクトリにある他のファイルは変更しません。

DEBUGログには、次の処理経路をoperation IDとともに記録します。

- 接続、状態確認、切断
- Local／SOCKS転送の追加と取消
- ローカルport候補の試行
- セッションと転送の状態遷移
- 設定の読み込み、保存、rollback
- runtime entryの回収
- 所有セッションのshutdown

DEBUGモードでも、次の情報は記録しません。

- パスワード、秘密鍵、鍵のパスフレーズ
- 環境変数全体
- TUIの生のキー入力
- OpenSSHのraw stdout／stderr

DEBUG eventをTUI描画中のstdout／stderrへ出力することもありません。また、DEBUGモードによってOpenSSHの設定や接続引数は変わりません。

ログには運用上のHostエイリアス、address、portなどが含まれる場合があります。第三者へ共有する前に内容を確認してください。

## よくある問題

### 接続先が表示されない

`~/.ssh/config`にワイルドカードではない`Host <alias>`があるか、`Include`先を読み取れるか確認してください。`Host *`、`Host dev-*`、否定パターンは設定としてOpenSSHへ適用されますが、接続候補そのものとしては表示されません。

### OpenSSHを起動できない

`portdeck --diagnose`と`ssh -V`を実行し、PATHから`ssh`を起動できるか確認してください。`ssh`が見つからない場合、portdeckは独自SSH実装へ切り替わりません。

### 接続または認証に失敗する

`E`でOpenSSH stderrを確認し、同じ端末から`ssh <alias>`で接続できるか試してください。認証、ホスト鍵確認、鍵のパスフレーズ、ProxyJumpはOpenSSHが処理します。portdeckは認証方式やホスト鍵設定を緩和して再試行しません。

### 転送を追加できない

次を確認してください。

- 希望したローカルportを含む最大20個の候補が使用中ではないか
- SSH serverで`AllowTcpForwarding`が許可されているか
- Local転送の場合、リモート側から宛先hostとportへ到達できるか
- SOCKS転送の場合、選択したbind addressとportが適切か

portdeckは事前のbind確認だけでは転送成功とみなしません。`E`でOpenSSHが転送追加を拒否した理由を確認してください。

### 転送を取消できない

取消には、追加時に保存した種別、実際のローカルport、正規化済み転送指定を使います。OpenSSHが取消に失敗した場合はUI上だけInactiveにしません。`r`で親ControlMasterの状態を確認し、`E`で詳細を確認してください。

### 転送がUnavailableになった

親ControlMasterの切断を検出すると、その配下の転送は`Unavailable`になります。`r`で状態を再確認してください。portdeckは無条件の自動再接続や自動転送を行わないため、必要に応じてユーザー操作で再接続します。

### 設定ファイルを読み込めない

エラーに表示された設定パスのTOML、schema version、各ルールの値を確認してください。解析や検証に失敗した場合、portdeckは設定ファイルを上書きしません。内容を修正してから再起動してください。

### runtime entryに関する起動警告が出る

portdeckは既知の接続先へ対応するControlMasterだけを`ssh -O check`して回収します。現在の接続先へ対応づけられないentryは推測で削除せず、`E`の起動診断にパスを表示します。詳しくは[起動時の回収](runtime-and-configuration.md#起動時の回収)を参照してください。
