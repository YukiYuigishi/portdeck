[English](../en/runtime-and-configuration.md) | 日本語

# 実行環境と設定

portdeckが保存する転送ルール、実行時のControlPath、起動時回収、セキュリティ上の境界を説明します。

## 保存設定

転送ルールの定義は、XDG Base Directoryに従ったTOMLファイルへ保存します。

```text
$XDG_CONFIG_HOME/portdeck/config.toml
# XDG_CONFIG_HOMEが未設定の場合:
$HOME/.config/portdeck/config.toml
```

`XDG_CONFIG_HOME`を設定する場合は絶対パスである必要があります。設定ファイルがまだ存在しない場合は、転送ルールがない状態で起動します。

変更操作のたびに、一時ファイルへの書き込み、`fsync`、`rename`、親ディレクトリの`fsync`を行います。一時ファイルはmode `0600`で作成し、途中で失敗した場合は既存の設定を成功したように置き換えません。設定を読み取れない、またはTOMLを解析できない場合も、壊れたファイルを上書きせず診断を表示して終了します。

現在のschema versionは`1`です。LocalとSOCKSの例を次に示します。

```toml
version = 1

[[targets]]
host_alias = "dev-server"

[[targets.forwards]]
id = "rule-00000001"
label = "web"
bind_address = "127.0.0.1"
requested_local_port = 8080
kind = "local"
remote_host = "127.0.0.1"
remote_port = 3000

[[targets.forwards]]
id = "rule-00000002"
label = "browser-proxy"
bind_address = "127.0.0.1"
requested_local_port = 1080
kind = "socks"
```

`kind`がない既存のversion 1ルールはLocalとして読み込み、次回保存時に`kind = "local"`を明記します。SOCKSルールにリモート宛先は保存しません。現在のSSH設定から検出できない接続先の保存セクションは、既知のルールを更新しても保持します。

保存するのは接続先ごとの次の定義情報です。

- ルールIDと任意のラベル
- LocalまたはSOCKSの種別
- ローカル側のbind addressと希望port
- Local転送の場合だけ、リモート側から見た宛先hostとport

実行中のセッション状態、OpenSSHのPID、実際に選ばれたローカルポート、認証情報は保存しません。再起動後にルールは表示されますが、自動接続や自動有効化は行いません。

## 実行状態とControlPath

ControlPathは次のportdeck専用ディレクトリへ置きます。

```text
$XDG_RUNTIME_DIR/portdeck/
# XDG_RUNTIME_DIRが未設定の場合:
/tmp/portdeck-<uid>/
```

`XDG_RUNTIME_DIR`を設定する場合は絶対パスである必要があります。runtimeディレクトリは実ディレクトリかつ現在のユーザー所有であることを確認し、mode `0700`に制限します。他ユーザー所有のパスやsymlinkなど安全に利用できないパスは拒否します。

Unix domain socketのパス長制限を避けるため、ControlPathは短い固定名と接続先IDの固定長ハッシュから作ります。Hostエイリアスをsocket名へ直接埋め込みません。portdeckは他ツールやユーザーが作成したControlMasterを採用しません。

有効な転送の状態と実際のローカルポートは、現在のportdeckプロセスがメモリ上で管理します。接続状態はPIDの有無ではなく、専用ControlPathに対する`ssh -O check`の結果で判断します。

## 起動時の回収

異常終了などで前回のportdeckが残したruntime entryは、次回起動時に確認します。

1. 現在のSSH設定から検出した接続先と、portdeck用ControlPathを対応づけます。
2. `ssh -O check`で既知のControlMasterが生存しているか確認します。
3. 生存している既知のmasterは`ssh -O exit`で明示的に終了します。
4. 生存確認に失敗した既知のstale socketだけを削除します。

現在の接続先へ対応づけられないruntime entryは推測で削除せず保持し、起動診断に表示します。単にsocketファイルを消して生存中のmasterを孤立させることはしません。

## 終了処理

`d`は選択したSSHセッションを、`q`または`Ctrl-C`はportdeckが所有する全セッションを`ssh -O exit`で終了します。セッション終了に伴い、その配下の転送も利用できなくなります。

通常終了時に所有ControlMasterの停止へ失敗した場合は、その失敗を握りつぶさず終了エラーとして報告します。TUIだけを閉じて接続を維持するdetach機能はありません。

## セキュリティ上の性質

- SSH認証、暗号化、ホスト鍵確認、ProxyJump、SOCKS4/5処理、TCP中継はシステムのOpenSSHへ委譲します。
- OpenSSHのコマンドはシェル文字列ではなく個別のargvとして実行します。
- `StrictHostKeyChecking=no`や`UserKnownHostsFile=/dev/null`を自動指定しません。
- ユーザーの`~/.ssh/config`、`known_hosts`、秘密鍵を変更しません。
- パスワード、秘密鍵、鍵のパスフレーズを取得、保存、ログ出力しません。
- 専用masterの開始時は`ClearAllForwardings=yes`を使い、SSH設定由来の未追跡forwardを混在させません。
- 転送の追加と取消はOpenSSHの終了ステータスで判定します。事前のローカルport確認だけで`Active`にはしません。
- 既定のbind addressは`127.0.0.1`です。`0.0.0.0`、`::`、`*`は追加確認なしに保存しません。
- SOCKS listenerにはportdeck独自の認証がなく、OpenSSH `-D`以外の独自proxy処理もありません。

DEBUGログの保存先、権限、記録対象については[トラブルシューティング](troubleshooting.md#debugモード)を参照してください。
