# Architecture Decision Records

Architecture Decision Record（ADR）は、portdeckに長期的な影響を与える技術判断について、判断が必要になった背景、採用した案、検討した代替案、結果として生じる制約を記録する。

ADRは「なぜこの設計を選んだか」のsource of truthであり、現在のアプリケーション仕様を置き換えない。現在の挙動は[`SPEC.md`](../../SPEC.md)、今後の優先順位は[`PLAN.md`](../../PLAN.md)、個別作業と検証結果は[`issues/`](../../issues/)を参照する。開発workflowは[`AGENTS.md`](../../AGENTS.md)に従う。

## Index

| ADR | Decision | Status |
| --- | --- | --- |
| [0001](0001-delegate-ssh-to-system-openssh.md) | システムOpenSSHへSSH処理を委譲する | Accepted |
| [0002](0002-use-dedicated-controlmaster-per-target.md) | targetごとに専用ControlMasterを使用する | Accepted |
| [0003](0003-separate-saved-rules-from-runtime-state.md) | 保存済みforward ruleと実行状態を分離する | Accepted |
| [0004](0004-use-openssh-dynamic-forwarding-for-socks.md) | SOCKS forwardingをOpenSSH dynamic forwardingへ委譲する | Accepted |

すべての番号付きADRを、番号順にこのindexへ載せる。新規ADRとindex更新は同じcommitへ含め、Status変更時もindexを同期する。

## When to Write an ADR

次のように、将来の保守担当者が採用理由やtrade-offを知る必要がある判断をADRへ記録する。

- component責務、module boundary、実行モデルを変更する
- securityまたはtrust boundaryを決める
- 永続化schema、migration、後方互換性の方針を決める
- core dependency、runtime、外部backendを採用または置換する
- CLIやconfigurationに意図的な後方非互換変更を加える
- 複数の有力案から一つを選び、将来同じ議論が再発し得る
- 撤回や変更に大きなmigration costを伴う

次の変更には通常ADRを作成しない。

- 既存のAccepted ADRと`SPEC.md`に沿った通常実装
- 局所的なbug fixや小規模refactor
- typo、link修正、機械的formatting
- issueのscopeと検証記録だけで理由を十分説明できる変更

判断の影響範囲が不明な場合は、実装前にissueでADRの要否を明示してreviewする。

## Naming and Numbering

- [`_template.md`](_template.md)を基に作成する。
- file名は`NNNN-short-kebab-case-title.md`とする。番号は4桁の連番で、一度使用した番号を再利用または並べ替えない。
- 新しい番号を決める前に、このdirectory、main branch、並行中のissueを確認し、primary agentが予約の衝突を調整する。
- Titleは判断を能動形で表し、単なるtopic名にしない。
- Dateは`YYYY-MM-DD`形式で、判断を記録した日を使用する。
- issue、置換するADR、仕様などの参照にはrepository内の相対linkを使う。

## Status

- **Proposed**: 判断案をreview中で、まだ採用されていない。
- **Accepted**: reviewを経て採用され、現在有効である。
- **Rejected**: 検討したが採用されなかった。検討履歴として保持する。
- **Deprecated**: 置換先を必ずしも持たないが、現在は推奨または適用されない。
- **Superseded**: 後続ADRによって置き換えられた。

新しいADRは原則`Proposed`で作成し、primary agentのreviewと合意を経て`Accepted`または`Rejected`へ変更する。既に実装と文書から明確に確認できる過去の判断を遡及記録する場合は、根拠をReferencesへ示したうえで`Accepted`として追加できる。

## Lifecycle

1. 関連issueへ判断が必要な背景、scope、ADRの必要性を記録する。
2. Implementation subagentが次の番号を予約し、templateから`Proposed` ADRを作成してindexへ追加する。
3. 代替案、security／compatibilityへの影響、migrationの必要性を含めてreviewする。
4. 採用する場合は`Accepted`、採用しない場合は`Rejected`とし、issueへ結果を記録する。
5. Accepted decisionが現行仕様を変更する場合は、同じchangeで`SPEC.md`を更新する。roadmapへの影響は`PLAN.md`、利用者が知る必要がある変更はuser documentationへ反映する。
6. 実装と検証が必要な場合は関連issueのacceptance criteriaとResolutionへ記録する。

Accepted ADRはhistorical recordとして扱う。誤字や壊れたlinkは修正できるが、過去のContext、Decision、Alternatives、Consequencesを現在の判断に合わせて書き換えない。

## Superseding a Decision

既存判断を変更する場合は、古いADRを削除または全面改稿せず、新しい番号のADRを作成する。

- 新しいADRのmetadataへ`Supersedes`を記載し、DecisionとConsequencesで変更点を説明する。
- 古いADRのStatusを`Superseded`へ変更し、`Superseded by`から新しいADRへlinkする。
- 新旧両方のindex entryを保持し、Statusを同期する。
- `SPEC.md`を新しい現行仕様へ更新し、必要ならmigrationとrelease planをissue／`PLAN.md`へ記録する。

この方法により、現在の判断と過去にその判断が合理的だった理由の両方を追跡できる。

## Relationship to Other Documents

- **Issue**: 判断を必要とした具体的な作業、scope、受入条件、検証、Resolutionを記録する。
- **ADR**: 長期的な技術判断と採用理由を記録する。
- **SPEC.md**: ADRを反映した現在の正しいアプリケーション仕様を記録する。
- **PLAN.md**: 未実装の作業、release gate、優先順位、保留中の判断を記録する。
- **User documentation**: ユーザーが操作や運用のために知る必要がある結果を説明する。

ADRだけを読まなければ現在の挙動が分からない状態や、issue、ADR、`SPEC.md`へ同じ説明を複製する状態を作らない。それぞれの文書は担当する情報を記録し、相対linkで関係を結ぶ。
