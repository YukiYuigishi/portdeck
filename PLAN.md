# PLAN

この文書はportdeckの現在地、次の優先作業、リリース条件、将来候補を追跡する。
安定したアプリケーション要件は[SPEC.md](SPEC.md)、開発方法は
[AGENTS.md](AGENTS.md)、個別作業の経緯と検証結果は[issues/](issues/)を正とする。
完了した実装の詳細はresolved issueとGit履歴に残し、ここでは繰り返さない。

## Current Status

Linux向けportdeck 0.1.0のMVP機能は実装済みであり、release hardeningの段階にある。

- `~/.ssh/config`と再帰的な`Include`から具体的なtargetを検出する。
- targetごとにportdeck専用のOpenSSH ControlMasterを開始、確認、終了する。
- Local (`ssh -L`)とSOCKS (`ssh -D`)の転送ルールを保存、編集、追加、取消する。
- 保存済みの定義とControlMaster上の実行状態を分離し、起動時に残存socketを確認する。
- TUIでtarget検索、Vim風pane移動、エラー詳細、接続後の安全な復帰を提供する。
- `--diagnose`と所有者限定のfile-backed `--debug` logを提供する。

次の個別作業は完了している。

- [Issue 001: 接続後のTUI復帰](issues/001-tui-does-not-resume-after-connect.md)
- [Issue 002: targetのHost alias検索](issues/002-filter-targets-by-host-alias.md)
- [Issue 003: 保存済みforward ruleの編集](issues/003-edit-saved-forward-rules.md)
- [Issue 004: Local／SOCKS forwardの選択](issues/004-select-local-or-socks-forward.md)
- [Issue 005: `h`／`l`によるpane移動](issues/005-use-h-and-l-for-pane-navigation.md)
- [Issue 006: file-backed DEBUG mode](issues/006-add-file-backed-debug-mode.md)

### Verified baseline

- OpenSSH 9.6p1を使用する隔離sshd環境でControlMaster lifecycleを確認済み。
- direct接続とProxyJump接続でLocal／SOCKS forwardingと実TCP通信を確認済み。
- SOCKS5 handshake、forward取消後のlistener閉鎖、ControlMaster終了を確認済み。
- 隔離sshdの公開鍵認証では、ユーザーのSSH設定、鍵、`known_hosts`を変更しない。
- unit、adapter、TUI buffer、CLI、DEBUG、隔離sshd integration testを整備済み。

ProxyJumpは実装・隔離検証とも完了しており、release gateには残さない。
最低対応OpenSSH versionと対話的な認証・host keyの互換性は未確定である。

## 0.1.0 Release Gates

以下は0.1.0のrelease判断前に、再現手順と結果をissueへ記録する。
要件を変更して延期する場合は、理由と利用者への影響を明記する。

### OpenSSH compatibility

- [ ] 最低対応OpenSSH versionの候補を決め、必要なControlMaster操作を実機で確認する。
- [ ] Linux上の複数OpenSSH versionでdiagnose、connect、check、forward、cancel、exitを確認する。
- [ ] version固有の制約を[SPEC.md](SPEC.md)とユーザー文書へ反映する。

### Authentication interaction

隔離sshdの鍵ファイルを直接指定する公開鍵認証は検証済みである。次を追加で確認する。

- [ ] `ssh-agent`に登録した鍵で接続でき、portdeckが鍵素材を取得しないことを確認する。
- [ ] 鍵のpassphrase入力中にTUIを安全にsuspendし、接続後に復帰することを確認する。
- [ ] keyboard-interactive認証の端末引渡し、成功、取消、失敗を確認する。

### Host key interaction

- [ ] 初回host key確認をOpenSSHへ引き渡し、応答後にTUIへ復帰することを確認する。
- [ ] host key不一致を緩和せず、失敗状態とOpenSSHの診断を表示することを確認する。
- [ ] テストでは隔離した`known_hosts`だけを使用する。

### Forwarding refusal

- [ ] sshdがTCP forwardingを禁止した環境でLocal forwardの失敗を確認する。
- [ ] 同じ環境でSOCKS forwardの失敗を確認する。
- [ ] 失敗したforwardが`Active`にならず、再操作可能であることを確認する。

## Next

1. Release gateごとに独立したissueを作り、再現環境と対象OpenSSH versionを定義する。
2. 最低version候補と最新の利用可能versionでOpenSSH compatibility matrixを実行する。
3. 認証とhost keyのPTY testを、ユーザー環境から隔離して実行する。
4. forwarding拒否用の隔離sshd設定を追加し、Local／SOCKS双方を確認する。
5. 結果を仕様と日英ユーザー文書へ反映し、0.1.0 release readinessを再評価する。

未完了gateの実施中に設計変更が必要になった場合は、実装前にissueを更新する。
security boundary、永続形式、module責務などの重要な判断を変える場合はADRも作成する。

## Post-MVP Backlog

以下は0.1.0 MVPのrelease gateではない。利用要求と優先度を確認してからissue化する。

### Remote Port Discovery

- [ ] 既存ControlMaster経由で`ss -H -ltn`を実行し、listen addressとportを表示する。
- [ ] 検出したportからforward追加へ進む導線を設計する。
- [ ] 手動／定期更新、port消失、権限不足の表示を決める。

### Protocol Hints

- [ ] `tcp`、`http`、`https`の表示用hintをforward ruleへ追加する。
- [ ] 明示操作によるURL copyやbrowser起動を検討する。
- [ ] TCP forwarding自体はHTTP解析やTLS終端を行わない方針を維持する。

### Session Profiles

- [ ] 接続時に有効化するforward setを明示的に定義できるようにする。
- [ ] opt-inの自動接続／自動有効化を設計する。
- [ ] TUI終了後もControlMasterを維持するdetach機能の要否を検討する。

### Mosh

- [ ] 対象となる利用場面と、SSH session／forwardingとの能力差を整理する。
- [ ] SSH target定義の再利用範囲を決める。
- [ ] Moshがport forwardingを提供しないことをUIと状態modelへ反映する。

### Additional SSH Forward Types

- [ ] 利用要求が確認できた場合に限りremote forwarding (`ssh -R`)を検討する。
- OpenSSH dynamic forwarding (`ssh -D`)はIssue 004で実装済み。

## Deferred Decisions

- 最低対応OpenSSH version
- macOSを正式な対応対象へ含める時期と検証範囲
- TUI終了後もsessionを残すdetach機能
- remote port discoveryの更新方式と間隔
- Moshを同じbinaryへ含めるか、独立した機能として提供するか
- remote forwarding (`ssh -R`)を製品scopeへ含めるか

## Architecture Decision Records

現在の主要な判断は[docs/adr/](docs/adr/)に記録する。

- [ADR-0001: システムOpenSSHへSSH処理を委譲する](docs/adr/0001-delegate-ssh-to-system-openssh.md)
- [ADR-0002: targetごとに専用ControlMasterを使用する](docs/adr/0002-use-dedicated-controlmaster-per-target.md)
- [ADR-0003: 保存済み定義と実行状態を分離する](docs/adr/0003-separate-saved-rules-from-runtime-state.md)
- [ADR-0004: SOCKS forwardingをOpenSSH `-D`へ委譲する](docs/adr/0004-use-openssh-dynamic-forwarding-for-socks.md)
