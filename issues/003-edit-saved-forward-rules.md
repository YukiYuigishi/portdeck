# Edit saved forward rules

- Status: Proposed
- Priority: Medium
- Reported: 2026-08-09
- Component: `application`, `config`, `tui`

## Summary

保存済み転送ルールを削除・再作成せず、既存フォームへ現在値を読み込んで編集できるようにする。

技術的には実装可能である。`ForwardRule`は永続的な定義、`ActiveForward`はOpenSSHへ追加済みの実行状態なので、編集時に両者の対応を壊さないことが主な設計条件となる。

## Proposed scope

- Forwardsペインで保存済みルールを選択し、`e`で編集フォームを開く。
- 既存のエラー詳細表示は、利用頻度が低い操作として`e`から`E`へ移す。
- ラベル、ローカル側bind address、希望ローカルポート、リモート宛先host、リモート宛先portを編集できる。
- 既存の`ForwardRuleId`と所属する接続先は維持する。
- 入力検証、既定値、外部公開bindの警告は追加フォームと共通にする。
- 保存成功後もルールの選択位置を維持する。

## Active rule policy

MVPの編集では、`Active`、`Adding`、`Removing`、`Unavailable`、`Failed`など実行状態または実際のローカルポートが残っているルールを編集不可とする。先に`Space`で転送を取消し、`Inactive`になってから編集する。

OpenSSHの`-O cancel`と`-O forward`を組み合わせても、旧転送取消後に新転送追加が失敗する可能性があり、定義更新と実行状態を原子的には変更できない。自動取消・再追加とロールバックは別機能として扱い、初期実装へ含めない。

ラベルだけの変更はOpenSSH転送指定に影響しないが、フィールドごとに例外を設けず、初期実装では同じInactive制約を適用する。

## Application behavior

- `AppState`へ、rule IDと検証済み`ForwardRuleDraft`を受け取る更新操作を追加する。
- 対象ruleが存在し、実行状態が完全にInactiveであることを更新前に確認する。
- rule IDとtarget IDを変えず、編集可能な定義フィールドだけを置換する。
- 永続化に失敗した場合はメモリ上の旧定義へ戻し、成功したように表示しない。
- 定義更新だけではSSHセッションへforwardを追加しない。編集後も保存済みルールはInactiveのままとする。

## Failure handling

- Activeなルール: 「転送を取消してから編集してください」と表示する。
- 永続化失敗: 旧定義を維持し、設定保存エラーの詳細を`E`で確認できるようにする。
- 入力不正: フォームを閉じず、該当する検証エラーを表示する。
- 外部公開bind: 追加時と同じ明示確認を要求し、取消時は旧値へ戻る。

## Non-goals

- ルールを別の接続先へ移動する操作
- Activeな転送の自動取消・再追加
- 複数ルールの一括編集
- 外部エディタの起動
- 設定ファイルの直接編集UI

## Testing

- 全フィールドを更新してもrule IDとtarget IDが維持されること
- Inactive以外のルールを編集できないこと
- 永続化成功のround-tripと、永続化失敗時のメモリ状態ロールバック
- 外部公開bind警告の確認・取消
- 編集フォームの初期値、Tab移動、Enter保存、Esc取消
- 通常画面で`e`が編集、`E`がエラー詳細として衝突せず動作すること
- 編集後のルールを`Space`で有効化すると新しい転送指定が使われること

## Acceptance criteria

- [ ] 保存済みのInactiveな転送ルールをTUIから編集できる。
- [ ] `e`で編集し、`E`で従来のエラー詳細を表示できる。
- [ ] 編集フォームへ現在値が正しく読み込まれる。
- [ ] rule ID、所属する接続先、選択位置が維持される。
- [ ] Activeまたは実行データが残るルールは編集を拒否され、取消方法が表示される。
- [ ] 外部公開bindには追加時と同じ警告が表示される。
- [ ] 永続化失敗時に旧定義が維持される。
- [ ] 編集後の有効化で新しいOpenSSH転送指定が使われる。
- [ ] application、設定round-trip、TUIイベントと描画の回帰テストがある。
- [ ] READMEとTUIのkey helpが新しいキー割当を説明している。
