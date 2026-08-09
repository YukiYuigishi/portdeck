# PLAN.md

## Current Status

portdeck 0.1.0のMVP実装を完了。OpenSSH 9.6p1を使った隔離sshd統合テストで、ControlMaster、Local転送、direct／ProxyJumpのSOCKS5転送、実TCP通信、個別取消、終了を確認済み。

Phase 9のうち、複数OpenSSHバージョンと実接続先を使う認証・ProxyJump・ホスト鍵の互換性マトリクスは継続課題として残す。これは現在のMVP実装範囲を広げるものではない。

MVPは、Linux上でシステムOpenSSHのControlMasterを管理し、TUIから `-L` ローカルポートフォワードを追加・削除できる状態を指す。

## Development Policy

- 各フェーズは、完了条件を満たしてから次へ進む。
- 最初にOpenSSH制御部分をCLIまたはテストから検証し、TUIはその後に載せる。
- ユーザーの実SSH環境に依存するテストと、偽 `ssh` を使う再現可能なテストを分ける。
- MVP完了までMosh、リモートポート自動検出、`-R`、TLS処理へ範囲を広げない。MVP完了後の利用要求を受け、OpenSSH `-D`は追加済み。

## Development Commands

- Lint: `./scripts/lint.sh`
- Test: `cargo test --all-targets --all-features`
- Install Git hooks: `./scripts/install-git-hooks.sh`

`pre-commit` hookとGitHub Actionsでも同じlintスクリプトを実行する。

## Phase 0: Requirements and Design Baseline

- [x] プロダクトの目的を定義する。
- [x] OpenSSHとTUIの責務を分離する。
- [x] ControlMasterを使用する方針を定義する。
- [x] MVPと将来機能を分離する。
- [x] セキュリティ要件と非目標を定義する。
- [x] プロジェクト名とバイナリ名を決定する（`portdeck`）。
- [ ] 対象とする最低OpenSSHバージョンを実機で確認する。

### Exit Criteria

- `AGENTS.md` と `PLAN.md` がレビュー可能な状態で存在する。
- MVPの範囲が「SSH接続と `-L` 管理」に限定されている。

## Phase 1: Project Bootstrap

- [x] Rustプロジェクトを初期化する。
- [x] formatter、lint、testの基本コマンドを決める。
- [x] `domain`、`ssh`、`config`、`runtime`、`tui` のモジュール境界を作る。
- [x] 構造化ログとエラー型の最小構成を作る。
- [x] CIでformat、lint、unit testを実行する。

### Exit Criteria

- 空のTUIまたは最小CLIが起動する。
- `cargo fmt --check`、`cargo clippy`、`cargo test` が成功する。
- UIコードから直接 `ssh` を起動しない構造になっている。

## Phase 2: OpenSSH Capability Probe

実装を広げる前に、実際のOpenSSHで必要な操作が成立することを小さな検証コードまたは統合テストで確認する。

- [x] `ssh` 実行ファイルを検出する。
- [x] `ssh -V` の取得と診断表示を実装する。
- [x] 専用ControlPathでmasterを開始する。
- [x] `-O check` でmasterを確認する。
- [x] `-O forward -L ...` で転送を追加する。
- [x] `-O cancel -L ...` で転送を削除する。
- [x] `-O exit` でmasterを終了する。
- [x] 認証が必要な場合のTUI suspend/resume方針を実装し、PTY上でTUIの停止・復元境界を検証する。
- [x] OpenSSH stderrと終了コードを記録し、失敗パターンを整理する。

### Exit Criteria

- 1本のControlMasterに対して複数のローカル転送を追加・削除できる。
- 転送先のテストTCPサーバーへ実際に通信できる。
- 接続・転送の失敗を終了コードから検出できる。
- 検証終了後にmasterと転送が残らない。

## Phase 3: Domain and Command Adapter

- [x] `Target`、`Session`、`ForwardRule`、`ActiveForward` を定義する。
- [x] セッション状態遷移を実装する。
- [x] 転送状態遷移を実装する。
- [x] OpenSSH実行結果を表す型を定義する。
- [x] argvベースのコマンドビルダーを実装する。
- [x] `connect`、`check`、`add_local_forward`、`cancel_local_forward`、`disconnect` を実装する。
- [x] 外部コマンド実行部分をテスト用に差し替え可能にする。
- [x] 偽 `ssh` によるadapter testを追加する。

### Exit Criteria

- シェルを介さず、すべてのSSH操作を実行できる。
- 成功、失敗、stderrが型付きの結果として上位層へ返る。
- 状態遷移とコマンド引数にunit testがある。

## Phase 4: SSH Target Discovery

- [x] `~/.ssh/config` の具体的なHostエイリアスを列挙する。
- [x] `Include` を再帰的に処理する。
- [x] include循環と重複を安全に処理する。
- [x] ワイルドカードと否定パターンを候補一覧から除外する。
- [x] `ssh -G <alias>` から表示用の有効設定を取得する。
- [x] 設定ファイルが存在しない場合を正常系として扱う。
- [x] 不正なHostエイリアスをコマンドへ渡さない入力検証を追加する。

### Exit Criteria

- 一般的な `~/.ssh/config` から接続候補を一覧化できる。
- ProxyJump等の意味解釈を独自実装せずOpenSSHへ委譲している。
- ユーザーのSSH設定を変更しない。

## Phase 5: Runtime and Control Socket Lifecycle

- [x] XDG runtime directoryを解決する。
- [x] 所有者限定のランタイムディレクトリを作成する。
- [x] 接続先ごとの短く安定したControlPathを生成する。
- [x] セッション開始と `-O check` による状態更新を実装する。
- [x] セッション終了処理を実装する。
- [x] 正常終了時に全専用masterを終了する。
- [x] 前回異常終了で残ったcontrol socketを検出する。
- [x] 生存masterと単なるstale socketを区別して回収する。
- [x] 他ツールのControlPathを触らないことをテストする。

### Exit Criteria

- 複数接続先のControlMasterを衝突なく管理できる。
- 接続状態がPIDではなく `-O check` から再構築される。
- 正常終了後に本ツール所有のmasterが残らない。

## Phase 6: Local Forward Management

- [x] 転送入力値を検証する。
- [x] 既定値 `127.0.0.1:<remote-port>` を実装する。
- [x] IPv4、ホスト名、IPv6を正しく正規化する。
- [x] 希望ローカルポートへの転送追加を実装する。
- [x] ポート競合時の限定的な候補探索を実装する。
- [x] 実際に確保したローカルポートを状態へ保存する。
- [x] 正確な転送指定による取消を実装する。
- [x] セッション切断時に配下の転送状態を更新する。
- [x] `0.0.0.0`、`::`、`*` bind時の警告情報を実装する。

### Exit Criteria

- 同一セッションへ複数転送を追加・削除できる。
- ローカルポート競合時に別の利用可能なポートへフォールバックできる。
- OpenSSHが拒否した転送を `Active` と表示しない。
- 取消失敗をユーザーへ通知できる。

## Phase 7: MVP TUI

- [x] アプリケーションイベントループを実装する。
- [x] 接続先・セッション一覧ペインを実装する。
- [x] 転送一覧ペインを実装する。
- [x] 接続・切断操作を実装する。
- [x] 転送追加フォームを実装する。
- [x] 転送削除と確認表示を実装する。
- [x] ステータス行とキーヘルプを実装する。
- [x] OpenSSH stderrの詳細表示を実装する。
- [x] TUIからOpenSSH認証画面へのsuspend/resumeを実装する。
- [x] 小さい端末サイズの表示を実装する。
- [x] 色以外の状態表現を追加する。

### Exit Criteria

- ユーザーがマウスなしで主要操作を完了できる。
- 接続先、接続状態、ローカル待受、リモート宛先を一画面で把握できる。
- 認証やホスト鍵確認をOpenSSHへ安全に引き渡せる。
- 失敗後もTUIが壊れず、再操作できる。

## Phase 8: Persistence and Recovery

- [x] 本ツール固有設定の保存形式を確定する（TOML schema version 1）。
- [x] ラベルと接続先に属する転送ルール定義を保存する。
- [x] atomic writeを実装する。
- [x] 壊れた設定ファイルの診断と安全な失敗を実装する。
- [x] 実行状態と保存済み定義を分離する。
- [x] 異常終了後の起動時リカバリー画面または処理を実装する。
- [x] 認証情報が保存対象に入らないことを確認する。

### Exit Criteria

- 再起動後も保存済み転送ルールを確認できる。
- 実際には切断済みのセッションを接続中と誤表示しない。
- 設定書込み中の異常終了で既存設定を失いにくい。

## Phase 9: Hardening and Release Readiness

- [ ] Linux上の複数OpenSSHバージョンで動作確認する。
- [ ] ProxyJumpを使う接続先で確認する。
- [ ] 公開鍵、ssh-agent、パスフレーズ、keyboard-interactive認証で確認する。
- [ ] ホスト鍵初回確認とホスト鍵不一致を確認する。
- [ ] サーバー側でTCP forwardingが禁止された場合を確認する。
- [x] 長いHostエイリアスでもControlPath長制限を超えないことを確認する。
- [x] SIGINT、SIGTERM、端末切断相当のSIGHUPで端末復元と正常終了を確認する。
- [x] READMEへ導入方法、権限、安全上の注意を書く。
- [x] MVP acceptance criteriaをunit、adapter、TUI buffer、隔離sshd統合テストで確認する。

### Exit Criteria

- 主要認証方式で秘密情報を本ツールが取得しない。
- 終了と異常系で不要なmasterを極力残さない。
- 初めて使うユーザーがREADMEだけで接続と転送を作成できる。

## Post-MVP Backlog

MVP完了後、利用上の必要性を確認して着手する。

### Remote Port Discovery

- [ ] ControlMaster経由で `ss -H -ltn` を実行する。
- [ ] listen addressとportを解析する。
- [ ] 権限がある場合だけプロセス情報を表示する。
- [ ] 検出ポートから転送を追加する導線を作る。
- [ ] 定期更新と手動更新の負荷を評価する。

### Protocol-aware Convenience

- [ ] `tcp`、`http`、`https` の表示用ヒントを追加する。
- [ ] HTTP/HTTPS URLをコピーできるようにする。
- [ ] ブラウザ起動を追加する場合は明示操作に限定する。
- [ ] TLSを終端・変換しないことをUIと文書で明記する。

### Session Profiles

- [ ] 接続時に有効化する転送セットを定義できるようにする。
- [ ] 自動接続・自動転送を明示的な設定として追加する。
- [ ] TUI終了後も維持するdetach機能の要否を検討する。

### Mosh

- [ ] Moshで解決したい利用場面を整理する。
- [ ] SSH接続先定義を再利用する。
- [ ] Moshセッションの起動・終了・表示を追加する。
- [ ] Moshがポート転送を提供しないことを能力モデルへ反映する。

### Additional SSH Forward Types

- [ ] 利用要求が確認できた場合に限り `-R` を検討する。
- [x] 利用要求に基づき、OpenSSH `-D`によるSOCKS転送を追加する。
- [x] Local／SOCKSそれぞれの外部公開警告を実装する。

## Deferred Decisions

- 最低OpenSSHバージョン
- macOS正式対応の時期
- TUI終了後もセッションを残すdetach機能
- リモートポート監視の更新間隔
- Moshを同一バイナリへ含めるか別機能にするか
