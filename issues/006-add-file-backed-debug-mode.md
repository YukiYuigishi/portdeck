# Add a file-backed DEBUG mode

- Status: Proposed
- Priority: Medium
- Reported: 2026-08-09
- Component: `cli`, `logging`, `application`, `ssh`, `tui`

## Summary

今後の障害調査で、接続・転送・状態遷移・終了回収の流れを再現しやすくするため、`portdeck --debug`でDEBUG levelの構造化ログを有効化する。

TUIの描画中にstderrへログを出すと画面を壊す可能性があるため、DEBUGログは端末ではなく所有者限定のファイルへ保存する。通常実行のUIとOpenSSHによる対話入力は変更しない。

## Current behavior

現在は起動直後にprocess-wideのcompact tracing subscriberを初期化し、stderrへINFO以上を出力している。CLI引数解析より先にloggingを初期化しているため、`--debug`の追加時は引数を先に解釈し、選択したlogging modeでsubscriberを構築する必要がある。

ログ地点は起動、OpenSSH version、設定解決件数、runtime回収、shutdown errorに限られている。DEBUG modeを有用にするには、秘密情報を避けながら主要操作にもinstrumentationを追加する。

## CLI behavior

- `portdeck --debug`: TUIを起動し、DEBUGログをファイルへ保存する。
- `portdeck --debug --diagnose`: OpenSSH診断にも同じlogging modeを使用できるようにする。
- `--help`へDEBUG mode、ログ保存先、秘密情報を含めない方針を記載する。
- 未知の引数や重複した不正な組合せは、現在と同様に非zeroで終了する。
- `RUST_LOG`だけに利用方法を依存せず、ユーザー向けの安定した`--debug`を提供する。

## Log location and lifecycle

- `$XDG_STATE_HOME/portdeck/`を優先する。
- `XDG_STATE_HOME`が未設定なら`$HOME/.local/state/portdeck/`を使用する。
- directoryは所有者だけがアクセスできるmode `0700`、log fileはmode `0600`で作成する。
- 1実行ごとにtimestampとPIDを含む`debug-<timestamp>-<pid>.log`を作り、同時実行で上書きしない。
- 作成したログパスをalternate screenへ入る前とTUIのstartup noticeに表示する。
- 無制限に増えないよう、portdeck自身の命名規則に一致する古いDEBUGログだけを上限付きで整理する。別名のユーザーファイルは削除しない。
- logging初期化または保存先作成に失敗した場合は、DEBUG modeを装って続行せず、端末へ明示的なエラーを返す。

## Debug events

少なくとも次を時刻、level、component、operation IDとともに記録する。

- アプリversion、OpenSSH version、対象件数
- SSH設定解決、runtime回収、shutdownの開始・終了と件数
- connect、check、disconnectの開始、終了、所要時間、終了ステータス
- local／SOCKS forwardのadd、cancel、候補port試行、最終状態
- セッションと転送の状態遷移。ただし古い非同期結果を無視した場合も理由を記録する。
- 永続設定のload、save、rollbackの成否
- TUIのcommand種別と対象ID。生のkey eventやフォーム入力文字列は記録しない。
- panic時に可能な範囲でpanic locationと要約をflushする。

operation IDは一連の開始・終了を対応付ける実行時識別子であり、永続的なセッションIDや認証情報として扱わない。

## Security and privacy

DEBUG modeでも次を記録しない。

- パスワード、秘密鍵、鍵パスフレーズ、SSH agentの内容
- 環境変数全体
- OpenSSHのraw stderr／stdout全文
- 入力された生のキーイベント
- 秘密鍵ファイルの内容、known_hostsの内容
- 任意の外部コマンドのshell文字列

OpenSSH操作はoperation種別、target ID、終了コード、所要時間、分類済みエラー種別を基本とする。host alias、転送先、設定パスなど運用情報を追加する場合も、mode `0600`のログであることを前提とし、必要最小限にする。

DEBUG modeは`StrictHostKeyChecking`、暗号方式、認証方式、ProxyJump、forward許可などOpenSSH設定を変更しない。

## TUI behavior

- DEBUGログをTUI描画中のstdout／stderrへ流さない。
- 起動時のStatusへ短く「DEBUG log: <path>」を表示する。
- エラー詳細画面からログパスを確認できるようにするが、ログ全文をTUIへ読み込むviewerは初期実装に含めない。
- 通常のstatus/error表示は維持し、DEBUG modeがないと原因を確認できない設計にしない。

## Non-goals

- SSH認証情報や通信内容のpacket capture
- OpenSSHの`-vvv`を常時自動指定すること
- ログのネットワーク送信やissueへの自動添付
- TUI内の汎用ログviewer
- ユーザー指定の任意パスへroot権限で書き込む機能
- DEBUG modeを通常時の永続設定にして自動有効化すること

## Testing

- `--debug`と`--debug --diagnose`のCLI解析
- XDG Stateとfallback pathの解決
- directory `0700`、file `0600`、同時実行時の一意なファイル名
- TUI実行中にDEBUG eventがstderrへ出ず、ログファイルへ出ること
- connect、forward、persistence rollback、shutdownの相関可能なイベント
- raw key、環境変数全体、fake password／passphrase、raw OpenSSH stderrがログに含まれないこと
- 古いログ整理がportdeck所有パターン以外のファイルを変更しないこと
- logging初期化失敗時の明示エラー

## Acceptance criteria

- [ ] `portdeck --debug`でTUIを起動できる。
- [ ] DEBUGログがXDG State配下の実行ごとのファイルへ保存される。
- [ ] ログdirectoryは`0700`、fileは`0600`である。
- [ ] ログパスを起動時とTUI上で確認できる。
- [ ] SSH・forward・永続化・shutdownの開始と結果を追跡できる。
- [ ] DEBUGログがTUI画面を崩さない。
- [ ] 認証情報、環境変数全体、生キー入力、raw OpenSSH出力を記録しない。
- [ ] DEBUG modeがOpenSSHの安全設定や接続挙動を変更しない。
- [ ] ログ保持数に上限があり、所有外ファイルを削除しない。
- [ ] CLI、permission、redaction、TUI非干渉の回帰テストがある。
- [ ] READMEへ利用方法とログ保存先を記載する。
