# Verify macOS startup compatibility

- Status: Resolved
- Priority: High
- Reported: 2026-08-10
- Started: 2026-08-10
- Resolved: 2026-08-10
- Component: `macos`, `startup`, `runtime`, `logging`, `ssh`, `tui`

## Summary

macOSでportdeckが正常に起動しないという報告を再現し、起動処理のどの段階で
失敗するかを特定する。確認済みの即時blockerであるLinux固有のUID取得を
portableなUnix実装へ置き換えたうえで、macOS上の起動、OpenSSH
ControlMaster、Local／SOCKS forwarding、TUI終了までの互換性を検証する。

調査開始時点の[対応環境](../SPEC.md#supported-environment)ではLinuxが第一対象で
あり、macOSはUnix domain socketとOpenSSHの挙動を検証するまでサポート外だった。
このissueの起票だけではmacOSのsupport statusを変更しないものとした。

## Background

[PLAN](../PLAN.md#deferred-decisions)では、macOSを正式な対応対象へ含める時期と
検証範囲を未決定としている。一方で、実際のmacOS環境からはTUIが正常に
起動しないことが報告されている。具体的な画面表示や終了経路は調査開始時に
再取得し、未確認のエラーを推測で記録しない。

通常起動をTUI開始前に妨げる再現済みのplatform blockerであり、macOS対応可否の
判断にも進めないため、PriorityをHighとする。

macOS対応の判断には、最初のstartup errorを解消するだけでなく、設定・state・
runtime path、Unix domain socket、システムOpenSSH、terminal制御を含む
一連のlifecycle検証が必要である。結果は環境情報と操作ごとに記録し、全項目を
通過するまではmacOSを対応環境と表現しない。

## Initial findings / Actual behavior

Darwin arm64環境で次のtargeted testを実行すると失敗することを確認した。

```console
cargo test runtime::tests::creates_owner_only_runtime_directory -- --exact --nocapture
```

失敗は`RuntimeError::CurrentUser(NotFound)`である。通常起動では`run_tui`から
`RuntimeDirectory::from_environment`へ進み、現在のUIDを
`fs::metadata("/proc/self")`のownerから取得している。macOSにはLinuxの
`/proc/self`を前提にできないため、runtime directory準備がTUI開始前に停止する。

同じLinux固有のUID取得はDEBUG log directoryのownership検証にも存在する。
したがって`--debug`は通常のruntime準備とは独立してlogging初期化時にも影響を
受ける。これは確認済みの即時blockerであるが、修正後のOpenSSH、ControlPath、
terminal lifecycleがmacOSで互換であることまでは証明しない。

## Scope

### Reproduction and classification

- 再現に使用したmacOS version、architecture、terminal application／`TERM`、
  portdeck revision、システム`ssh`のpathとOpenSSH versionを記録する。
- 引数なし起動、`--diagnose`、`--debug`、`--debug --diagnose`の結果を確認し、
  failureをCLI／logging初期化、target discovery、保存設定load、runtime回収、
  OpenSSH probe／設定解決、terminal初期化、TUI event loopのどこで起きるか
  分類する。
- `--diagnose`の出力と`--debug` logを調査に使用する。共有する記録からは
  host alias、address、username、filesystem上の個別識別情報、認証情報、
  raw OpenSSH outputを除去し、redaction後も原因判断に必要なversion、operation、
  error category、終了statusを残す。

### Portable startup behavior

- `runtime`と`logging`にあるLinux固有の現在UID取得を監査し、macOSを含む対象
  Unix上で利用できる方法へ置き換える。共通化の形は実装時に判断するが、両方の
  呼び出し経路で同じportableかつ検証可能なsemanticsを持たせる。
- portable化後もruntime directoryとDEBUG log directoryがcurrent user所有の
  実directoryであることを検査し、symlink拒否、owner-only permission、既存の
  namespace境界を弱めない。
- macOSにおける設定、state、runtime directoryのenvironment解決とfallbackを
  監査する。pathが絶対pathであること、作成・ownership・permission、既存fileの
  安全な扱いを確認する。
- Unix-domain ControlPathの実際のpath長上限、permission、socket作成、stale
  判定と回収を確認する。portdeck外のsocketや、OpenSSHでstaleと確認していない
  entryを削除しない。

### OpenSSH and terminal lifecycle

- macOS付属のシステムOpenSSHについてversion、argv、stdout／stderr、終了status、
  ControlMasterのstart／check／forward／cancel／exit lifecycleを確認する。
  shell command constructionや安全性を下げるSSH optionは導入しない。
- terminalのraw mode、alternate screen、cursor状態について、初期化、OpenSSHへ
  対話端末を渡す前のsuspend、復帰時の全面再描画、通常・error・signal終了時の
  restorationを確認する。
- `--diagnose`、TUI起動、target discovery、connectとcheck、LocalとSOCKSの
  add／cancel、disconnect／exit、cleanなTUI quitを順に確認する。終了後に
  portdeck所有のOpenSSH process、ControlMaster socket、forward listenerが
  残っていないことを確認する。
- 調査結果がrepository側の不具合を示す場合に限り、必要最小限の修正、対応layerの
  regression test、関連文書更新を同じissueで行う。追加の設計判断やscope変更が
  必要なら、product fileを変更する前にこのissueを更新する。

## Non-goals

- 調査前にmacOSの正式supportや最低macOS／OpenSSH versionを宣言すること
- macOS専用のSSH client、SSH protocol実装、独自TCP／SOCKS proxyを追加すること
- host key確認、暗号方式、認証方式、forward policyを自動的に緩和すること
- Windows native対応やLinuxで未対応の機能を同時に実装すること
- ユーザーのSSH設定、`known_hosts`、秘密鍵、既存ControlMasterを変更すること
- connection情報、認証情報、環境変数全体、未加工のDEBUG logやOpenSSH出力を
  issue、test fixture、commitへ保存すること

## Acceptance criteria

- [x] macOSで報告された起動失敗の再現環境とfailure stageが、秘密情報を含まない
  形で記録され、確認したroot causeが推測と区別されている。
- [x] `runtime`と`logging`のLinux固有UID取得がportableなUnix実装へ置き換わり、
  macOSで現在UIDを取得できる一方、directory ownership検証を弱めていない。
- [x] portable UID取得、異なるowner、symlink、permission、path fallbackに対する
  regression coverageがあり、macOSで該当runtime／logging testが成功する。
- [x] config、state、runtime pathとControlPathのmacOS差異を監査し、path長、
  ownership、permission、stale recoveryの結果が記録されている。
- [x] macOS version、architecture、terminal、OpenSSH versionごとのcompatibility
  matrixに、diagnose、startup、target discovery、connect、check、Local／SOCKS
  add・cancel、disconnect／exit、TUI quitの結果が記録されている。
- [x] lifecycleの成功時も失敗時もterminalが復元され、正常終了後にportdeck所有の
  process、socket、listenerが残らないことを確認している。
- [x] repository側の不具合には原因へ対応する最小修正とregression testがあり、
  platformまたは外部要因の場合は制約と利用者への影響が明記されている。
- [x] lifecycle matrixの全必須項目が成功するまでmacOSをsupport対象と表現せず、
  証拠によってsupport statusが変わる場合だけ`SPEC.md`、`PLAN.md`、日英の利用者向け
  文書が同じchangeで更新されている。
- [x] ユーザーのSSH資産とsecretを変更・保存・公開しておらず、OpenSSHのsecurity
  boundaryとportdeck専用resourceのownership境界を維持している。
- [x] [CONTRIBUTING](../CONTRIBUTING.md#checks)のrequired checksが成功し、
  OpenSSH実行挙動を変更した場合は隔離環境のreal OpenSSH integration testも
  macOSで成功している。実行できないcheckは理由と残るriskが記録されている。

## Verification plan

1. 上記の環境情報を取得し、`--diagnose`とsanitizedな`--debug`記録を使って
   startup sequenceを再現する。既知のUID blockerと、その後に現れる問題を分ける。
2. runtime／logging双方のUID取得をportable化し、ownershipとpermissionの
   negative testを含むtargeted testをLinuxとmacOSで実行する。
3. macOSの標準path条件と、明示したXDG path条件でconfig、state、runtime解決を
   確認する。短いControlPath、長すぎるControlPath、stale／unknown entryを
   隔離したtemporary environmentで検証する。
4. fake `ssh` adapter testでargv、単一引数性、exit status、stdout／stderr、
   shell展開がないことを確認する。
5. 隔離したclient設定、key、`known_hosts`、runtime directoryを使用してreal
   OpenSSH lifecycleを実行し、connect／check、Local／SOCKSの実通信とcancel、
   exit、resource cleanupを確認する。
6. PTY上で通常起動、OpenSSH対話からの復帰、clean quit、error path、signal終了を
   実行し、raw mode、alternate screen、cursor、描画状態の復元を確認する。
7. `./scripts/lint.sh`、`cargo test --all-targets --all-features`を実行する。
   OpenSSH実行挙動を変更した場合はmacOSの隔離環境で
   `cargo test --test openssh_integration -- --ignored --test-threads=1 --nocapture`
   も実行する。
8. compatibility matrixと残る制約をResolutionへ記録し、証拠に基づいて必要な
   source of truthだけを更新する。相対Markdown linkと`git diff --check`も確認する。

## Resolution

macOS 26.5 arm64、システム`/usr/bin/ssh`のOpenSSH 10.2p1
（LibreSSL 3.3.6）、自動PTY（`TERM=xterm-256color`）で検証した。この組み合わせを
検証済みbaselineとして対応対象に含める。他のmacOS version、macOS x86_64、最低
対応OpenSSH versionは未検証であり、このissueでは保証しない。

### Root cause and changes

確認したrepository側の原因は2点だった。

1. `runtime`と`logging`がcurrent userのUIDをLinux固有の`/proc/self`から取得し、
   macOSではTUI開始前のruntime準備、`--debug`ではlogging初期化でも失敗していた。
   safeな`rustix::process::geteuid`を共通platform helperから使用するよう変更し、
   ownership、異なるowner、symlink、mode `0700`／`0600`の検証を維持した。
2. macOSのOpenSSH 10.2p1はControlMaster socket作成時に17 byteの一時suffixを
   ControlPathへ付ける。永続ControlPathだけを100 byte以内にしても、suffix込みで
   DarwinのUnix socket上限を超えていた。合計100 byteの保守的budgetからsuffixを
   予約し、OpenSSHへ渡すControlPathを83 byte以内にした。未設定時の
   `std::env::temp_dir()`が長すぎる場合は、owner、実directory、symlink、modeを
   同じ基準で検証する短い`/tmp/portdeck-<uid>`へfallbackする。明示された
   `XDG_RUNTIME_DIR`は暗黙に置き換えず、長すぎるControlPathを明確に拒否する。

高速なmacOS filesystem上で露出したtemporary fixture名の衝突とDEBUG capture testの
並列実行競合も、process IDとatomic counter、および単一subscriberを使って安定化した。
`#![forbid(unsafe_code)]`とOpenSSHへのargv単位の委譲は維持している。

### Compatibility matrix

| 項目 | macOS 26.5 arm64 / OpenSSH 10.2p1 |
| --- | --- |
| `--diagnose` | 成功。システムOpenSSH versionを取得 |
| `--debug --diagnose` | 成功。directory `0700`、file `0600` |
| 通常／`--debug` TUI startup | 成功。設定・state・runtime準備とtarget discoveryを完了 |
| connect / check | 成功。TUI操作と隔離real OpenSSHのControlMasterで確認 |
| Local add / traffic / cancel | 成功。実TCP通信と取消後のlistener閉鎖を確認 |
| SOCKS add / traffic / cancel | 成功。direct／ProxyJumpのSOCKS5実通信と取消を確認 |
| disconnect / exit | 成功。`ssh -O exit`後にControlMaster socketをcleanup |
| clean TUI quit | 成功。PTYを復元し、所有runtimeを空にして終了 |
| SIGTERM TUI quit | 成功。PTY event loopからclean exitし、所有runtimeを空にして終了 |

config／stateのXDG path、runtime fallback、ControlPathの83／84 byte境界、異なる
owner、symlink、permission、専用namespaceだけを対象とするstale recoveryをunit testで
確認した。隔離integration testは生成したclient config、key、`known_hosts`、runtime、
ローカルsshdだけを使用した。検証中にユーザーのSSH設定、`known_hosts`、秘密鍵、
既存ControlMasterを変更せず、接続先や認証情報をrepositoryへ記録していない。

### Verification

- `./scripts/lint.sh`: 成功
- `cargo test --all-targets --all-features`: 成功
  - library 109件、CLI 4件、DEBUG logging 4件、SSH adapter 7件
  - real OpenSSH integration 4件は既定どおりignored
- `cargo test --test openssh_integration -- --ignored --test-threads=1 --nocapture`:
  4件すべて成功
- `cargo run -- --diagnose`: 成功
- runtime／loggingのtargeted ownership、permission、path test: 成功
- 通常quit、SIGTERM、DEBUGを含む隔離PTY lifecycle: 成功
- 変更したMarkdownの相対link検査と`git diff --check`: 成功

実装と記録は`12995ae`、`82d29e8`、`3a7ddf6`、`dcb65c0`、`0068330`に含まれる。
