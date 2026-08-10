# AGENTS.md

## Purpose

この文書は、portdeckを変更するエージェントの開発手法と恒久的なengineering guardrailを定める。アプリケーションの詳細仕様やroadmapをここへ複製しない。

portdeckは、システムのOpenSSHへSSH接続とport forwardingを委譲し、そのControlMaster sessionとforward ruleをTUIで管理するRust製ツールである。現在の挙動と対象範囲は`SPEC.md`を正とする。

## Sources of Truth

| Concern | Source of truth |
| --- | --- |
| 現在のアプリケーション仕様、用語、非目標 | [SPEC.md](SPEC.md) |
| 現在地、release gate、次の作業、backlog | [PLAN.md](PLAN.md) |
| 個別変更のscope、受入条件、検証、完了記録 | [issues/](issues/) |
| 長期的に重要な技術判断とその理由 | [docs/adr/](docs/adr/README.md) |
| 利用者向けの導入説明 | [README.md](README.md)、[README.ja.md](README.ja.md)、[docs/en/](docs/en/)、[docs/ja/](docs/ja/) |
| 人間のcontributor向けの環境構築と検証手順 | [CONTRIBUTING.md](CONTRIBUTING.md) |

情報は適切なsource of truthだけに記録し、複数文書へ詳細を複製しない。ADRが現行仕様を変える場合は`SPEC.md`も更新する。個別作業の詳細はissueへ置き、`PLAN.md`から必要に応じて参照する。

## Agent Roles

### Primary agent

Primary agent（root agent）は作業全体のorchestratorであり、次を担当する。

- read-onlyの調査、既存issueとの重複確認、タスク分解、依存関係の整理を行う。
- issueごとのbranchと専用worktreeを準備し、implementation subagentへ明確なscopeと受入条件を割り当てる。
- subagentのdiff、commit、検証結果をレビューする。
- 問題を発見した場合は自分で修正せず、原則としてその変更を担当したimplementation subagentへ戻す。
- 承認したbranchをmain branchへmergeし、統合後の最終検証とcleanupを行う。

Primary agentは原則としてrepository fileを直接編集しない。実装、テスト、文書、issue、ADRを含むすべてのmaterial changeはimplementation subagentへ委譲する。read-only commandとbranch、worktree、merge、cleanupなどのrepository administrationはprimary agentが行ってよい。

### Implementation subagent

Implementation subagentは、primary agentから割り当てられた変更を実行する担当者であり、次を守る。

- 割り当てられた専用worktree内で、実装、テスト、必要な文書、issueまたはADRの更新を完了する。
- 指定されたscopeと担当fileを越えて変更しない。追加変更が必要なら、編集前にprimary agentへ報告する。
- primary agentから明示的に指示されない限り、別のsubagentへ再委譲しない。この節の委譲規則はimplementation subagent自身に再帰的な委譲を要求しない。
- main branchへmergeしない。また、他担当者のworktreeやbranchを変更しない。
- 論理単位でcommitし、branchがcleanな状態で、commit一覧、検証結果、未解決事項をprimary agentへ報告する。

## Issue-driven Development

### When an issue is required

次のmaterial changeは、実装や編集を始める前に`issues/`へ起票する。

- featureまたはbehavior change
- bug fix
- 意味のあるrefactor、architectureまたはpersistenceの変更
- 複数文書に影響する、または運用方針を変えるdocumentation change
- release gateや検証作業のまとまり

明白なtypo、linkの単純修正、意味を変えないformattingのように履歴を独立して追う価値が低い変更はissueを省略できる。小さくても挙動、互換性、security、運用ルールへ影響する場合は省略しない。

起票前に既存のopen／resolved issueを検索し、重複するissueを作らない。既存issueのscopeへ含める場合は、そのissueを更新してから作業する。

### Issue contents

Issue fileは`issues/NNN-short-description.md`形式とし、少なくとも次を記録する。

- Status、Priority、Reported date、Component
- SummaryとBackground
- ScopeとNon-goals
- Acceptance criteria
- Verification plan
- Resolution（完了時の実績、検証結果、関連commit）

作業開始時にstatusを`In Progress`、受入条件を満たし統合可能になった時点で`Resolved`へ更新する。実装中にscopeや判断が変わった場合は、コードだけでなくissueも同期する。失敗した検証や残課題を成功したように記録しない。

## Branches, Worktrees, and Commits

- 原則として1 issueにつき1つのdescriptive branchと専用worktreeを用意する。
- branchは最新のcleanなmain branchから作成する。mainに既存の未commit変更がある場合、それを上書き、退避、破棄しない。
- 1つのworktreeを複数のimplementation subagentで共有しない。独立して進められるissueだけを並行化する。
- implementation subagentは割り当てられたworktreeでのみfileを変更する。
- commitは実装、テスト、文書、issue更新など、review可能な論理単位へ細かく分ける。
- commit messageは変更の目的が分かる形にする。commitには`--no-gpg-sign`を使用してよい。
- 関係のないuser changeや別issueの変更をcommitへ含めない。
- primary agentはレビューと必要な再検証の後、`--no-ff --no-gpg-sign`を使ってmainへmergeする。
- merge後はmainの統合状態を検証し、cleanであることを確認してから`git worktree remove`と安全なbranch削除を行う。未統合またはdirtyなworktreeを強制削除しない。

## Standard Workflow

1. Primary agentがsource of truth、現在のrepository state、既存issueをread-onlyで調査する。
2. Material changeならprimary agentが専用branch／worktreeを用意し、implementation subagentへ最初にissueの起票または更新を割り当てる。Subagentはproduct fileより先にissueへscope、non-goals、acceptance criteria、verification planを記録してcommitする。
3. Primary agentがissueをレビューし、implementation subagentへ確定した担当範囲を割り当てる。
4. Implementation subagentが変更、テスト、関連文書、issue／ADR更新を行い、論理単位でcommitする。
5. Primary agentがdiff、commit、受入条件、検証結果をレビューする。不備は担当subagentへ戻す。
6. Primary agentが承認済みbranchをno-ff mergeし、main上で必要な最終検証を行う。
7. Primary agentがissueのResolutionとrepositoryのclean状態を確認し、worktreeと統合済みbranchをcleanupする。

複数issueを並行実装する場合も、各issueのownershipとworktreeを分離する。後続branchが先行変更へ依存する場合は、primary agentがmerge順と再base／merge方針を明示し、implementation subagentが独断でmainや他branchを取り込まない。

## Architecture Decision Records

ADRの詳細な運用、index、templateは[docs/adr/README.md](docs/adr/README.md)を正とする。

次のように、後から採用理由を確認する価値がある長期的な判断にはADRを作成する。

- component責務やmodule boundaryの変更
- securityまたはtrust boundaryの変更
- persistence schemaやcompatibility policyの変更
- core dependency、runtime、外部backendの採用
- CLIやconfigurationに対する意図的な後方非互換変更
- 有力な代替案が複数あり、将来再検討され得る設計判断

局所的なbug fix、既存判断に沿った通常実装、typo、機械的refactorだけを理由にADRを作らない。

ADRもmaterial changeとしてissueとimplementation subagentのscopeに含める。新しい判断は次の番号で記録し、まず`Proposed`としてreviewする。採用後は`Accepted`とする。判断を置き換える場合は過去ADRの本文を現在の結論へ書き換えず、新しいADRを作成して相互参照し、古いADRを`Superseded`へ変更する。

ADRは「なぜ」を保持するが、現在のアプリケーション仕様の代替にはしない。Accepted ADRが挙動を変更する場合は同じchangeで`SPEC.md`を更新し、roadmapへ影響する場合は`PLAN.md`も更新する。

## Engineering Guardrails

### OpenSSH and security boundary

- SSH protocol、認証、暗号化、SSH設定解決、TCP forwardingはシステムのOpenSSHへ委譲する。合意された仕様変更とADRなしにSSH libraryや独自proxyへ置き換えない。
- OpenSSHの挙動を推測で実装しない。対象versionの`ssh(1)`／`ssh_config(5)`と隔離された実コマンドで確認する。
- 外部commandはargvを個別に渡して実行し、`/bin/sh -c`などのshell command constructionを導入しない。
- ユーザーの`~/.ssh/config`、`known_hosts`、秘密鍵、既存ControlMasterを変更またはtest fixtureとして利用しない。integration testは隔離された一時環境を使う。
- password、秘密鍵、passphrase、環境変数全体を保存またはlog出力しない。ホスト鍵検証や暗号設定を自動的に弱めない。

### Boundaries and failure handling

- domain state、OpenSSH adapter、configuration／runtime管理、TUI描画の責務を混在させない。TUIからOpenSSH argvを直接構築しない。
- 外部commandの失敗をpanicや曖昧な成功へ変換しない。終了statusとstderrを構造化し、実行状態とユーザー向けmessageへ反映する。
- 定義状態と実行状態を区別し、OpenSSHで成功していない操作をUI上だけ成功扱いにしない。
- 現在必要なboundaryは保つが、将来機能だけを目的とした抽象化、plugin機構、dependencyを先行して追加しない。
- repositoryがdirtyな場合、既存変更はユーザーまたは別担当者の所有物として扱い、無断で修正、format、revert、commitしない。

### Tests and documentation

- behavior changeには、変更したlayerに対応するtestを追加または更新する。
- fake executableを使うadapter testではargv、exit status、stdout、stderrとshell展開が発生しないことを検証する。
- OpenSSH lifecycleやforwardingを変える場合、利用可能な隔離環境で実OpenSSH integration testも実施する。
- TUI変更ではevent transition、主要rendering、小さいterminalでpanicしないことを検証する。
- behavior、CLI、key binding、configuration、supported environmentを変えた場合、同じchangeで`SPEC.md`と該当user documentationを更新する。
- failureを握りつぶしたtestや、ユーザーの実SSH資産に依存するtestを追加しない。

## Required Checks

変更範囲に応じて、少なくとも次を実行する。詳細は[CONTRIBUTING.md](CONTRIBUTING.md)を参照する。

```console
./scripts/lint.sh
cargo test --all-targets --all-features
```

OpenSSHの実行挙動を変えた場合は、対応環境で次も実行する。

```console
cargo test --test openssh_integration -- --ignored --test-threads=1 --nocapture
```

Documentation-only changeでも相対Markdown linkと`git diff --check`を検査する。検証を実行できない場合は、省略理由と残るriskをissueのResolutionとprimary agentへの報告に記載する。

## Definition of Done

変更は次をすべて満たしたときに完了とする。

- issueのscopeとacceptance criteriaを満たし、Non-goalsへ不要に踏み込んでいない。
- relevant test、lint、integration checkが成功し、実行できないcheckが明記されている。
- current behavior、roadmap、decision rationale、user guidanceがそれぞれ正しいsource of truthへ反映されている。
- errorとsecurity boundaryが維持され、user SSH assetsやsecretを変更・露出していない。
- commitが論理単位に分かれ、implementation branchとmainが所定の時点でcleanである。
- issueが`Resolved`となり、Resolutionに実績、検証結果、関連commitが記録されている。
- primary agentによるreview、no-ff merge、main上の最終検証、worktree／branch cleanupが完了している。
