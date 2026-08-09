# portdeck

portdeckは、システムのOpenSSH接続とローカルポートフォワードを管理するLinux向けTUIです。`~/.ssh/config`から接続先を選び、専用ControlMasterの開始・状態確認・終了と、`ssh -L`相当の転送の保存・追加・取消を一画面で扱えます。

SSHプロトコル、認証、暗号化、ホスト鍵確認、ProxyJump、TCP中継はOpenSSHへ委譲します。portdeckは汎用ターミナル、SSH鍵管理ツール、独自プロキシではありません。

## Requirements

- Linux
- Rust stable（ソースからインストールする場合）
- PATHから実行できるOpenSSH Clientの`ssh`
- 具体的な`Host`エイリアスを含む`~/.ssh/config`（接続先を表示する場合）

MVPはOpenSSH 9.6p1で実通信を含めて検証しています。これより古いバージョンの最低保証値はまだ確定していません。macOSとWindowsは現在のサポート対象外です。

## Install and run

```console
cargo install --path .
portdeck --diagnose
portdeck --debug --diagnose
portdeck
```

`--diagnose`は`ssh -V`を実行し、使用されるOpenSSHのバージョンを表示します。`--help`と`--version`も利用できます。

### DEBUG mode

障害調査時は`portdeck --debug`でTUIを起動すると、DEBUG levelの構造化ログを実行ごとのファイルへ保存します。`portdeck --debug --diagnose`でも同じ形式のログを利用できます。保存先は次のとおりです。

```text
$XDG_STATE_HOME/portdeck/debug-<timestamp>-<pid>.log
# XDG_STATE_HOMEが未設定の場合:
$HOME/.local/state/portdeck/debug-<timestamp>-<pid>.log
```

ログdirectoryはmode `0700`、ログファイルはmode `0600`です。実際のパスはalternate screenへ入る前に端末へ1行表示し、TUIの起動Statusとエラー詳細からも確認できます。過去ログは新しいものを最大10件保持するよう、1起動につき最大64件まで整理します。`debug-<数値>-<数値>.log`へ厳密に一致するportdeck所有名だけが対象で、それ以外のファイルは変更しません。

DEBUG eventは接続、状態確認、切断、転送の追加・取消、port候補、状態遷移、設定保存・rollback、runtime回収、shutdownをoperation IDとともに記録します。TUI描画中のstdout／stderrへDEBUG eventは出しません。パスワード、鍵のパスフレーズ、環境変数全体、生のキー入力、OpenSSHのraw stdout／stderrは記録しません。DEBUG modeによってOpenSSHの設定や接続引数が変わることもありません。

接続候補は`~/.ssh/config`と再帰的な`Include`から収集します。`Host *`、ワイルドカード、否定パターンは候補には表示しませんが、その設定は`ssh -G <alias>`と実際の接続時にOpenSSHが通常どおり適用します。portdeckはSSH設定ファイルを変更しません。

## Keyboard controls

通常画面では次のキーを使います。

| Key | Action |
| --- | --- |
| `Tab` / `←` / `→` | TargetsとForwardsのペインを切り替える |
| `↑` / `↓` / `j` / `k` | 選択を移動する |
| `c` | 選択した接続先へ接続する |
| `r` | `ssh -O check`で接続状態を再確認する |
| `a` | 保存済み転送ルールを追加する |
| `Space` | 選択した転送を有効化、または取消する |
| `D` | 転送ルールを確認後に削除する |
| `d` | SSHセッションを確認後に終了する |
| `e` | 直近のOpenSSH stderrまたは起動診断を表示する |
| `q` / `Ctrl-C` | portdeck所有セッションを終了してquitする |

接続時にはTUIを一時停止し、認証、鍵のパスフレーズ、初回ホスト鍵確認に端末を直接使える状態でOpenSSHを起動します。OpenSSHが終了した後にTUIへ戻ります。

### Add a forward

`a`を押し、次の項目を入力します。`Tab`または`↑`/`↓`で項目を移動し、`Enter`で保存します。

- Label: 任意の表示名
- Local bind address: 既定値`127.0.0.1`
- Preferred local port: 空欄ならリモート宛先ポートと同じ番号
- Remote destination host: リモート側から見た宛先。既定値`127.0.0.1`
- Remote destination port: 必須、1から65535

保存しただけでは転送は有効になりません。Forwardsペインでルールを選択し、`Space`を押してください。希望ポートが競合する場合は最大20個の連続した候補を試し、OpenSSHが実際に追加できたローカルポートを表示します。

`0.0.0.0`、`::`、`*`へのbindはローカルネットワークなどへ公開される可能性があるため、保存前に追加確認を表示します。

## Runtime and persistence

転送ルールの定義は次のTOMLへ、変更操作ごとに一時ファイル・fsync・renameを使って保存します。

```text
$XDG_CONFIG_HOME/portdeck/config.toml
# XDG_CONFIG_HOMEが未設定の場合:
$HOME/.config/portdeck/config.toml
```

形式の例です。

```toml
version = 1

[[targets]]
host_alias = "dev-server"

[[targets.forwards]]
id = "rule-00000001"
label = "web"
bind_address = "127.0.0.1"
requested_local_port = 8080
remote_host = "127.0.0.1"
remote_port = 3000
```

実行中のセッション状態、実際に選ばれたポート、PID、認証情報は保存しません。再起動時にルールは表示されますが、自動接続・自動有効化は行いません。

ControlPathは`$XDG_RUNTIME_DIR/portdeck/`に置きます。`XDG_RUNTIME_DIR`がない場合は`/tmp/portdeck-<uid>/`を使います。ディレクトリは現在のユーザー所有か確認し、mode `0700`に制限します。接続先名はsocket名へ埋め込まず、固定長ハッシュを使います。

起動時に前回のportdeckが残した既知のControlMasterを`ssh -O check`で確認し、生存していれば明示的に終了します。生存確認に失敗した既知のstale socketだけを削除し、現在の接続先へ対応づけられないruntime entryは保持して診断に表示します。

## Security model

- コマンドはシェル文字列ではなく、個別のargvとしてOpenSSHへ渡します。
- `StrictHostKeyChecking=no`や`UserKnownHostsFile=/dev/null`を追加しません。
- パスワード、秘密鍵、鍵パスフレーズを取得・保存・ログ出力しません。
- DEBUG modeでも環境変数全体、生のキー入力、OpenSSHのraw stdout／stderrをログ出力しません。
- ユーザーの`~/.ssh/config`、`known_hosts`、秘密鍵を変更しません。
- 専用masterでは`ClearAllForwardings=yes`を使い、SSH設定由来の未追跡forwardを混在させません。
- 追加・取消の最終成否はOpenSSHの終了ステータスで判定します。事前のport bind確認だけで`Active`にはしません。
- 通常終了時はportdeckが所有する全ControlMasterを確認して終了します。TUIを閉じた後も接続を残すdetach機能はありません。

## Troubleshooting

- 接続先がない: `~/.ssh/config`にワイルドカードではない`Host <alias>`があるか、`Include`先を読めるか確認してください。
- OpenSSHを起動できない: `portdeck --diagnose`と`ssh -V`を確認してください。
- 接続・認証に失敗する: `e`でOpenSSH stderrを表示し、同じaliasに`ssh <alias>`で接続できるか確認してください。portdeckは認証方式やホスト鍵設定を緩和して再試行しません。
- 転送を追加できない: ローカルポート競合、サーバーの`AllowTcpForwarding`、リモート宛先を確認してください。
- 状態が古い: `r`でControlMasterを再確認してください。切断を検出すると配下の転送も`Unavailable`になります。
- 詳細な操作経路が必要: `portdeck --debug`で再現し、起動時またはTUIに表示された所有者限定ログを確認してください。共有前に運用上のhost名やport番号も確認してください。
- 設定ファイルが壊れている: portdeckはファイルを上書きせず、パース診断を表示して終了します。内容を修正してから再起動してください。

## Development

```console
./scripts/install-git-hooks.sh
./scripts/lint.sh
cargo test --all-targets --all-features
```

インストールしたpre-commit hookとGitHub Actionsは同じlintスクリプトを使います。hookを通さず意図的にコミットする必要がある場合も、変更を共有する前に上記チェックを手動で実行してください。

偽`ssh`を使うadapter testsは通常のテストに含まれます。実sshd、隔離したhost/client key、専用`known_hosts`、実TCP通信を使う統合テストは明示的に実行します。

```console
cargo test --test openssh_integration -- --ignored --nocapture
```

この統合テストは`/usr/bin/ssh`、`/usr/bin/ssh-keygen`、`/usr/sbin/sshd`を必要とします。ユーザーのSSH設定、`known_hosts`、鍵、既存ControlMasterには触れません。

実装順序と未実施の環境別hardening項目は[PLAN.md](PLAN.md)を参照してください。
