# axvern ドキュメント

Rust製CLI `axvern` の仕様書セット。**Pueueに実行・タスク管理を委譲し、axvernはポリシー制御、モダンCLI連携、保存ログの抽出とパス分析を提供する。**

| 文書 | 内容 | 主な参照先 |
| --- | --- | --- |
| [REQUIREMENTS.md](./REQUIREMENTS.md) | 目的、責務の境界、MVP/非対象、非機能要件 | P-xx / NFR-xx |
| [ARCHITECTURE.md](./ARCHITECTURE.md) | システム構成、実行フロー、ユースケース | UC-xx |
| [FUNCTIONAL_REQUIREMENTS.md](./FUNCTIONAL_REQUIREMENTS.md) | Pueue、Policy、Result Query、CLI委譲、`fs analyze` | FR-xx |
| [CLI_SPEC.md](./CLI_SPEC.md) | コマンド一覧、応答例、設定ファイル | CLI契約 |
| [VERIFICATION.md](./VERIFICATION.md) | 受入テスト、実装前Spike | AC-xx |
| [ROADMAP.md](./ROADMAP.md) | v0.1〜v0.2の実装順、依存、将来拡張 | M0〜M8 |
| [REFERENCES.md](./REFERENCES.md) | PueueやモダンCLIの参考URL | 外部資料 |

## 読む順番

初めて仕様を見る場合は **REQUIREMENTS → ARCHITECTURE → FUNCTIONAL_REQUIREMENTS → CLI_SPEC**。実装時は **ROADMAP → VERIFICATION** を併用する。

## 版管理方針

- 要件の正本は `REQUIREMENTS.md` と `FUNCTIONAL_REQUIREMENTS.md`。実装順や優先度は `ROADMAP.md`。
- 受入条件は `VERIFICATION.md` に一元化し、識別子（FR/AC）は変更時も追跡可能にする。
- 旧統合版の節番号（例:「4章」）は移行元の表現として一部残るため、分割後は文書名で参照する。
- v0.1: Pueueバックグラウンド実行、結果クエリ、ポリシー、基本ファイル操作。
- v0.2: `tokei + eza + fd` を組み合わせた `fs analyze` と拡張CLI群。
