# Use `h` and `l` for pane navigation

- Status: Resolved
- Priority: Low
- Reported: 2026-08-09
- Component: `tui`

## Summary

Vimの移動キーに慣れたユーザー向けに、通常画面で`h`をTargetsペイン、`l`をForwardsペインへの移動として扱う。

既存の`Tab`、`←`、`→`は残し、`j`、`k`による上下選択と合わせてマウスなしで自然に操作できるようにする。

## Key semantics

- `h`: Targetsペインへフォーカスする。既にTargetsなら何もしない。
- `l`: Forwardsペインへフォーカスする。既にForwardsなら何もしない。
- `←`: `h`と同じくTargetsへ移動する。
- `→`: `l`と同じくForwardsへ移動する。
- `Tab`: 従来どおり現在とは反対のペインへ切り替える。
- `j` / `k`: フォーカス中のペイン内で従来どおり選択を上下する。

現在の`←`と`→`はどちらもペインをtoggleする実装だが、方向キーとVimキーを対応させるため、左はTargets、右はForwardsへ向かう操作として明確化する。

## Mode behavior

- `h`と`l`をペイン移動に使うのは通常画面だけとする。
- 転送ルールの追加・編集フォームでは、`h`と`l`を通常の入力文字として扱う。
- `/`検索中も、`h`と`l`を検索文字列へ入力できるようにする。
- 確認、外部公開警告、エラー詳細などのmodal中は既存のmodalキー処理を優先する。
- Targetsに一致項目がない場合やForwardsが空の場合でもpanicせず、フォーカス表示だけを移動する。

## Documentation

- TUI下部のkey helpへ`h/l: pane`を追加する。
- READMEのKeyboard controlsへ`h`、`l`を追加する。
- 小さい端末でkey helpを省略する場合も、既存のquit操作や主要port表示を圧迫しないようにする。

## Testing

- 通常画面で`h`がTargets、`l`がForwardsへフォーカスすること
- 同じペインのキーを繰り返してもtoggleしないこと
- `Tab`は従来どおりtoggleすること
- `←`と`→`が方向どおりのペインへ移ること
- 追加・編集・検索フォームで`h`と`l`を文字入力できること
- 空のTargets／Forwardsと小さい端末でもpanicしないこと

## Acceptance criteria

- [x] 通常画面で`h`によりTargetsペインへ移動できる。
- [x] 通常画面で`l`によりForwardsペインへ移動できる。
- [x] `h`、`l`を繰り返しても意図せずペインがtoggleしない。
- [x] `Tab`、`←`、`→`の既存操作も利用できる。
- [x] フォームと検索中は`h`、`l`を文字として入力できる。
- [x] READMEとTUIのkey helpが新しい操作を説明している。
- [x] TUIイベント遷移と描画の回帰テストがある。

## Resolution

Implemented in `84f88ab`.
Empty-list navigation coverage added in `502c2f3`.

- `h`/`←`はTargets、`l`/`→`はForwardsへ方向を指定して移動する。
- `Tab`のトグルと`j`/`k`の上下移動を維持した。
- 通常画面以外のモード処理を優先し、転送フォームとTarget検索で`h`/`l`を文字入力できることをテストした。

## Verification

- `./scripts/lint.sh`: passed
- `cargo test --all-targets --all-features`: 73 unit, 3 CLI, 6 adapter tests passed; 2 opt-in OpenSSH integration tests ignored as designed
