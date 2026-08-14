# Support installation from the public Git repository

- Status: Resolved
- Priority: High
- Reported date: 2026-08-14
- Resolved date: 2026-08-14
- Component: `installation`, `documentation`, `ci`, `packaging`

## Summary

利用者がrepositoryを手動でcloneせずに、公開GitHub repositoryから次のcommandで
portdeckをインストールできる手順を正式に整備する。

```console
cargo install --git https://github.com/YukiYuigishi/portdeck.git --locked
```

日英の利用者向け文書へこの手順を記載し、repository checkoutからの
`cargo install --path . --locked`もcontributorおよびsource checkout向けの選択肢として
維持する。Git sourceからのinstallabilityをCIで検証し、install方法の退行を検出する。

## Background

現在の[English README](../README.md#install-and-run)と
[日本語README](../README.ja.md#インストール)は`cargo install --path .`だけを案内している。
このcommandは利用者が先にrepositoryをcloneし、そのcheckoutへ移動していることを
前提とするため、公開repositoryのURLだけで完結する導入手順になっていない。

CargoはGit repositoryを直接sourceとしてbinary crateをinstallできる。`--locked`を
付けてcommittedな`Cargo.lock`を使用すれば、repositoryで検証したdependency解決と
利用者のinstall時の解決を揃えられる。一方、文書へcommandを追加するだけでは、将来の
manifest、lockfile、binary target、package構成の変更でinstallできなくなる退行を
検出できないため、隔離したCI smoke testも必要である。

これは配布経路と利用者向け導入手順の整備であり、portdeckのruntime behavior、
supported platform、OpenSSHとのsecurity boundaryを変更するものではない。そのため
`SPEC.md`と`PLAN.md`は、この調査で現行仕様またはroadmapの変更が必要と判明しない限り
更新しない。長期的なarchitecture判断でもないためADRは作成しない。

## Scope

- 公開repositoryのdefault branchから
  `cargo install --git https://github.com/YukiYuigishi/portdeck.git --locked`で
  `portdeck` binaryをインストールできる状態を確立する。
- `README.md`と`README.ja.md`でGit URLからのinstallを利用者向けの標準手順として
  案内する。
- repository checkoutを既に持つcontributorやsource build利用者向けに、
  `cargo install --path . --locked`をローカルinstallの選択肢として残す。
- Git source distributionに必要なpackage identityとprovenanceを
  `Cargo.toml`で監査し、このrepositoryで事実確認でき、installabilityに必要なmetadata
  だけを追加する。存在しないlicenseを推測または宣言しない。
- CIで、checkoutされたrevisionをGit sourceとして隔離したtemporary Cargo homeと
  install rootへ`--locked`でinstallするsmoke testを追加する。公開network上のdefault
  branchを再取得せず、PR／pushで検証対象となっているcommit自体をinstallする。
- CIでinstalled binaryの`--version`と`--diagnose`を実行する。temporary `HOME`、
  XDG directory、runtime directoryを使用し、利用者のSSH config、`known_hosts`、秘密鍵、
  既存ControlMasterへ依存または接触しない。
- installまたはdiagnostic commandが失敗した場合、CI stepを失敗させて終了statusと
  必要なdiagnostic outputを確認できるようにする。ただしsecretや環境変数全体は
  出力しない。

## Non-goals

- crates.ioへpublishすること、またはそのためのrelease processを決めること
- GitHub Releases、prebuilt binary、installer、Homebrew formulaを追加すること
- projectのlicenseを選択、推測、または新規に宣言すること
- applicationのCLI、TUI、configuration、persistence、OpenSSH実行挙動を変更すること
- supported OSや最低Rust／OpenSSH versionを変更すること
- `SPEC.md`や`PLAN.md`へ個別install手順を複製すること
- CIで利用者の`~/.ssh`、既存のSSH agent、key、ControlMasterをtest fixtureとして使うこと
- release tagやrevision pinning policyをこのissueで新設すること

## Acceptance criteria

- [x] 公開repositoryから
  `cargo install --git https://github.com/YukiYuigishi/portdeck.git --locked`で
  portdeckをclone操作なしにinstallできる。
- [x] `README.md`と`README.ja.md`が上記Git install commandを明示し、install後の
  `portdeck --diagnose`と起動手順へつながっている。
- [x] 両READMEが既存checkout向けの`cargo install --path . --locked`も明確に区別して
  案内している。
- [x] Git source installに必要なmanifestとlockfileが揃い、package metadataの変更は
  repositoryから確認できる事実だけで構成され、licenseを新規に宣言していない。
- [x] CIが検証対象revisionをGit sourceとしてisolated temporary install rootへ
  `--locked`でinstallし、installされたbinaryから`--version`と`--diagnose`を実行する。
- [x] CI smoke testはtemporary `HOME`とXDG/runtime pathsを使用し、利用者のSSH asset、
  secret、既存ControlMasterへ依存または接触しない。
- [x] install failure、lockfile不整合、binaryの実行失敗がCI failureとして検出される。
- [x] application behavior、OpenSSH delegation、supported environment、SPEC、PLAN、ADRに
  不要な変更がない。
- [x] required lint、test、Markdown link検査、`git diff --check`が成功する。

## Verification plan

1. `Cargo.toml`、`Cargo.lock`、binary targetを監査し、Git source installに必要なfileと
   package metadataがcommittedされていることを確認する。`cargo metadata`も使用して
   package identityとbinary targetを確認する。
2. cleanなtemporary directoryを作成し、checkoutされたrevisionのlocal `file://` Git
   URLをsourceとして、isolated `CARGO_HOME`と`--root`を指定した
   `cargo install --git <local-git-url> --locked`を実行する。これにより検証対象commitを
   変えずに公開URLのGit installと同じCargo経路を検査する。
3. temporary `HOME`、`XDG_CONFIG_HOME`、`XDG_STATE_HOME`、`XDG_RUNTIME_DIR`を設定し、
   install root内の`portdeck --version`と`portdeck --diagnose`が成功することを確認する。
   実行前後にfixture外のSSH assetやControlMasterを作成・変更していないことを確認する。
4. `README.md`と`README.ja.md`のcommand、URL、`--locked`、Git installとlocal path
   installの説明が対応していることをreviewする。
5. repository内の相対Markdown linkを検査し、`git diff --check`を実行する。
6. `./scripts/lint.sh`と`cargo test --all-targets --all-features`を実行する。
   OpenSSH実行挙動は変更しないため、real OpenSSH integration testは対象外とする。
7. 実装後にissueのacceptance criteria、実行したcommandと結果、変更したpackage
   metadata、関連commitをResolutionへ記録する。

## Resolution

2026-08-14にGit sourceからのinstall手順と継続検証を整備した。

### Changes

- `README.md`と`README.ja.md`で
  `cargo install --git https://github.com/YukiYuigishi/portdeck.git --locked`を
  clone-freeな標準手順とし、既存checkout向けの
  `cargo install --path . --locked`を別の選択肢として維持した。
- `scripts/install-smoke.sh`を追加し、検証対象commitをlocal `file://` Git sourceから
  temporary `CARGO_HOME`とinstall rootへinstallするようにした。installed binaryの
  `--version`と`--diagnose`はtemporary `HOME`／XDG pathsと、`-V`だけを受け付ける
  fake `ssh`で実行するため、利用者のSSH assetや既存ControlMasterへ接触しない。
- GitHub Actionsのquality jobへsmoke scriptを追加し、install、lockfile、binary実行の
  退行をpushとpull requestで検出するようにした。
- `cargo metadata --no-deps --format-version 1`でmanifestを監査した。既存のpackage
  name `portdeck`、version `0.1.0`、`src/main.rs`のbinary target、committedな
  `Cargo.lock`でGit installに必要な情報は揃っていたため、`Cargo.toml`は変更せず、
  licenseも新規に宣言しなかった。

### Verification results

- `./scripts/lint.sh`: 成功。
- `cargo test --all-targets --all-features`: 成功。unit 109件、CLI 4件、DEBUG logging
  4件、SSH adapter 7件が成功し、real OpenSSH integration 4件は既定どおりignoredだった。
- relative Markdown link検査: 成功。ADR templateの意図的なplaceholderは除外した。
- `git diff --check`、`sh -n scripts/install-smoke.sh`、
  `cargo metadata --no-deps --format-version 1`: 成功。
- localのnetworkを使う空`CARGO_HOME` smoke初回は、crates.ioの`de/ra/deranged`取得が
  curl code 28のlow-speed timeoutで失敗した。retryはcrates.io index更新で約6分間
  進まなかったため中止した。registry cacheをseedした別のonline試行も
  `co/ns/const-oid`取得で同じcurl code 28となった。これらをinstall成功とは扱っていない。
- networkに依存しない代替検証では、`cargo vendor --locked --offline`で既存cacheから
  temporary vendor treeを作り、空のtemporary `CARGO_HOME`からlocal `file://` Git
  sourceのcommit `910de24`を`--rev`、`--locked`、command-line source replacement付きで
  installした。release buildとinstallに成功し、installed binaryは
  `portdeck 0.1.0`と`OpenSSH_9.6p1 vendored-install-smoke`を出力した。repository fileは
  変更せず、temporary resourceは終了時に削除した。
- authoritativeな[GitHub Actions run 31767195589](https://github.com/YukiYuigishi/portdeck/actions/runs/31767195589)では、
  `Run linters`、`Run tests`、`Smoke test Git installation`を含むjob全体が成功した。
  clean-network runner上でcommitted scriptそのものによるisolated Git installとinstalled
  binaryの実行が成功したため、local network failureから残っていたriskを解消した。
- application behaviorとOpenSSH argvを変更しておらず、smoke testもfake `ssh -V`だけを
  実行するため、real OpenSSH integration testは明示実行しなかった。

### Related commits

- `fe579f4 docs(issue): plan Git repository installation`
- `910de24 ci: verify installation from Git source`
