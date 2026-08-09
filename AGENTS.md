# AGENTS.md

## Project Overview

本プロジェクトは、OpenSSHの接続とローカルポートフォワードをTUIから管理するRust製ツールである。

中心となる課題は、リモート開発時に毎回 `ssh -L` を組み立て、複数のSSHプロセスと転送ポートを人手で追跡する手間を減らすことにある。

通信、認証、暗号化、SSH設定の解決、TCP中継はシステムのOpenSSHへ委譲する。本ツール自身はSSHプロトコルやTCPプロキシを実装しない。

プロジェクト名は未定である。文書中では単に「本ツール」と呼ぶ。

## Product Goal

ユーザーが `~/.ssh/config` の接続先を選び、次の操作をTUI内で完結できる状態を目標とする。

1. SSH接続を開始・終了する。
2. 接続状態を確認する。
3. `-L` 相当のローカルポートフォワードを追加・削除する。
4. どの接続で、どのローカルポートが、どのリモート宛先へ転送されているか確認する。
5. よく使う転送ルールを接続先ごとに保存して再利用する。
6. ローカルポートの競合やSSHの失敗理由を把握する。

MVPの価値は「面倒な `ssh -L` の組み立てと生存管理を、見える状態で扱えること」に置く。

## Product Positioning

MVPは汎用ターミナル、SSHクライアント、認証情報管理ツールではない。「SSH接続・ポートフォワード管理TUI」である。

内部設計では将来のMoshなどを妨げないが、先回りしてプラグイン機構や汎用セッション基盤を作らない。まずOpenSSHによるローカルポートフォワードを完成させる。

## Terminology

- **接続先（target）**: `~/.ssh/config` に記載された具体的な `Host` エイリアス。
- **SSHセッション（session）**: 本ツールが所有するOpenSSH ControlMaster接続。
- **転送ルール（forward）**: 接続先に保存された `-L` 相当の定義。
- **有効な転送（active forward）**: 転送ルールを現在のSSHセッションへ追加した実行状態。
- **ローカル側**: TUIとOpenSSH Clientを動かしている端末。
- **リモート側**: OpenSSH Server (`sshd`) が動いている接続先。
- **リモート宛先**: リモート側から接続する `host:port`。通常は `127.0.0.1:<port>`。
- **定義状態**: 設定ファイルに保存された接続先や転送ルール。
- **実行状態**: 現在のControlMasterと転送の状態。

`localhost` だけではどちら側か曖昧になるため、UI、ログ、文書では可能な限り「ローカル側」「リモート側」と明記する。

## Primary User Flow

1. ユーザーがTUIを起動する。
2. TUIがSSH設定から接続先を一覧表示する。
3. ユーザーが接続先を選んで接続する。
4. 本ツールが専用ControlMasterを起動する。
5. ユーザーがリモート宛先ポートを指定する。
6. 本ツールがローカル待受ポートを決定し、ControlMasterへ転送追加を依頼する。
7. TUIが実際のローカル待受とリモート宛先を表示する。
8. ユーザーが転送またはSSHセッションを終了する。

## MVP Functional Requirements

### 1. SSH接続先の表示

- `~/.ssh/config` と、その `Include` 先から具体的な `Host` エイリアスを列挙する。
- `Host *`、ワイルドカード、否定パターンは接続候補として直接表示しない。ただしOpenSSHによる設定解決には通常どおり適用される。
- HostName、User、Port、ProxyJumpなどの有効値を独自に再実装しない。必要な表示情報は `ssh -G <alias>` でOpenSSHに解決させる。
- 本ツールは `~/.ssh/config` を変更しない。

### 2. SSHセッションの開始

- 接続先ごとに、本ツール専用のControlMasterを1つ起動する。
- 認証、ホスト鍵確認、鍵のパスフレーズ、ProxyJumpはOpenSSHに任せる。
- 対話が必要な間はTUI表示を一時停止し、OpenSSHが端末を直接利用できるようにする。
- 接続成功後はTUIへ戻り、状態を `Connected` として表示する。
- ユーザーのSSH設定にある暗号方式やホスト鍵検証を緩和しない。

概念上の起動コマンドは次のとおりである。実装ではシェル文字列を組み立てず、引数を個別に渡す。

```text
ssh -M -N -f \
  -S <control-path> \
  -o ClearAllForwardings=yes \
  <host-alias>
```

`ClearAllForwardings=yes` により、本ツールが認識していない `LocalForward` 等を専用ControlMasterへ暗黙に混在させない。既存のSSH設定による転送を取り込む機能はMVP対象外とする。

### 3. SSHセッションの状態確認

- ControlMasterの状態は `ssh -S <control-path> -O check <host-alias>` で確認する。
- PIDの存在だけを接続成功の根拠にしない。
- 少なくとも `Disconnected`、`Connecting`、`Connected`、`Stopping`、`Failed` を区別する。
- 接続が失われた場合、転送も利用不能であることを同じ画面上に反映する。
- MVPでは無条件の自動再接続を行わない。ユーザー操作による再接続を基本とする。

### 4. ローカルポートフォワードの追加

- 入力項目は次のとおりとする。
  - ラベル（任意）
  - ローカル待受アドレス。既定値は `127.0.0.1`
  - 希望ローカルポート。省略時はリモートポートと同じ番号
  - リモート宛先ホスト。既定値は `127.0.0.1`
  - リモート宛先ポート。必須
- 転送はControlMasterへ動的に追加する。

```text
ssh -S <control-path> \
  -o ClearAllForwardings=no \
  -O forward \
  -L <bind-address>:<local-port>:<remote-host>:<remote-port> \
  <host-alias>
```

- OpenSSHの終了ステータスが成功になるまで、転送を `Active` と表示しない。
- ローカルポートが使用中の場合は、次の候補を限定回数試し、実際に確保できた番号を表示する。
- OpenSSHのローカル転送ではポート `0` の割当結果を取得する標準手段に依存しない。事前のbind確認だけで成功とみなさず、最終的な成否はOpenSSHの転送追加結果で判定する。
- 任意のアドレスへの公開は暗黙に行わない。`0.0.0.0`、`::`、`*` を指定する場合は警告を表示する。

### 5. ローカルポートフォワードの削除

- 追加時に使用した正規化済み転送指定を保存する。
- 削除時は同じ指定を使ってControlMasterへ取消を依頼する。

```text
ssh -S <control-path> \
  -o ClearAllForwardings=no \
  -O cancel \
  -L <bind-address>:<local-port>:<remote-host>:<remote-port> \
  <host-alias>
```

- OpenSSHが取消に失敗した場合、UI上だけ削除して成功したように見せない。

### 6. SSHセッションの終了

- セッション終了時は `ssh -S <control-path> -O exit <host-alias>` を使う。
- セッション終了に伴って、その配下の転送を非アクティブとして扱う。
- 通常終了時、本ツールが所有するControlMasterは明示的に終了する。
- TUIを閉じても接続を維持するdetach機能はMVP対象外とする。

### 7. 状態表示とエラー表示

- 接続先ごとに、接続状態と有効な転送数を表示する。
- 転送ごとに、ラベル、状態、ローカル待受、リモート宛先を表示する。
- OpenSSHの標準エラーをユーザーが確認できるようにする。
- エラーは少なくとも次を区別する。
  - OpenSSHが見つからない
  - 接続・認証失敗
  - ControlMasterが存在しない
  - ローカルポート競合
  - サーバー側ポートフォワード拒否
  - 転送取消失敗
  - SSH設定の解析失敗

### 8. 転送ルールの保存

- ラベル、bind address、希望ローカルポート、リモート宛先を接続先ごとに保存する。
- 保存済みルールと、現在OpenSSHへ追加されている有効な転送を区別する。
- TUIを再起動しても保存済みルールを確認・再利用できる。
- MVPでは保存済みルールを起動時に自動接続・自動有効化しない。
- 認証情報、秘密鍵、鍵パスフレーズ、OpenSSHの実行時PIDは保存しない。

## Post-MVP Requirements

優先度順の候補であり、MVP完了前に着手しない。

### Remote Port Discovery

- 既存ControlMaster上でリモートコマンドを実行し、listen中のTCPポートを取得する。
- Linuxでは最初に `ss -H -ltn` を対象とする。
- `ss -H -ltnp` のプロセス情報は権限により欠落する前提で扱う。sudoを要求しない。
- 検出したポートから転送追加フォームを開けるようにする。
- ポートが消えた場合は「リモートでlistenしていない」と表示するが、SSHセッション切断とは区別する。

### Protocol Hint

- 転送ルールに `tcp`、`http`、`https` の表示用ヒントを持たせる。
- これはURL表示やブラウザ起動にのみ使用し、HTTP解析、TLS終端、HTTPからHTTPSへの変換は行わない。
- SSHローカルフォワードは常に生のTCPを転送する。

### Automatic Activation

- 自動接続・自動転送は明示的に有効化されたルールだけを対象とする。

### Mosh

- 接続先定義をSSHと共有し、Moshセッションを起動・表示できるようにする。
- Moshはポートフォワード機能を提供しないため、SSH転送とは別の能力として扱う。
- Mosh対応のためにMVPのOpenSSH転送モデルを一般化しすぎない。

## Non-Goals

MVPでは以下を実装しない。

- SSHプロトコル、暗号、認証のRustによる再実装
- Rustによる独自SOCKSプロキシまたは独自TCPプロキシ（OpenSSH `-D`への委譲は対応済み）
- リモート側エージェントや専用デーモン
- `-R` リモートフォワード
- VPN、TUN/TAP
- SFTPファイラー
- ターミナルエミュレーター
- SSH鍵、パスワード、秘密情報の保管
- TLS証明書の発行やTLS終端
- Windowsネイティブ対応
- 汎用プラグインシステム

## Supported Environment

- MVPの第一対象はLinuxクライアントとする。
- システムにOpenSSH Clientがインストールされていることを前提とする。
- リモート側は標準的なOpenSSH Serverを前提とし、専用ソフトウェアを要求しない。
- macOSはUnix domain socketとOpenSSHの挙動を検証後に対応対象へ含める。
- WindowsはControlMaster、端末制御、パスの扱いを別途設計するまで対象外とする。

## Architecture

```text
TUI
  ↓ user intent / rendered state
Application State
  ├─ Target Catalog
  ├─ Session Manager
  └─ Forward Manager
       ↓
OpenSSH Adapter
  ↓ argv-based process execution
System OpenSSH Client
  ↓ ControlMaster / SSH channels
Remote sshd
```

### Component Responsibilities

#### `domain`

- 接続先、SSHセッション、転送ルール、状態遷移を表現する。
- TUIフレームワークやプロセス実行APIへ依存しない。

#### `ssh`

- OpenSSHコマンドの構築と実行を担当する。
- `connect`、`check`、`add_local_forward`、`cancel_local_forward`、`disconnect` を提供する。
- 標準出力、標準エラー、終了ステータスを構造化して返す。
- シェルを介さず、すべての引数を `Command::arg` 相当で渡す。

#### `config`

- SSH Hostエイリアスの列挙を担当する。
- OpenSSH設定の完全な意味解釈は行わない。
- 本ツール固有のラベルや保存済み転送ルールを読み書きする。

#### `runtime`

- ControlPath用ディレクトリと実行中セッションの対応を管理する。
- 起動時の残存ControlMaster検出と終了処理を担当する。

#### `tui`

- 入力、選択、モーダル、状態表示を担当する。
- OpenSSHコマンド文字列を直接生成しない。

## Domain Model

概念上、次の情報を保持する。フィールド名や直列化形式は実装時に調整してよいが、責務を混在させない。

```text
Target
  id
  host_alias
  source

Session
  id
  target_id
  control_path
  state
  last_error

ForwardRule
  id
  target_id
  label
  bind_address
  requested_local_port
  remote_host
  remote_port

ActiveForward
  rule_id
  session_id
  actual_local_port
  runtime_state
  last_error
```

`ForwardRule` は接続先に属する永続的な定義、`ActiveForward` は現在のSSHセッションに属する実行状態である。

`requested_local_port` と `actual_local_port` を分離する。ポート競合時に実際の番号が変わっても、ユーザーの希望値を失わないためである。

## State Model

### Session State

```text
Disconnected → Connecting → Connected → Stopping → Disconnected
                      ↓           ↓
                    Failed      Failed
```

### Forward State

```text
Inactive → Adding → Active → Removing → Inactive
              ↓         ↓          ↓
            Failed    Unavailable  Failed
```

- `Unavailable` は親SSHセッションが切断された状態を表す。
- UI操作の途中で非同期結果が返っても、古い操作結果で新しい状態を上書きしない。
- 状態変更はアプリケーション層に集約し、描画処理から直接変更しない。

## Control Socket Management

- ControlPathは本ツール専用のランタイムディレクトリに置く。
- Linuxでは `$XDG_RUNTIME_DIR/<app>/` を優先し、所有者だけがアクセスできる `0700` のディレクトリを使用する。
- ControlPathはUnix domain socketのパス長制限を考慮し、短い固定ディレクトリと接続先IDのハッシュで構成する。
- 他ツールやユーザーが作成したControlMasterを勝手に採用・終了しない。
- 接続先文字列をそのままファイルパスへ埋め込まない。
- 正常終了時は全専用ControlMasterを終了する。
- 異常終了後に残った専用ソケットは、次回起動時に `-O check` で実体を確認してから回収する。単にsocketファイルを削除して生存中のmasterを孤立させない。

## Persistence

- 永続化対象は接続先への付加情報と保存済み転送ルールであり、認証情報や実行状態ではない。
- 初期実装ではTOMLなど人間が確認できる単一設定ファイルで十分であり、DBを導入しない。
- LinuxではXDG Base Directoryに従う。
- 設定更新は一時ファイルへ書いてからrenameするなど、途中終了で壊れにくい方法を使う。
- 実行中PIDだけを永続的な真実として保存しない。状態はControlMasterへ問い合わせて再構築する。

## TUI Requirements

MVPは少なくとも次の2ペイン構成を持つ。

```text
┌ Targets / Sessions ─────┬ Forwards ─────────────────────────────┐
│ ● dev-server            │ web   127.0.0.1:8080 → 127.0.0.1:3000 │
│ ○ research-server       │ db    127.0.0.1:5432 → db:5432        │
└─────────────────────────┴───────────────────────────────────────┘
 Status / error / key help
```

- 左ペインは接続先とセッション状態を表示する。
- 右ペインは選択中セッションの転送を表示する。
- 接続、切断、転送追加、転送削除、再確認、終了をキーボードで行える。
- 破壊的操作は対象が明確になる確認表示を行う。
- 色だけに依存せず、記号または文字でも状態を示す。
- OpenSSHによる対話が必要な場合、TUIを安全にsuspend/resumeする。
- 小さい端末では詳細を省略しても、ローカルポートとリモートポートは確認できるようにする。

## Security Requirements

- SSH認証と暗号処理はOpenSSHへ委譲する。
- パスワード、秘密鍵、鍵パスフレーズを取得・保存・ログ出力しない。
- `StrictHostKeyChecking=no` や `UserKnownHostsFile=/dev/null` を自動指定しない。
- コマンド実行に `/bin/sh -c` 等を使わない。
- Hostエイリアス、アドレス、ラベルを個別の引数・データとして扱い、コマンド文字列へ連結しない。
- `-` で始まる不正なHostエイリアスや、改行・NULを含む入力を拒否する。
- 既定のbind addressは `127.0.0.1` とする。
- 外部公開bindは明示操作と警告を必要とする。
- ControlPathディレクトリを他ユーザーが書き込める場所に置かない。
- ログには秘密情報や環境変数全体を含めない。

## Failure Handling

- 外部コマンドの失敗をpanicへ変換しない。
- ユーザー向けの短い要約と、必要に応じて確認できるOpenSSH stderrを分ける。
- 追加処理の途中で失敗した転送を `Active` として残さない。
- ControlMaster切断後に `-O cancel` が失敗しても、定義状態と実行状態を分離して表示する。
- ポート探索には上限を設け、無制限にスキャンしない。
- 接続失敗時に認証方式やホスト鍵検証を勝手に変更して再試行しない。

## Technology Direction

- Language: Rust stable
- TUI: `ratatui`
- Terminal backend: `crossterm`
- Serialization: `serde`
- Error representation: `thiserror` または同等の明示的なエラー型
- Logging: `tracing` または同等の構造化ログ
- SSH backend: システムの `ssh` コマンド

非同期ランタイムは、プロセス監視とTUIイベント処理の必要性を確認して選ぶ。依存追加そのものを目的にTokio等を導入しない。

## Extensibility Boundary

- TUIはOpenSSHの具体的なコマンドラインを知らない。
- アプリケーション層は「セッションの開始・終了・状態確認」と「ローカル転送の追加・削除」を別の能力として扱う。
- Moshは将来、セッション起動能力として追加できるが、ローカル転送能力を実装しない。
- すべてのバックエンドに同じ機能があるという前提を置かない。
- MVPではバックエンドの動的ロードや外部プラグインAPIを作らない。

## Testing Requirements

### Unit Tests

- Hostエイリアスの抽出と入力検証
- ControlPath IDの安定性と衝突回避
- OpenSSH argvの構築
- IPv4、ホスト名、IPv6を含む転送指定の正規化
- セッションと転送の状態遷移
- ポート候補選択の上限
- 設定ファイルのround-trip

### Adapter Tests

- テスト用の偽 `ssh` 実行ファイルを使い、引数、終了コード、stdout、stderrを検証する。
- 接続成功、接続失敗、check失敗、forward失敗、cancel失敗を再現する。
- シェル展開が起きず、入力が単一引数として渡されることを検証する。

### Integration Tests

- 利用可能な環境ではローカルのテスト用sshdを使う。
- ControlMasterの開始、`-O check`、転送追加、実通信、取消、終了を確認する。
- 統合テストはユーザーの実際の `~/.ssh/config` や既存ControlMasterを変更しない。

### TUI Tests

- 主要状態の描画を固定サイズのバッファで検証する。
- 接続、追加、削除、エラー詳細表示のイベント遷移を検証する。
- 小さい端末サイズでもpanicしないことを検証する。

## MVP Acceptance Criteria

- `~/.ssh/config` の具体的なHostをTUIで選択できる。
- 選択したHostへ専用ControlMasterを開始できる。
- 接続状態を `-O check` で確認できる。
- `127.0.0.1:<local>` からリモート側 `<host>:<port>` への転送を追加できる。
- ポート競合時に別ポートを選び、実際のローカルポートを表示できる。
- 転送を個別に取消できる。
- 転送ルールを接続先ごとに保存し、再起動後に再利用できる。
- SSHセッションを終了できる。
- 認証情報を保存せず、SSH設定の安全性を低下させない。
- 失敗時に原因確認に必要なOpenSSH stderrへアクセスできる。
- 正常終了時に本ツール所有のControlMasterを残さない。

## Engineering Rules for Agents

- OpenSSHの挙動を推測で実装せず、対象バージョンの `ssh(1)` / `ssh_config(5)` と実コマンドで確認する。
- SSHライブラリの導入や独自TCP中継への変更は、要件変更として明示的に合意されない限り行わない。
- ユーザーの `~/.ssh/config`、known_hosts、秘密鍵をテストで変更しない。
- 外部コマンドは必ずargv配列として実行し、シェルを介さない。
- エラーを握りつぶさず、状態とユーザー向けメッセージへ反映する。
- TUI描画、ドメイン状態、OpenSSH実行を同じモジュールに混在させない。
- 新機能を追加する際は、MVPの非目標と `PLAN.md` の順序を確認する。
- 振る舞いを変更した場合は、対応するテストと文書も更新する。
- 将来機能のためだけの抽象化を追加しない。現在必要な境界を保ち、後から置換可能にする。
