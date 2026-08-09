# Filter targets by host alias with `/`

- Status: Resolved
- Priority: Medium
- Reported: 2026-08-09
- Component: `tui`

## Summary

接続先が多い環境で目的のHostエイリアスへ素早く移動できるように、通常画面で`/`を押すと接続先検索を開始できるようにする。

MVPでは、左ペインに表示している具体的なSSH Hostエイリアスを、大文字小文字を区別しない部分一致で絞り込む。OpenSSHの有効な`HostName`、User、ProxyJumpなどは検索対象に含めず、SSH設定の再解釈も行わない。

## User flow

1. 通常画面で`/`を押す。
2. Status領域へ検索入力と一致件数を表示する。
3. 文字入力またはBackspaceのたびにTargetsを絞り込む。
4. `Enter`で現在の絞り込みを確定し、通常操作へ戻る。
5. 絞り込み後も、選択中の接続先に対して`c`、`r`、`d`など既存操作を実行できる。
6. 空の検索を確定すると絞り込みを解除する。

## Key semantics

- `/`: 検索入力を開始する。Forwardsペインにフォーカスがある場合もTargets検索へ移る。
- 通常文字: 検索文字列へ追加する。
- `Backspace`: 末尾の1文字を削除する。
- `Enter`: 現在の検索を確定する。空なら全件表示へ戻す。
- `Esc`: 編集中の検索を破棄し、検索開始前の確定済みフィルターへ戻す。
- `Ctrl-C`: 検索中も既存どおりquit確認として扱い、検索文字には追加しない。

## State and selection rules

- フィルターはTUI内の表示状態であり、接続先定義や保存済み転送ルールへ永続化しない。
- 選択中のHostエイリアスが絞り込み結果に残る場合は、その選択を維持する。
- 選択中のHostエイリアスが結果から外れた場合は、先頭の一致項目を選択する。
- 一致件数が0件でもpanicせず、右ペインを空にして「一致する接続先がありません」と表示する。
- フィルター解除時は可能な限り検索前に選択していたHostエイリアスへ戻す。
- 接続状態と有効な転送数は、絞り込み中も既存と同じ情報を表示する。

## Non-goals

- 正規表現、glob、fuzzy matching
- `ssh -G`で解決したHostName、User、ProxyJumpを横断する検索
- 検索履歴やフィルターの永続化
- 接続先カタログ自体の追加、削除、並べ替え

## Testing

- 大文字小文字を区別しない部分一致
- 入力、Backspace、Enter、Escのモード遷移
- 選択維持と先頭一致へのフォールバック
- 0件表示からの検索編集・解除
- 絞り込み後の`c`、`r`、`d`が表示中のTargetIdへ向くこと
- Forwardsペインから`/`を押した場合のフォーカス遷移
- 小さい端末でも検索文字列と0件状態の描画がpanicしないこと

## Acceptance criteria

- [x] `/`でHostエイリアス検索を開始できる。
- [x] 入力に応じてTargetsが大文字小文字を区別せず部分一致で更新される。
- [x] `Enter`で確定、`Esc`で編集取消、空検索の確定で全件表示へ戻れる。
- [x] 検索結果の選択と右ペインの転送ルールが同じ接続先を参照する。
- [x] 0件でもpanicせず、該当なしと明示される。
- [x] 絞り込み後も既存の接続・確認・切断操作を実行できる。
- [x] 検索はSSH設定と本ツールの永続設定を変更しない。
- [x] TUIのイベント遷移と固定サイズ描画に回帰テストがある。

## Resolution

Implemented in `4f1a8fd`.

- `/`で検索モードへ入り、Hostエイリアスを大文字小文字を区別しない部分一致でライブ絞り込みする。
- 確定済みフィルターと検索開始時のTargetIdをTUI状態だけに保持し、`Enter`、`Esc`、空検索で選択を適切に復元する。
- 描画と接続・確認・切断コマンドは、すべて同じ絞り込み後のTargetId解決を使用する。
- 0件のメッセージ、検索文字列と一致件数、小さい端末の描画を回帰テストした。

## Verification

- `./scripts/lint.sh`: passed
- `cargo test --all-targets --all-features`: 72 unit, 3 CLI, 6 adapter tests passed; 2 opt-in OpenSSH integration tests ignored as designed
