# ADR-0003: 保存済みforward ruleと実行状態を分離する

- Status: Accepted
- Date: 2026-08-10
- Recorded retrospectively: Yes
- Related issues: [Issue 003](../../issues/003-edit-saved-forward-rules.md), [Issue 004](../../issues/004-select-local-or-socks-forward.md), [Issue 007](../../issues/007-organize-project-governance-documents.md)

このADRは、portdeckのMVP設計と実装にすでに採用されている判断を、後から参照できる形で記録する。

## Context

ユーザーが再利用するforward ruleは再起動後も残す必要があるが、ControlMasterとforward channelの状態は現在のOpenSSH processに属する。希望したlocal portが競合した場合には、実際にOpenSSHへ追加したportが保存済みの希望値と異なることもある。

実行時PIDや前回の`Active`表示を永続的な真実として保存すると、process終了や異常終了の後に実態と表示が食い違う。取消には、OpenSSHへ追加したforwardの種別、実際のlocal port、正規化済み指定を正確に再利用する必要がある。

## Decision

- `ForwardRule`をtargetに属する永続的な定義、`ActiveForward`を現在のsessionに属する実行状態として別のdomain typeで扱う。
- 設定fileにはlabel、bind address、希望local port、forward種別、および種別固有の宛先だけを保存する。
- session state、forward state、PID、actual local port、正規化済み実行指定、OpenSSH errorは永続化しない。
- `requested_local_port`と`actual_local_port`を分け、競合時に希望値を失わないようにする。
- forward追加成功後に、実際の種別、local port、正規化済み`-L`／`-D`指定をruntime stateへ保持し、取消時に同じ指定を使う。
- 起動時は保存済みruleをInactiveとして読み込み、forwardやsessionを暗黙に再開しない。
- 設定更新はschemaを検証し、同じfilesystem内の一時fileからrenameするatomic writeを使用する。

## Alternatives considered

### 実行状態とPIDを設定fileへ保存する

OpenSSHの実態と独立してstaleになり、PIDの再利用もあるため、接続やforwardの真実として使用できない。

### 保存済みruleとactive forwardを1つの状態へ統合する

希望値と実際値、定義の削除とOpenSSH上の取消失敗を区別できず、失敗時にUIだけ成功したように見えるため採用しない。

### 保存したruleを起動時に自動有効化する

認証prompt、port競合、意図しないlistener公開を起動時に発生させるため、MVPの既定動作にはしない。

## Consequences

- 再起動後もruleを確認・編集・再利用できる一方、実行状態は必ず現在のsessionから始まる。
- port競合時にもユーザーの希望portとOpenSSHが使用したportを両方表現できる。
- cancel失敗時に定義とruntime stateを保持し、成功したように見せず再操作できる。
- 将来自動有効化を追加する場合も、保存済み定義に明示的なopt-inを追加する必要がある。
- schema変更では旧設定のmigrationとround-trip testが必要になる。

## References

- [Application specification](../../SPEC.md)
- [Domain model](../../src/domain.rs)
- [Application state](../../src/application.rs)
- [Rule store](../../src/config/store.rs)
- [Runtime and configuration guide](../en/runtime-and-configuration.md)
