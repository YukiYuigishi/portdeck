# Select local or SOCKS forwarding

- Status: Proposed
- Priority: Medium
- Reported: 2026-08-09
- Milestone: Post-MVP
- Component: `domain`, `application`, `ssh`, `config`, `tui`

## Summary

転送ルールの作成・編集時に、現在のLocal forward（OpenSSH `-L`）とSOCKS proxy（OpenSSH DynamicForward `-D`）を選択できるようにする。

本ツールがSOCKSプロトコルやTCP中継を実装するのではなく、既存の専用ControlMasterへOpenSSHのdynamic forwardingを追加・取消する。認証、暗号化、ProxyJump、宛先へのTCP接続は引き続きOpenSSHへ委譲する。

この機能は、現在Non-GoalとしているSOCKS proxyおよび`-D`をPost-MVP要件へ変更する明示的なスコープ更新である。実装時はプロジェクト文書のNon-Goalsと機能説明も更新する。

## Verified OpenSSH behavior

ローカルのOpenSSH 9.6p1に付属する`ssh(1)`で次を確認した。

- `-D [bind_address:]port`はローカル側にlistenし、OpenSSHがSOCKS serverとして動作する。
- OpenSSHはSOCKS4とSOCKS5をサポートするため、UI上の種別名は特定versionへ限定せず`SOCKS`とする。
- ControlMasterの`-O forward`はforwardingの追加、`-O cancel`は取消に対応する。
- 既定のbindはSSH設定の影響を受けるため、本ツールはLocal forwardと同様に`127.0.0.1`を明示する。

実装時には、隔離sshdを使って`-O forward -D`、SOCKS経由の実通信、同じ指定による`-O cancel`を実コマンドで検証する。ユーザーの実接続先を使った事前検証は接続前段の`Broken pipe`で完了しなかったため、OpenSSH設定を緩和した再試行は行っていない。

## User flow

1. Forwardsペインで`a`を押す。またはissue 003の編集機能で`e`を押す。
2. 転送種別から`Local`または`SOCKS`を選択する。
3. `Local`では従来どおりローカル側bind、希望ローカルポート、リモート宛先host/portを入力する。
4. `SOCKS`ではラベル、ローカル側bind、希望ローカルポートだけを入力する。空欄時のポート既定値は`1080`とする。
5. 保存後、`Space`で選択したルールをControlMasterへ追加・取消する。
6. UIに実際のローカル待受と`SOCKS`種別を表示する。

## OpenSSH operations

追加時はシェルを介さず、引数を個別に渡す。

```text
ssh -S <control-path> \
  -o ClearAllForwardings=no \
  -O forward \
  -D <bind-address>:<actual-local-port> \
  <host-alias>
```

取消時は、追加成功時に保存した同じ正規化済み指定を使用する。

```text
ssh -S <control-path> \
  -o ClearAllForwardings=no \
  -O cancel \
  -D <bind-address>:<actual-local-port> \
  <host-alias>
```

OpenSSHの終了ステータスが成功するまでActiveと表示しない。ポート競合時はLocal forwardと同じ上限付き候補選択を使用し、事前bind確認だけで成功とみなさない。

## Domain and persistence

- 転送種別を文字列や空のremote hostで暗黙表現せず、`Local`と`SOCKS`の明示的なvariantとして扱う。
- 共通フィールドはrule ID、target ID、label、bind address、requested local portとする。
- `Local`だけがremote hostとremote portを持つ。
- `ActiveForward`は追加時の種別と正規化済み指定、actual local portを保持し、同じ形式で取消できるようにする。
- 既存設定に種別がない場合は`Local`として読み込み、現在の設定ファイルを壊さない後方互換migrationを行う。
- 保存済みルールと実行状態の分離、自動有効化しない方針は維持する。

## Security and UX

- SOCKS listenerには本ツール独自の認証機能がないため、既定bindは必ず`127.0.0.1`とする。
- `0.0.0.0`、`::`、`*`など外部公開bindには、追加・編集時にLocal forward以上に明確な警告を表示する。
- UIでは`SOCKS 127.0.0.1:1080`のように表示し、Local forwardのリモート宛先表記と混同させない。
- SOCKS clientが要求した個別宛先や通信内容を本ツールで解析・保存・表示しない。
- SSH設定のDynamicForwardを専用masterへ暗黙に取り込まず、`ClearAllForwardings=yes`の接続方針を維持する。

## Failure handling

- ControlMaster未接続、ローカルポート競合、サーバー・OpenSSHによるforward拒否を既存の状態・エラー表示へ反映する。
- 追加失敗時はActiveにせず、候補探索の上限到達を明示する。
- 取消失敗時はUI上だけInactiveにせず、実行状態と正規化済み指定を保持する。
- 親SSHセッション切断時はLocal forwardと同様にUnavailableとする。

## Non-goals

- 本ツール独自のSOCKS serverやTCP proxy実装
- SOCKS listener用のユーザー名・パスワード認証
- 接続ごとのSOCKS宛先・通信ログ表示
- HTTP proxyへの変換、PAC生成、ブラウザ設定変更
- SSH設定由来の既存DynamicForwardの取込み
- Remote側でlistenするreverse dynamic forwarding

## Testing

- SOCKS bind指定のIPv4、IPv6正規化とargv構築
- fake SSHによるforward成功、競合再試行、cancel失敗
- Local／SOCKSを含む設定ファイルのround-tripと旧形式からのmigration
- 種別切替時のフォーム項目、既定port 1080、外部公開警告
- Local／SOCKSが混在する一覧描画と小さい端末での描画
- 隔離sshd上のControlMasterへ`-O forward -D`を追加し、SOCKS4またはSOCKS5経由で実TCP通信できること
- 同じ指定の`-O cancel -D`後にlistenerが閉じること
- ProxyJump経由のControlMasterでもSOCKS追加・通信・取消が成功すること

## Acceptance criteria

- [ ] 転送ルールの追加・編集時にLocalまたはSOCKSを選択できる。
- [ ] SOCKSルールは既定で`127.0.0.1:1080`を使用する。
- [ ] `Space`でSOCKS listenerを追加・取消でき、実際のローカルポートを表示する。
- [ ] OpenSSHの成功終了前にActiveと表示しない。
- [ ] ポート競合時に上限付きで別候補を試す。
- [ ] 外部公開bindに明示警告が表示される。
- [ ] 既存のLocal forward設定を変更なしで読み込める。
- [ ] 終了時にSOCKS forwardとControlMasterが残らない。
- [ ] direct接続とProxyJump接続の両方でSOCKS実通信を確認する統合テストがある。
- [ ] SOCKS処理はシステムOpenSSHへ委譲され、本ツール内に独自proxyを実装しない。
