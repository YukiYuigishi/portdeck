# Use `h` and `l` for pane navigation

- Status: Proposed
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

- [ ] 通常画面で`h`によりTargetsペインへ移動できる。
- [ ] 通常画面で`l`によりForwardsペインへ移動できる。
- [ ] `h`、`l`を繰り返しても意図せずペインがtoggleしない。
- [ ] `Tab`、`←`、`→`の既存操作も利用できる。
- [ ] フォームと検索中は`h`、`l`を文字として入力できる。
- [ ] READMEとTUIのkey helpが新しい操作を説明している。
- [ ] TUIイベント遷移と描画の回帰テストがある。
