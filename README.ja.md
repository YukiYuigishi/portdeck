[English](README.md) | 日本語

# portdeck

portdeckは、システムのOpenSSH接続とLocal／SOCKS転送を一画面で管理するLinux向けTUIです。`~/.ssh/config`から接続先を選び、専用ControlMasterの開始・確認・終了と、転送ルールの保存・有効化・取消を操作できます。

SSHプロトコル、認証、暗号化、ホスト鍵確認、ProxyJump、TCP中継はOpenSSHへ委譲します。portdeckは汎用ターミナル、SSH鍵管理ツール、独自プロキシではありません。

## 主な機能

- `~/.ssh/config`と再帰的な`Include`から具体的なHostエイリアスを表示
- portdeck専用のOpenSSH ControlMasterを接続先ごとに管理
- OpenSSH `-L`によるLocal転送と`-D`によるSOCKS転送
- 転送ルールの保存、編集、ポート競合時の候補探索
- Hostエイリアス検索とVim風のキーボード操作
- OpenSSHエラー表示と、所有者限定ファイルへ保存するDEBUGログ

## 必要な環境

- Linux
- PATHから実行できるOpenSSH Clientの`ssh`
- Rust stable（ソースからインストールする場合）
- 具体的な`Host`エイリアスを含む`~/.ssh/config`（接続先を表示する場合）

MVPはOpenSSH 9.6p1で実通信を含めて検証しています。これより古いバージョンの最低保証値はまだ確定していません。macOSとWindowsは現在のサポート対象外です。

## インストール

```console
cargo install --path .
```

OpenSSHを利用できるか確認してから起動します。

```console
portdeck --diagnose
portdeck
```

`--help`と`--version`も利用できます。障害調査時は`portdeck --debug`で所有者限定のログを保存できます。

## クイックスタート

1. `portdeck`を起動し、接続先を選びます。
2. `c`で接続します。認証などの対話中はTUIが一時停止し、完了後に画面へ戻ります。
3. `a`でLocalまたはSOCKSの転送ルールを保存します。
4. Forwardsペインへ移動し、`Space`で転送を有効化します。
5. `Space`で転送を取り消し、`q`でportdeck所有セッションを終了して閉じます。

保存した転送ルールは、明示的に`Space`を押すまで有効になりません。再起動時も自動接続・自動有効化は行いません。

## 主なキー

| キー | 操作 |
| --- | --- |
| `c` | 選択した接続先へ接続 |
| `a` | 転送ルールを追加 |
| `Space` | 転送を有効化または取消 |
| `e` | Inactiveな転送ルールを編集 |
| `/` | Hostエイリアスを検索 |
| `Tab` / `h` / `l` | ペインを移動 |
| `↑` / `↓` / `j` / `k` | 選択を移動 |
| `E` | エラー詳細を表示 |
| `q` / `Ctrl-C` | 所有セッションを終了してquit |

削除、切断、状態確認を含む全操作は[使い方](docs/ja/usage.md)を参照してください。

## セキュリティ

- OpenSSHへはシェルを介さず、引数を個別に渡します。
- SSH設定、`known_hosts`、秘密鍵を変更しません。
- パスワード、秘密鍵、鍵のパスフレーズを保存・ログ出力しません。
- 外部公開bindには追加確認を表示します。SOCKS listenerにはportdeck独自の認証がありません。
- 通常終了時はportdeckが所有する全ControlMasterを停止します。detach機能はありません。

詳細は[実行環境と設定](docs/ja/runtime-and-configuration.md)を参照してください。

## ドキュメント

- [使い方](docs/ja/usage.md) — 接続、検索、Local／SOCKS転送、編集、終了
- [実行環境と設定](docs/ja/runtime-and-configuration.md) — 設定ファイル、ControlPath、回収処理、セキュリティ
- [トラブルシューティング](docs/ja/troubleshooting.md) — `--diagnose`、`--debug`、代表的な問題

## 開発

lint、commit hook、テスト、実sshdを使う統合テストについては[CONTRIBUTING.md](CONTRIBUTING.md)を参照してください。実装順序と未実施の環境別hardening項目は[PLAN.md](PLAN.md)にあります。
