# portdeck Application Specification

この文書は、portdeck 0.1系における現在のアプリケーション仕様を定める。
ユーザー向けの操作説明は[README](README.ja.md)と
[使い方](docs/ja/usage.md)、今後の作業順序と未完了事項は
[PLAN](PLAN.md)を参照する。個別変更の背景と検証履歴は`issues/`、重要な
設計判断とその理由は`docs/adr/`で管理する。

## Product Goal

portdeckは、リモート開発時にOpenSSHの接続とポートフォワードを組み立て、
複数のSSHプロセス、ローカル待受port、リモート宛先を人手で追跡する負担を
減らすLinuxおよび検証済みmacOS向けTUIである。

ユーザーは`~/.ssh/config`の接続先を選び、次の操作をTUI内で完結できる。

1. portdeck専用のSSHセッションを開始、確認、終了する。
2. Local転送（OpenSSH `-L`）またはSOCKS転送（OpenSSH `-D`）のルールを
   保存、編集、削除する。
3. 保存済みルールを現在のセッションへ追加し、個別に取り消す。
4. 接続状態、転送状態、実際のローカル待受、Local転送のリモート宛先を
   一画面で確認する。
5. ポート競合やOpenSSHの失敗理由を確認する。

通信、認証、暗号化、ホスト鍵確認、ProxyJump、SOCKS処理、TCP中継は
システムのOpenSSHへ委譲する。portdeck自身はSSH client、ターミナル
エミュレーター、認証情報管理ツール、独自proxyではない。

## Product Positioning

現在の製品境界は「OpenSSH接続・Local／SOCKSフォワード管理TUI」である。
OpenSSHの既存能力を安全に操作し、定義状態と実行状態を分けて可視化する
ことを価値の中心に置く。

将来機能のために現在の能力を過度に一般化しない。未実装機能の候補と
優先順位は[PLAN](PLAN.md)だけで管理する。

## Terminology

- **接続先（target）**: SSH設定から検出した具体的な`Host`エイリアス。
- **SSHセッション（session）**: portdeckが接続先ごとに所有するOpenSSH
  ControlMaster接続。
- **転送ルール（forward rule）**: 接続先に属して永続化される転送定義。
- **Local転送**: ローカル側の固定listenerから、リモート側から見た
  `host:port`へraw TCPを転送するOpenSSH `-L`の能力。
- **SOCKS転送**: ローカル側にSOCKS4/5 listenerを作るOpenSSH `-D`の能力。
  SOCKS protocolはOpenSSHが処理する。
- **有効な転送（active forward）**: 保存済みルールを現在のControlMasterへ
  追加した実行状態。
- **ローカル側**: portdeckとOpenSSH Clientを実行している端末側。
- **リモート側**: OpenSSH Server（`sshd`）が動作する接続先側。
- **リモート宛先**: Local転送でリモート側から接続する`host:port`。
- **希望ローカルport**: ルールに保存された最初のport候補。
- **実ローカルport**: OpenSSHが実際に転送へ使用したport。
- **定義状態**: 設定ファイルに保存された転送ルール。
- **実行状態**: 現在のControlMasterと、その配下へ追加した転送の状態。

`localhost`だけではどちら側か曖昧になるため、UI、ログ、文書では可能な
限り「ローカル側」「リモート側」を明記する。

## Supported Environment

- 第一対象はLinux clientである。
- macOSは、macOS 26.5 arm64とシステムOpenSSH 10.2p1の組み合わせを
  検証済みbaselineとして対応する。他のmacOS versionとmacOS x86_64は
  未検証である。
- `PATH`から実行できるシステムOpenSSH Clientの`ssh`を必要とする。
- リモート側は標準的なOpenSSH Serverを前提とし、専用agentやdaemonを
  要求しない。
- 実通信を含む現在の統合テスト実績はLinux上のOpenSSH 9.6p1、および
  macOS 26.5 arm64上のOpenSSH 10.2p1である。最低対応OpenSSH versionは
  未確定である。
- Windows nativeはControlMaster、terminal制御、path設計を行うまで
  サポート外とする。

未完了の互換性検証は[PLAN](PLAN.md)のrelease gatesで管理する。

## Current User Flow

1. portdeckを起動すると、SSH設定から接続先と保存済みルールを読み込む。
2. OpenSSH versionと各接続先の有効設定を確認し、前回残った専用runtime
   entryを安全に回収する。
3. ユーザーが接続先を選び、`c`で専用ControlMasterを開始する。
4. OpenSSHの対話が必要な間はTUIを停止し、完了後にTUIを全面再描画する。
5. ユーザーがLocalまたはSOCKSの転送ルールを追加する。追加時点では
   ルールは保存されるだけで、有効化されない。
6. Forwardsペインで`Space`を押すと、portdeckがローカルport候補を決め、
   現在のControlMasterへ転送追加を依頼する。
7. TUIが保存済み定義、実行状態、OpenSSHが受理した実ローカルportを表示する。
8. ユーザーが転送を個別に取り消し、またはSSHセッションを終了する。
9. 通常終了時、portdeckは所有する全ControlMasterを確認して明示的に終了する。

保存済みルールの自動接続、自動有効化、detachは行わない。

## Functional Requirements

### Target discovery and effective configuration

- `$HOME/.ssh/config`と、そこから再帰的に参照される読み取り可能な
  `Include` fileから具体的な`Host`エイリアスを列挙する。
- root configが存在しない場合は空の接続先一覧として扱う。
- `Host *`、wildcardを含むpattern、否定pattern、optionのように`-`で
  始まる値、空白や制御文字を含む値は接続候補として表示しない。
- Include循環と重複fileを安全に処理し、HostエイリアスはASCIIの大文字
  小文字を区別せず重複排除する。最初に現れた表記とsource fileを保持する。
- HostName、User、Port、ProxyJumpなどの意味を独自実装しない。各aliasを
  `ssh -G <alias>`へ渡し、有効な接続先を表示用に解決する。
- TUIは解決できた場合に`user@hostname:port`とProxyJumpを表示する。
  一部aliasの解決失敗は、そのtargetの診断として保持する。
- SSH設定とInclude先を読み取り専用として扱い、変更しない。
- `/`検索はHostエイリアスに対するcase-insensitiveな表示filterであり、
  SSH設定とportdeck設定を変更しない。filter後の選択から起動した操作は、
  必ず画面上で選択された実targetへ送る。

### OpenSSH adapter

すべての外部コマンドはshellを介さず、programとargvを個別に渡す。
Hostエイリアス、ControlPath、forward specificationはそれぞれ単一引数とする。

概念上の主要操作は次のとおりである。

```text
ssh -V
ssh -G <host-alias>
ssh -M -N -f -S <control-path> -o ClearAllForwardings=yes <host-alias>
ssh -S <control-path> -O check <host-alias>
ssh -S <control-path> -o ClearAllForwardings=no -O forward -L <spec> <host-alias>
ssh -S <control-path> -o ClearAllForwardings=no -O forward -D <spec> <host-alias>
ssh -S <control-path> -o ClearAllForwardings=no -O cancel -L <spec> <host-alias>
ssh -S <control-path> -o ClearAllForwardings=no -O cancel -D <spec> <host-alias>
ssh -S <control-path> -O exit <host-alias>
```

`ClearAllForwardings=yes`により、ユーザーのSSH設定にある`LocalForward`等を
portdeckの専用masterへ暗黙に混在させない。動的な追加と取消では
`ClearAllForwardings=no`を指定し、portdeckが追跡する転送だけを操作する。

OpenSSHのstdout、stderr、終了statusは構造化した結果として上位層へ返す。
connectは対話のためstdin／stdoutをterminalから継承する。stderrはmode `0600`で
作成直後にunlinkした一時regular fileへ記録し、background masterやProxyJumpが
descriptorを保持しても接続用processの終了後にTUI復帰を妨げない。それ以外の
管理操作は出力をcaptureする。

### Session lifecycle

- targetごとにportdeck専用ControlMasterを最大1つ管理する。
- connectは`Disconnected`または`Failed`から開始し、OpenSSH起動後に必ず
  `-O check`を行う。check成功前に`Connected`と表示しない。
- 認証、host key確認、鍵passphrase、ProxyJumpはOpenSSHへ委譲する。
- 対話中はraw modeとalternate screenを解除してOpenSSHへterminalを渡す。
  OpenSSHの完了後はalternate screen、raw mode、cursor状態を復元し、
  ratatuiの差分bufferを無効化して全cellを再描画する。
- `r`による再確認は`-O check`を使用する。PIDやsocket fileの存在だけで
  接続済みと判断しない。
- checkで切断を検出した場合、sessionを`Disconnected`へ移し、その配下で
  実行中だった転送を`Unavailable`として表示する。
- 再接続時、`Unavailable`なruntime情報は現在のmasterへ持ち越さず
  `Inactive`へ戻す。無条件の自動再接続は行わない。
- 個別切断は確認後に`-O exit`を実行し、成功時に配下のruntime転送を
  非active化する。終了失敗を成功として表示しない。
- `q`、`Ctrl-C`、SIGINT、SIGTERM、SIGHUPによる終了ではterminalを復元し、
  所有sessionを確認して終了する。生存sessionがある通常のquitは確認を出す。

### Forward rule definition

`ForwardRule`はtargetに属する永続定義であり、種類を明示variantとして持つ。

共通入力は次のとおりである。

- label: 任意。表示用途であり、空ならrule IDを表示する。
- local bind address: 既定値`127.0.0.1`。
- preferred local port: 1〜65535。省略可能。
- forward kind: `Local`または`Socks`。

Local固有の入力は次のとおりである。

- remote destination host: 既定値`127.0.0.1`。リモート側から見た宛先。
- remote destination port: 1〜65535、必須。
- preferred local portを省略した場合、remote destination portを最初の
  local候補とする。

SOCKSにはremote destinationを持たせない。preferred local portを省略した
場合は1080を最初の候補とする。

IPv4、hostname、IPv6を受け付け、OpenSSHへ渡す際にIPv6をbracket付きの
曖昧でないforward specificationへ正規化する。NUL、改行、不正なbracket、
port 0は拒否する。

`0.0.0.0`、`::`、`*`へのbindは外部公開の可能性があるため、追加と編集の
保存前に明示確認を要求する。SOCKS listenerにはportdeck独自の認証がなく、
到達可能な利用者がSSH session経由で任意宛先へ接続できる可能性も警告する。

### Add and edit saved rules

- `a`は現在選択中のtargetへ新しいルールを保存する。
- 保存成功後もruntime stateは`Inactive`であり、自動的に転送を開始しない。
- `e`は、runtime転送情報を保持しない`Inactive`なルールだけを編集できる。
- 編集フォームは現在値を引き継ぎ、LocalとSOCKSの種類も変更できる。
- 編集はrule ID、target、一覧上の順序を維持し、転送を有効化しない。
- `Active`、`Adding`、`Removing`、`Failed`、`Unavailable`、または実local
  port等のruntime情報を保持したルールは編集できない。
- add、edit、deleteの永続化が失敗した場合はmemory上の定義変更をrollbackし、
  未保存の状態を成功として表示しない。

### Activate a forward

- 転送を追加できるのは、親sessionが`Connected`のときだけである。
- `Inactive`なルールへ`Space`を押すと、希望portから連続する候補を最大20件
  試す。65535を越えてwrapしない。
- 各候補で一時的なlocal bind確認を行うが、その成功だけでは転送を
  `Active`にしない。
- Localは正規化した`-L <bind>:<actual-local>:<remote>:<remote-port>`、
  SOCKSは`-D <bind>:<actual-local>`をControlMasterへ追加する。
- OpenSSHの終了statusが成功したときだけ、実local port、転送kind、
  正規化済みspecificationをruntime stateへ保存し、`Active`と表示する。
- OpenSSHがport競合を返した場合も次候補を試す。候補を使い切った場合は
  `LocalPortConflict`として失敗する。
- ControlMaster消失はsession切断と転送`Unavailable`へ反映し、その他の
  server拒否は`ForwardRejected`として扱う。

port `0`による自動割当結果の取得には依存しない。最終的な真実はOpenSSHの
転送追加結果である。

### Cancel and delete a forward

- `Active`なルールへ`Space`を押すと、その転送だけを取り消す。
- 取消は追加時のkind、実local port、正規化済みspecificationと現在の定義が
  一致することを確認し、同じ`-L`または`-D`指定を`-O cancel`へ渡す。
- 取消成功後にのみruntime情報を消去して`Inactive`へ戻す。
- 取消失敗時は実local portとspecificationを保持し、UI上だけ成功したように
  見せない。runtime情報が残る`Failed`な取消は同じ指定で再試行できる。
- `D`による定義削除は確認を要求する。runtime転送が残る場合は先に取消を
  成功させ、保存済み定義の削除成功後にだけ一覧から消す。

## Persistence

### Saved configuration

保存先は次のとおりである。

```text
$XDG_CONFIG_HOME/portdeck/config.toml
# XDG_CONFIG_HOME未設定時
$HOME/.config/portdeck/config.toml
```

- `XDG_CONFIG_HOME`が設定されている場合はabsolute pathだけを受け付ける。
- TOML schema version 1を使用し、targetのHost aliasと、そのtargetに属する
  forward ruleを保存する。
- Local ruleはlabel、bind address、希望local port、`kind = "local"`、
  remote host、remote portを保存する。
- SOCKS ruleはlabel、bind address、希望local port、`kind = "socks"`を保存し、
  remote destinationを保存してはならない。
- 明示的な`kind`を導入する前のversion 1 ruleはLocalとして読み込む。
  次回の定義変更による保存時にkindを明示して書き戻す。
- 現在のSSH設定から検出できないtarget sectionも検証後に保持し、既知targetの
  変更によって無関係な保存済み定義を失わない。
- unknown field、重複target、unsupported schema、不正値は診断付きで拒否する。
  現在検出できる既知targetからloadするruleについては、targetをまたぐ重複
  rule IDも拒否する。未知target sectionは各ruleの値を検証して保持するが、
  そのrule IDは既知targetをloadする際の重複判定には含めない。既存fileを
  parseできない場合は上書きしない。
- 更新は同一directory内の一時fileへ書き、file sync、atomic rename、parent
  directory syncを行う。永続化失敗時はmemory上のmutationをrollbackする。

次の値は永続化しない。

- session state、process ID、ControlMasterの生存情報
- ActiveForward、実local port、正規化済みruntime specification
- password、private key、鍵passphrase、SSH agent情報

再起動後はルールだけを復元し、sessionと転送は自動的に開始しない。

### Runtime directory and ControlPath

ControlMaster socketは次の専用directoryへ置く。

```text
$XDG_RUNTIME_DIR/portdeck/
# XDG_RUNTIME_DIR未設定時
<std::env::temp_dir()>/portdeck-<uid>/
# 上記ではControlPathが長すぎる場合
/tmp/portdeck-<uid>/
```

- runtime directoryはabsolute path、current user所有の実directoryでなければ
  ならず、symlinkは拒否する。modeを`0700`に制限する。
- 明示された`XDG_RUNTIME_DIR`は別pathへ置き換えない。そこから作るControlPathが
  長すぎる場合は明確なerrorとする。
- ControlPath名はtarget IDのSHA-256 digestの一部から作る固定長名であり、
  Host aliasをそのまま含めない。
- Unix domain socketのpath長はOpenSSHがsocket作成時に付ける17 byteの一時suffixを
  含めて保守的な100 byte以内とする。このためportdeckが渡すControlPathは
  最大83 byteとする。
- `cm-` namespaceに一致するportdeck専用entryだけを列挙・回収対象にする。

起動時回収では、現在のtargetへ対応する既知socketを`-O check`する。生存中なら
`-O exit`で終了し、OpenSSHがstaleと確認したものだけ削除する。現在のtargetへ
対応付けられない専用namespace entryは削除せず、診断として表示する。
portdeck外で作られたControlMasterを採用・終了しない。

## TUI Requirements

### Layout and presentation

通常画面はTargets / SessionsとForwardsの2ペイン、およびstatus、error、
key helpを持つ。

- TargetsはHost alias、session状態、active forward数、解決済み接続情報を
  表示する。
- Forwardsは選択targetのルールだけを表示し、label、kind、状態、local
  listener、Localの場合はremote destinationを示す。
- 状態は色だけでなく記号と文字でも区別する。
- 狭い端末では表示を短縮しても、Localではlocal／remoteの両port、SOCKSでは
  kindとlocal port、全画面ではquit導線を保持する。極小terminalでもpanic
  してはならない。

### Keyboard and modal behavior

通常画面の規範的なkey mappingは次のとおりである。

| Key | Action |
| --- | --- |
| `Tab` | TargetsとForwardsをtoggle |
| `h` / `←` | Targetsへfocus |
| `l` / `→` | Forwardsへfocus |
| `↑` / `↓` / `j` / `k` | focus中の選択を移動 |
| `/` | Host alias検索を開始 |
| `c` | 選択targetへ接続 |
| `r` | 選択sessionを`-O check`で再確認 |
| `a` | forward rule追加form |
| `e` | 選択したinactive ruleを編集 |
| `Space` | 選択転送を有効化または取消 |
| `D` | 確認後にruleを削除 |
| `d` | 確認後にsessionを切断 |
| `E` | 最新errorまたは起動診断の詳細 |
| `q` / `Ctrl-C` | 所有sessionを終了してquit |

検索中は通常の文字をfilterへ入力でき、`Backspace`で削除、`Enter`で適用、
`Esc`で検索開始前のfilterと選択へ戻す。空filterの適用は全targetを表示する。

追加・編集formでは`Tab`または上下でfieldを移動し、kind fieldの左右、Space、
`l`、`s`でLocal／SOCKSを選択する。`Enter`で保存し、`Esc`で破棄する。
通常画面以外ではform、検索、確認、warning、error detailのmode処理を優先する。

切断、削除、生存sessionを伴うquit、外部公開bindには対象が分かる確認を表示する。
一般の確認は`y`または`Enter`で承認し、`n`または`Esc`で取り消す。error detailは
`Esc`、`E`、`Enter`で閉じる。

### State and error presentation

Session stateは次を区別する。

```text
Disconnected -> Connecting -> Connected -> Stopping -> Disconnected
       ^              |            |             |
       +--------------+------------+-------------+
                      failure -> Failed
```

`Failed`から再接続または停止を試行でき、`-O check`の結果に基づいて
`Connected`または`Disconnected`へ再構築できる。

Forward runtime stateは次を区別する。

```text
Inactive -> Adding -> Active -> Removing -> Inactive
               |         |          |
               +------> Failed <-----+
               +------> Unavailable <+
```

`Unavailable`は親ControlMasterが利用不能になった状態である。古いmasterの
runtime情報を新しいsessionの有効転送として扱わない。

少なくとも次のfailure categoryを区別し、短いstatus summaryと詳細診断を
分離する。

- OpenSSH executableを起動できない
- 接続または認証失敗
- ControlMasterが利用不能
- local port競合
- serverによるforward拒否
- forward取消失敗
- SSH target discoveryまたは有効設定の解決失敗
- persistence、runtime directory、local bind確認などlocal operation失敗

OpenSSH stderrは`E`で確認できる。外部commandの失敗をpanicへ変換せず、
状態と再操作可能なUIへ反映する。

## CLI and Diagnostics

- 引数なしの`portdeck`はTUIを開始する。
- `--diagnose`は`PATH`上の`ssh -V`を実行し、検出したversionを表示する。
- `--debug`はTUIを開始し、DEBUG levelの構造化logを実行ごとのprivate fileへ
  保存する。
- `--debug --diagnose`は診断modeと同じfile-backed loggingを組み合わせる。
- `--help`、`--version`を提供する。unknown、duplicate、競合するmodeは
  non-zeroで終了し、`--debug`はhelp/versionと組み合わせない。

DEBUG logの保存先は次のとおりである。

```text
$XDG_STATE_HOME/portdeck/debug-<timestamp>-<pid>.log
# XDG_STATE_HOME未設定時
$HOME/.local/state/portdeck/debug-<timestamp>-<pid>.log
```

- `XDG_STATE_HOME`はabsolute pathでなければならない。
- directoryはcurrent user所有の実directory、mode `0700`、各fileはmode
  `0600`とする。初期化失敗時はDEBUG modeを装って続行しない。
- 1実行ごとに一意のfileを作り、pathをalternate screenへ入る前とTUIの
  startup notice／error detailに表示する。
- 新しい10件を保持する方針で、厳密にportdeck所有patternと一致する古いlog
  だけを1起動最大64件まで削除する。他名のfileは変更しない。
- operation ID付きで、connect、check、disconnect、forward add/cancel、port
  候補、状態遷移、設定load/save/rollback、runtime回収、shutdownを記録する。
- password、鍵passphrase、private key、environment全体、生key入力、raw
  OpenSSH stdout/stderrを記録しない。panic logではpayloadを省略する。
- DEBUG modeはOpenSSH argv、安全設定、認証、接続挙動を変更しない。

## Architecture

```text
CLI / process startup
  -> target discovery + saved rule load + runtime recovery
  -> TUI
       -> Application State
            |- Target catalog / Session Manager
            |- Saved Rule catalog / Active Forward state
            -> OpenSSH Adapter -> system ssh -> remote sshd
            -> Persistence / Runtime resources
```

### Module responsibilities

- `domain`: target、session、明示的なLocal／SOCKS rule、active forward、failure、
  検査付き状態遷移を表現する。terminalとprocess APIへ依存しない。
- `application`: 定義状態とruntime状態を調停し、session／forward操作、port
  候補、persistence rollback、起動回収を集約する。
- `ssh`: OpenSSH argvの検証・正規化・実行と、stdout、stderr、exit statusの
  構造化を担当する。UI状態を持たない。
- `config`: SSH Host aliasの列挙、`ssh -G`結果の表示用解析、portdeck固有
  TOML ruleのload/saveを担当する。OpenSSH設定の完全な意味解釈を行わない。
- `runtime`: owner-only runtime directory、短いControlPath、専用namespaceの
  stale entry管理を担当する。
- `tui`: key入力、選択、検索、form、modal、状態描画、OpenSSH対話前後の
  terminal suspend/resumeを担当する。OpenSSH argvを生成しない。
- `cli`: 副作用なしにCLI optionとmodeを検証する。
- `logging`: 通常loggingとprivate DEBUG fileのlifecycle、operation ID、
  redaction境界を担当する。
- `main` / `error`: componentの組み立て、startup、shutdownとtop-level error
  reportingを担当する。

## Domain Model

概念上の主要modelは次のとおりである。

```text
Target
  id
  host_alias
  source

Session
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
  kind
    Local { remote_host, remote_port }
    Socks

ActiveForward
  rule_id
  actual_local_port
  kind: Local | Socks
  normalized_spec
  state
  last_error
```

`ForwardRule`は永続定義、`ActiveForward`は現在のsessionに属するruntime
情報である。`requested_local_port`と`actual_local_port`を分けることで、
競合時に別portを選んでもユーザーの希望値を失わない。Local／SOCKSを
明示variantとして持ち、SOCKSに存在しないremote destinationをoptional fieldの
組み合わせで表現しない。

状態変更はdomain/application層に集約し、描画処理から直接変更しない。
非同期実行を導入する場合も、古い操作結果で新しい状態を上書きしてはならない。

## Security Requirements

- SSH protocol、認証、暗号、host key verification、ProxyJump、SOCKS、TCP
  forwardingはシステムOpenSSHへ委譲する。
- `StrictHostKeyChecking=no`、`UserKnownHostsFile=/dev/null`、弱い暗号方式や
  認証方式を自動指定しない。失敗後に安全性を下げて再試行しない。
- `/bin/sh -c`等を使わず、外部commandはargv配列で実行する。
- Host alias、address、labelを検証し、option injection、NUL、改行、曖昧な
  IPv6 specificationを拒否する。
- SSH config、Include、`known_hosts`、private keyを変更しない。
- password、private key、鍵passphraseを取得、永続化、log出力しない。
- listenerは既定で`127.0.0.1`へbindし、外部公開を明示確認なしに行わない。
- ControlPathとDEBUG logのdirectory ownership、実directory、permissionを
  検証し、他userが書き込める場所を採用しない。
- portdeck外のControlMasterやnamespace外fileを採用、終了、削除しない。
- logへ認証情報、environment全体、raw OpenSSH出力を含めない。

## Failure Handling

- 外部command、filesystem、terminalの失敗をpanicへ変換しない。
- user向けsummaryと、OpenSSH stderrまたはlocal error detailを分離する。
- OpenSSHが成功するまでsessionを`Connected`、forwardを`Active`としない。
- 転送追加の途中失敗でruntime情報を有効として残さない。
- 取消失敗、永続化失敗、shutdown失敗をUIだけで成功扱いしない。
- ControlMaster消失を全子forwardへ反映し、定義状態は失わない。
- port探索は最大20件とし、無制限scanやinteger wrapを行わない。
- 壊れた設定を検出した場合は診断して停止し、既存設定を上書きしない。
- runtime回収では`-O check`前にsocketを削除せず、未知entryを保守的に保持する。
- terminal guardは通常終了とerror pathでraw mode、alternate screen、cursorを
  可能な限り復元する。

## Non-Goals and Current Boundaries

現在は次を実装しない。

- RustによるSSH protocol、暗号、認証の再実装
- Rustによる独自SOCKS proxyまたは独自TCP relay
- リモート側agent、専用daemon、remote port discovery
- `-R` remote forwarding、VPN、TUN/TAP
- protocol hint、HTTP解析、TLS終端、HTTPからHTTPSへの変換
- 自動接続、自動転送、TUI終了後もsessionを残すdetach
- Mosh、SFTP file browser、terminal emulator
- SSH key、password、秘密情報の保管
- Windows native対応、汎用plugin system

OpenSSH `-D`への委譲は独自SOCKS実装には含まれず、現在の対応範囲である。
これらの境界を変更する候補は[PLAN](PLAN.md)で管理する。

## Testing Requirements

### Unit tests

- Host alias抽出、再帰Include、循環・重複、入力検証、`ssh -G`解析
- ControlPath IDの安定性、衝突回避、path長、ownershipとnamespace
- OpenSSH argv、単一引数性、Local／SOCKS specification、IPv4／hostname／IPv6
- sessionとforwardの状態遷移、実行状態と定義状態の分離
- port候補の上限と65535境界
- Local／SOCKS formのdefault、validation、public bind判定
- add、edit、delete、persistence失敗時rollback
- TOML schema version 1のround-trip、kindなしLocal migration、unknown target保持
- DEBUG path、permission、retention、redaction、operation correlation

### Adapter tests

偽`ssh` executableを使い、argv、execution mode、exit status、stdout、stderrを
検証する。connect、check、Local／SOCKS add/cancel、disconnectの成功と失敗、
shell展開が起きず入力が単一引数として渡ることを再現可能に確認する。

### TUI tests

- 固定size bufferで主要状態、Local／SOCKS混在、狭い／極小terminalを描画する。
- target検索、pane移動、add/edit form、public bind warning、activate/cancel、
  delete/disconnect/quit確認、error detailのevent遷移を検証する。
- filter後のtargetとForwardsペイン、実際に送るcommandのtargetが一致することを
  検証する。
- OpenSSH対話から復帰した同一frameでも全面redrawされることを検証する。
- PTY上でraw mode、alternate screen、cursor、signal終了時のterminal復元を
  検証する。

### Integration tests

- 隔離したclient設定、専用key、専用known_hosts、local test sshdを使用する。
- ControlMaster start/check、Local転送の実TCP通信とcancel、direct SOCKS5の
  handshake／実TCP通信／cancel、ProxyJump経由のControlMasterとSOCKS5通信、
  session exitを確認する。
- userの実`~/.ssh/config`、`known_hosts`、private key、既存ControlMasterを
  変更しない。
- 環境依存のreal OpenSSH testは通常suiteからopt-inできる形で分離する。

### Project checks

`./scripts/lint.sh`は`cargo fmt --check`と全target／全featureのclippyを実行する。
通常testは`cargo test --all-targets --all-features`で実行する。pre-commit hookは
lint scriptを呼び、CIはlint scriptと通常testを別stepで実行する。振る舞いを
変える変更は対応するtestと、必要な仕様・ユーザー文書を同時に更新する。

## Current Acceptance Criteria

- [x] SSH設定と再帰Includeから具体的なHost aliasを選択できる。
- [x] Host aliasをcase-insensitiveに検索し、filter後も正しいtargetを操作できる。
- [x] 選択targetへ専用ControlMasterを開始し、対話後にTUIを全面再描画できる。
- [x] 接続状態を`-O check`で確認し、切断を転送状態へ反映できる。
- [x] LocalとSOCKSのruleをtargetごとに保存し、再起動後に再利用できる。
- [x] Inactiveなruleをidentityと順序を保って編集できる。
- [x] Localはremote destination、SOCKSはlocal listenerだけを明示variantで表す。
- [x] ポート競合時に限定候補を試し、OpenSSHが受理した実local portを表示できる。
- [x] Local／SOCKS転送を正確な同一specificationで個別に取り消せる。
- [x] active転送を安全に取り消してから保存済みruleを削除できる。
- [x] 外部公開bindに確認と、SOCKSの認証なし公開に追加警告がある。
- [x] sessionを個別終了し、正常終了時に所有ControlMasterを残さない。
- [x] 保存済み定義と実行状態を分離し、設定更新をatomicに行える。
- [x] OpenSSH stderrと分類済みerrorをTUIから確認できる。
- [x] `--diagnose`でOpenSSHを確認し、`--debug`でprivateかつredactedな
  file-backed logを取得できる。
- [x] 認証情報を保存・log出力せず、SSH設定の安全性を低下させない。
- [x] unit、adapter、TUI、CLI／logging、隔離sshd integration testがある。

対応環境を広げる前に必要な追加検証は[PLAN](PLAN.md)に記録する。
