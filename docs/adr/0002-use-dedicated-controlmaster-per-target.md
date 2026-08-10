# ADR-0002: targetごとに専用ControlMasterを使用する

- Status: Accepted
- Date: 2026-08-10
- Recorded retrospectively: Yes
- Related issues: [Issue 001](../../issues/001-tui-does-not-resume-after-connect.md), [Issue 007](../../issues/007-organize-project-governance-documents.md)

このADRは、portdeckのMVP設計と実装にすでに採用されている判断を、後から参照できる形で記録する。

## Context

複数のforwardを同じSSH接続へ動的に追加・取消し、接続状態とforwardの所有関係をTUIへ表示する必要がある。portdeckが作成していないControlMasterを採用すると、そのlifecycleや既存forwardを安全に管理できない。一方、forwardごとに個別のSSH processを起動すると、接続とforwardの状態を分離して扱えない。

Unix domain socketのControlPathには長さ制限があり、Host aliasをそのままpathへ含めることにも情報露出と不正なpath要素の問題がある。異常終了後には、生存するmasterと単なるstale socketを区別する必要もある。

## Decision

- concrete targetごとに、portdeckが所有する専用OpenSSH ControlMasterを1つ開始する。
- 接続時は`ClearAllForwardings=yes`を指定し、ユーザー設定由来のforwardを専用masterへ暗黙に混在させない。
- forward追加・取消とsession終了は、専用ControlPathに対する`ssh -O forward`、`-O cancel`、`-O exit`で行う。
- sessionの生存確認にはPIDではなく`ssh -O check`を使用する。
- ControlPathは所有者限定のapplication runtime directoryに置き、target IDの固定長hashから短い名前を生成する。
- 起動時はportdeckのnamespaceに属するsocketだけを調べ、生存masterは終了させ、staleと確認したentryだけを削除する。
- 正常終了時はportdeckが所有するすべてのControlMasterを明示的に終了する。

## Alternatives considered

### ユーザーまたは他toolのControlMasterを共有する

portdeckが所有していない接続やforwardを誤って変更・終了する可能性があり、状態の真実を管理できないため採用しない。

### forwardごとに独立したSSH processを起動する

認証済み接続を再利用できず、sessionと複数forwardのlifecycleを一貫して表示・操作しにくいため採用しない。

### PIDまたはsocketの存在だけで接続状態を判断する

processの再利用やstale socketを生存中のControlMasterと区別できないため採用しない。

## Consequences

- target sessionとその配下のforwardを明確に対応付けられる。
- 1つの認証済み接続へLocal／SOCKS forwardを動的に追加・取消できる。
- portdeckは他toolのmasterを採用・終了しない。
- ControlPath directoryの所有権、mode、path長、異常終了後の回収を管理する必要がある。
- TUI終了後も接続を残すdetachは、このlifecycle方針を明示的に拡張しない限り提供しない。

## References

- [Application specification](../../SPEC.md)
- [Runtime directory implementation](../../src/runtime.rs)
- [Session manager](../../src/application.rs)
- [OpenSSH integration tests](../../tests/openssh_integration.rs)
- [Runtime and configuration guide](../en/runtime-and-configuration.md)
