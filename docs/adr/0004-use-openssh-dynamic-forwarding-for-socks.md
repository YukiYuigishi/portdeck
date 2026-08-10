# ADR-0004: SOCKS forwardingをOpenSSH dynamic forwardingへ委譲する

- Status: Accepted
- Date: 2026-08-10
- Recorded retrospectively: Yes
- Related issues: [Issue 004](../../issues/004-select-local-or-socks-forward.md), [Issue 007](../../issues/007-organize-project-governance-documents.md)

このADRは、Issue 004でMVP後に追加され、すでに実装・検証されている判断を、後から参照できる形で記録する。

## Context

Local forwardはremote側から到達する固定の`host:port`をruleごとに指定する。利用要求として、clientが接続先を選べるSOCKS listenerも同じTUIとControlMasterで管理したいという要望が追加された。

OpenSSHは`ssh -D [bind_address:]port`でSOCKS4／SOCKS5 listenerを提供し、既存ControlMasterへの`-O forward`と`-O cancel`にもdynamic forwardingを指定できる。portdeck内にSOCKS serverを実装すると、独自proxyを持たないというsecurity boundaryから外れる。

## Decision

- SOCKS forwardingはシステムOpenSSHのdynamic forwarding (`ssh -D`)へ委譲する。
- domainではLocalとSOCKSを文字列や空のremote hostで表さず、`ForwardKind`の明示的なvariantとして扱う。
- SOCKS ruleはlabel、local bind address、希望local portを持ち、remote destinationを持たない。
- 既定listenerは`127.0.0.1:1080`とする。
- 追加は`ssh -O forward -D <bind>:<actual-port>`、取消は同じ正規化済み指定による`ssh -O cancel -D`で行う。
- port競合時の上限付き候補選択、OpenSSH成功後だけ`Active`にする規則、親session切断時の状態更新をLocalと共有する。
- 外部公開bindには、認証のないSOCKS proxyを公開することを明示する警告と確認を表示する。
- SOCKS clientの宛先や通信内容をportdeckで解析、保存、表示しない。

## Alternatives considered

### RustでSOCKS serverまたはTCP proxyを実装する

通信protocol、接続先処理、追加のsecurity責任をportdeckへ取り込み、OpenSSHへ委譲する基本方針に反するため採用しない。

### 固定のLocal forwardで代用する

SOCKS clientが接続ごとに任意の宛先を選ぶ能力を表現できないため代替にならない。

### SOCKS forwardingを対象外のままにする

具体的な利用要求があり、OpenSSHの既存能力と専用ControlMasterで安全に提供・検証できるため採用しない。

## Consequences

- LocalとSOCKSを同じsession lifecycle、状態表示、保存・取消modelで管理できる。
- SOCKS protocolと宛先へのTCP接続はOpenSSHが担い、ProxyJump設定もOpenSSHの解決結果を利用する。
- portdeckはSOCKS listenerに独自認証を追加しないため、loopbackを既定とし、外部公開時のriskをUIで伝える必要がある。
- destination単位の状態や通信logは表示できず、表示すべきでもない。
- kindを持たない既存schema version 1のruleはLocalとして読み込み、後方互換性を保つ必要がある。

## References

- [Issue 004: Select local or SOCKS forwarding](../../issues/004-select-local-or-socks-forward.md)
- [Application specification](../../SPEC.md)
- [Forward domain types](../../src/domain.rs)
- [OpenSSH dynamic forward adapter](../../src/ssh.rs)
- [Direct and ProxyJump SOCKS integration tests](../../tests/openssh_integration.rs)
