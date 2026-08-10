# ADR-0001: システムOpenSSHへSSH処理を委譲する

- Status: Accepted
- Date: 2026-08-10
- Recorded retrospectively: Yes
- Related issues: [Issue 007](../../issues/007-organize-project-governance-documents.md)

このADRは、portdeckのMVP設計と実装にすでに採用されている判断を、後から参照できる形で記録する。

## Context

portdeckはSSH接続とport forwardingの生存管理をTUIから扱う必要がある。一方、SSH protocol、認証、暗号、host key検証、`ssh_config`の完全な解決は、長い運用実績とユーザーごとの設定を持つ領域である。

これらをRustで再実装すると、portdeckが秘密情報を扱う範囲とsecurity上の責任が広がる。また、ProxyJump、鍵のpassphrase、keyboard-interactive認証など、システムOpenSSHがすでに提供する挙動との互換性維持が必要になる。

## Decision

- SSH protocol、認証、暗号、host key検証、ProxyJump、TCP forwardingを、`PATH`から選択したシステムOpenSSH clientへ委譲する。
- effective SSH configurationは独自解釈せず、具体的なHost aliasに対する`ssh -G`で解決する。
- 接続とforwardingは`ssh -M`およびControlMasterの`-O`操作で制御する。
- 外部commandはshellを介さず、programと各argumentを分離して実行する。
- portdeckは認証情報や秘密鍵を取得、保存、log出力しない。
- host key検証を緩和するoptionをportdeckから自動指定しない。

## Alternatives considered

### RustのSSH libraryを利用する

SSH protocolと認証をapplication process内で扱う範囲が広がり、ユーザーのOpenSSH設定や認証方法との互換性を別途維持する必要があるため採用しない。

### SSH protocolまたはTCP proxyを独自実装する

portdeckの中心的価値ではない暗号・通信処理をsecurity boundaryへ取り込み、検証範囲を大きくするため採用しない。

### shell command文字列を組み立てる

Host aliasやforwarding入力にshellの解釈が加わり、argument境界を安全に保てないため採用しない。

## Consequences

- ユーザーは既存のOpenSSH設定、agent、認証方式、ProxyJumpを利用できる。
- portdeckのsecurity boundaryをprocess制御、状態管理、入力検証へ限定できる。
- 実行環境にOpenSSH clientが必要であり、そのversion差が互換性に影響する。
- 対話が必要な接続ではterminalをOpenSSHへ引き渡し、TUIをsuspend／resumeする必要がある。
- 最低対応OpenSSH versionと複数versionでの動作確認をrelease gateとして管理する。

## References

- [Application specification](../../SPEC.md)
- [OpenSSH adapter](../../src/ssh.rs)
- [SSH adapter tests](../../tests/ssh_adapter.rs)
- [OpenSSH integration tests](../../tests/openssh_integration.rs)
- [Security highlights](../../README.md#security-highlights)
