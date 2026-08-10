# Organize project governance documents

- Status: In Progress
- Priority: Medium
- Reported: 2026-08-10
- Component: `documentation`, `project-governance`

## Summary

プロジェクトの仕様、計画、開発手法をそれぞれ適切な文書へ分離し、エージェントが一貫したissue-driven workflowで開発できるようにする。

`AGENTS.md`は「どのように開発するか」、新設する`SPEC.md`は「何を作るか」、`PLAN.md`は「現在地と次に何をするか」を扱う。個別作業は`issues/`、重要な設計判断とその理由は`docs/adr/`へ記録する。

## Background

現在の`AGENTS.md`は、エージェント向けの開発規約だけでなく、製品目標、機能要件、状態モデル、永続化、セキュリティ要件、受入条件まで抱えている。そのため、作業時に常に必要な規約と、アプリケーション仕様の境界が分かりにくい。

`PLAN.md`には完了済みの実装履歴と未完了のrelease gate、Post-MVP候補が混在している。現在地や次に着手すべき項目を判断するために、過去の長いchecklistを読み解く必要があるほか、完了済みのProxyJump検証など一部の状態が実績と一致していない。

また、これまで採用したissue、subagent、worktree、細かなcommitによる開発方法や、設計判断をADRへ残す基準がプロジェクトの恒久的な規約として整理されていない。

## Documentation responsibilities

- `AGENTS.md`: primary agentとimplementation subagentの役割、issue-driven development、worktreeとbranch、commitとmerge、レビュー、検証、cleanup、ADR運用など、開発の進め方を定める。
- `SPEC.md`: 製品目標、用語、現在の機能要件、非目標、対応環境、architecture、domain/state model、永続化、security、failure handling、testing requirements、acceptance criteriaを記録する。
- `PLAN.md`: current status、直近releaseのgate、次の優先作業、Post-MVP backlog、保留中の判断だけを記録する。
- `issues/`: 不具合や機能、文書整備などの個別作業について、背景、scope、非目標、受入条件、検証結果を記録する。
- `docs/adr/`: 後から理由を確認する価値がある重要な技術判断について、context、decision、alternatives、consequencesを記録する。
- `README.md`、`README.ja.md`、`docs/en/`、`docs/ja/`: ユーザー向け文書として維持し、この作業では再設計しない。
- `CONTRIBUTING.md`: 人間のcontributor向けの具体的な開発・検証手順として維持する。

文書間で同じ情報を複製しない。ADRが仕様を変更する場合は`SPEC.md`も更新し、現行仕様の確認に過去のADRをすべて読む必要がある状態を作らない。個別作業の詳細はissueへ置き、`PLAN.md`からは必要に応じてissueを参照する。

## Scope

### Application specification

- `SPEC.md`を新設し、現在`AGENTS.md`に含まれるアプリケーション要件を意味を変えずに移す。
- 実装済みのLocal／SOCKS forward、target検索、forward編集、Vim風pane移動、file-backed DEBUG mode、ProxyJump対応を含む現在の挙動と整合させる。
- 仕様、Post-MVP候補、Non-Goalsの矛盾や明らかな古い記述を整理する。

### Agent development rules

- `AGENTS.md`を、プロジェクト概要と開発手法・基本方針を短時間で確認できる文書へ再構成する。
- primary agentは調査、分解、割当、レビュー、merge、最終検証、cleanupを担当し、原則として実装ファイルを直接編集しないことを明記する。
- implementation subagentは割り当てられた専用worktreeで実装、テスト、文書、issue更新を完了し、mainへ直接mergeしないことを明記する。
- 不具合と機能は実装前に`issues/`へ起票し、重複確認、scope、acceptance criteria、verification、完了時のResolutionを記録するworkflowを定める。
- 軽微な誤字修正など、issueを作る価値が低い変更の例外を定める。
- issueごとのbranchとworktree、1 worktreeを複数subagentで共有しないこと、論理単位のcommit、`--no-gpg-sign`の許可、レビュー後の`--no-ff` merge、merge後のcleanupを定める。
- 既存のOpenSSH委譲、security、module boundary、テストに関する重要なengineering ruleを保持する。

### Planning

- `PLAN.md`をcurrent status、release gates、next work、Post-MVP backlog、deferred decisions中心へ整理する。
- 完了済みphaseの詳細はGit履歴とresolved issueへ委ね、現在の判断に必要な要約と参照だけを残す。
- ProxyJumpなど、実装・検証済みなのに未完了表示となっている項目を実績に合わせる。
- 未完了のOpenSSH互換性、認証、ホスト鍵、forward拒否などをrelease gateとして明確にする。

### Architecture decision records

- `docs/adr/README.md`へ、ADRを作成する基準、番号、status、更新・supersede方法、issue／SPECとの関係を記載する。
- 再利用できるADR templateを用意する。
- 現在のarchitectureを理解するうえで重要な既存判断を、少数のaccepted ADRとして記録する。少なくとも、システムOpenSSHへの委譲、targetごとの専用ControlMaster、定義状態だけを永続化する方針を対象候補とする。
- 過去の判断を推測で記録せず、既存仕様、実装、issue、Git履歴から根拠を確認する。

## Non-goals

- アプリケーションの実行時動作、CLI、TUI、設定schemaを変更すること
- `README.md`、`README.ja.md`、`docs/en/`、`docs/ja/`の構成を再設計すること
- release gateとして残る互換性・認証・ホスト鍵テストをこの作業で実施すること
- Remote Port Discovery、Protocol Hint、自動有効化、Mosh、remote forwardingなど未実装機能へ着手すること
- 完了済みissueの履歴を書き換えること
- すべての局所的な実装判断をADRへ変換すること

## Verification plan

- `AGENTS.md`、`SPEC.md`、`PLAN.md`、`issues/`、`docs/adr/`の責務が重複せず、相互参照が有効であることをレビューする。
- 移動前後で、現行の機能要件、Non-Goals、security rule、testing requirement、未完了release gateが失われていないことを差分で確認する。
- `SPEC.md`が実装済みのLocal／SOCKS forwardと現在のTUI操作を反映していることを、コードおよびユーザー文書と照合する。
- `PLAN.md`の完了・未完了状態をresolved issueと既存の検証記録に照らして確認する。
- ADRのindex、template、accepted record間のlinkと番号が整合することを確認する。
- repository内の相対Markdown linkを検査する。
- `git diff --check`を実行する。
- 文書だけの変更であることを確認し、通常のlintとtestが不要か、実行した場合はその結果をResolutionへ記録する。

## Acceptance criteria

- [ ] `SPEC.md`が新設され、現在のアプリケーション仕様が`AGENTS.md`から移されている。
- [ ] `AGENTS.md`が開発手法、基本方針、primary agent／implementation subagentの役割を中心とする文書になっている。
- [ ] issue起票、専用worktree、commit、review、merge、verification、cleanupまでのworkflowが`AGENTS.md`に明記されている。
- [ ] `PLAN.md`がcurrent status、release gates、next work、Post-MVP backlog、deferred decisionsを中心に整理されている。
- [ ] 完了済みProxyJump検証など、既知の実績が`PLAN.md`へ正しく反映されている。
- [ ] `docs/adr/`に運用方針、template、主要な既存判断のaccepted ADRがある。
- [ ] ADRを作成する判断基準と、supersede時に過去の記録を保持する方法が明記されている。
- [ ] 仕様、計画、issue、ADR、ユーザー文書の責務と参照関係に不必要な重複がない。
- [ ] アプリケーションの実行時動作や未実装機能に変更がない。
- [ ] 相対Markdown linkと`git diff --check`の検査が成功する。
