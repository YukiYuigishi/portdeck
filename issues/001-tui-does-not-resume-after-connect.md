# TUI does not resume after a successful SSH connection

- Status: Open
- Priority: High
- Reported: 2026-08-09
- Component: `tui`, `ssh`

## Summary

TUIで接続先を選択して`c`を押すと、認証・接続用に通常画面へ切り替わるが、SSH接続後もTargets / Forwards画面へ自動的に戻らない。

接続処理が成功または失敗した時点でTUIへ自動復帰するのが期待動作であり、`Ctrl-C`で接続画面を抜ける運用は想定しない。

## Steps to reproduce

1. portdeckを起動する。
2. SSH接続先を選択する。
3. `c`を押す。
4. 必要な認証またはホスト鍵確認を完了する。
5. SSHのログが表示される通常画面からTUIへ戻るか確認する。

## Actual behavior

- SSH接続後も通常画面に留まり、Targets / Forwards画面へ戻らない。
- ユーザーからは、接続処理が継続中なのか、接続済みで待機しているのか判断できない。
- `Space`による保存済み転送ルールの有効化へ進めない。

## Expected behavior

- OpenSSHの接続用プロセスが成功または失敗したら、TUIを自動的にresumeする。
- 成功時は`Connected`を表示し、保存済みルールを選択して`Space`で転送を有効化できる。
- 失敗時もTUIへ戻り、短い要約と`e`で確認できるOpenSSH stderrを表示する。
- 復帰に`Ctrl-C`を必要としない。

## Ctrl-C semantics

- OpenSSHへ端末を引き渡している間の`Ctrl-C`は、進行中のOpenSSH接続を中断する操作である。
- TUI上の`Ctrl-C`は、portdeckの終了操作である。
- 接続成功後にTUIへ戻るためのキーとしては扱わない。

## Investigation notes

現在のinteractive executorは、次の組み合わせで接続プロセスの完了とstderrのEOFを待っている。

```text
ssh -M -N -f -S <control-path> ...
stderr(Stdio::piped()).output()
```

`-f`でbackground化されたControlMaster側にstderr pipeが継承される環境では、接続用の親プロセスが終了してもpipeのEOFが発生せず、`output()`が待ち続ける。

調査開始時の隔離sshd統合テストは独自executorを使用し、fake SSHテストは即時終了するスクリプトを使用していたため、この待機状態を再現できていなかった。

## Verification results

### Background process retains stderr pipe

2026-08-09、隔離fake SSHで接続用親プロセスから30秒生存するbackground子を作り、stderrを継承させた。

- 接続用`ssh`プロセスは終了済みのzombieになった。
- background子はPID 1へreparentされ、stderrの書き側を保持した。
- portdeckは同じpipeの読み側を保持し、TUIをresumeしなかった。
- background子が30秒後に終了してstderrを閉じた直後、portdeckは接続成功としてTUIをresumeした。
- ControlMaster相当の状態は待機中から既に利用可能だった。

これにより、`Command::output()`が接続用プロセスの終了後もstderr EOFを待つことと、当該descriptorの寿命がTUI復帰を直接遅延させることを確認した。

### Isolated real OpenSSH

隔離sshd統合テストを独自executorから本番の`SystemCommandExecutor`経路へ変更し、OpenSSH 9.6p1、公開鍵認証、`LogLevel DEBUG3`で確認した。

- `ssh -M -N -f`は正常に戻った。
- `-O check`、同一master上の複数forward、実TCP通信、個別cancel、`-O exit`まで成功した。
- テスト全体は約0.33秒で完了し、通常のOpenSSH 9.6p1構成ではhangを再現しなかった。

したがって、確認済みの待機機構は実在するが、通常構成の全OpenSSH接続で発生するわけではない。報告環境でbackground側にdescriptorが残る条件、またはresume済みだが旧描画のため復帰していないように見えた可能性を追加で切り分ける。

## Proposed direction

- interactive接続ではpipeのEOFではなく、接続用OpenSSHプロセスの終了ステータスを待つ。
- stderrを保持する場合は、所有者限定の一時regular fileなど、background masterがdescriptorを継承しても呼び出し元の完了を妨げない方法を検討する。
- 一時診断データを使う場合はmode `0600`、確実な削除、ログへの秘密情報混入防止を確認する。
- 接続後の`ssh -O check`とTUI全面redrawは従来どおり実行する。

## Acceptance criteria

- [ ] 実OpenSSHで接続成功後にTUIへ自動復帰する。
- [ ] 認証失敗・接続失敗後にもTUIへ自動復帰する。
- [ ] パスフレーズ、初回ホスト鍵確認、keyboard-interactiveで端末入力できる。
- [ ] 成功後に保存済みルールを`Space`で有効化できる。
- [ ] 失敗時のOpenSSH stderrを`e`で確認できる。
- [ ] background processがstderr descriptorを保持するケースの回帰テストがある。
- [ ] TUI復帰後に画面全体が正しくredrawされる。
- [ ] 終了後に一時診断ファイルや不要なControlMasterが残らない。

## Related work

- `e6ae491 fix(tui): redraw fully after SSH interaction`は、resume後の差分描画崩れを修正したもの。このissueはresume自体へ到達しない別の問題を扱う。
