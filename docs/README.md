# axvern ドキュメント

Rust製開発最適化ハーネス `axvern` の仕様書セット。**実行（v0.1）→調査（v0.2）→結果活用（v0.3）→計測・比較・改善（v0.4+）** の順で段階的に構築する。

| 文書 | 内容 | 主な参照先 |
| --- | --- | --- |
| [REQUIREMENTS.md](./REQUIREMENTS.md) | 目的、責務の境界、MVP/非対象、非機能要件 | P-xx / NFR-xx |
| [ARCHITECTURE.md](./ARCHITECTURE.md) | システム構成、実行フロー、ユースケース | UC-xx |
| [FUNCTIONAL_REQUIREMENTS.md](./FUNCTIONAL_REQUIREMENTS.md) | Pueue、Policy、Result Query、CLI委譲、`fs analyze` | FR-xx |
| [CLI_SPEC.md](./CLI_SPEC.md) | コマンド一覧、応答例、設定ファイル | CLI契約 |
| [VERIFICATION.md](./VERIFICATION.md) | 受入テスト、実装前Spike | AC-xx |
| [ROADMAP.md](./ROADMAP.md) | v0.1〜v0.4+の実装順、依存、完了ゲート | Phase 1〜4 |
| [COMMAND_COMPOSITION.md](./COMMAND_COMPOSITION.md) | サブコマンドと既存CLIの合成方法 | v0.2の実装方式 |
| [COMMAND_FLOWS.md](./COMMAND_FLOWS.md) | 共通実行パイプラインのMermaid設計 | 処理境界 |
| [REFERENCES.md](./REFERENCES.md) | PueueやモダンCLIの参考URL | 外部資料 |

## 読む順番

初めて仕様を見る場合は **REQUIREMENTS → ARCHITECTURE → FUNCTIONAL_REQUIREMENTS → CLI_SPEC**。実装時は **ROADMAP → VERIFICATION** を併用する。

## 版管理方針

- 要件の正本は `REQUIREMENTS.md` と `FUNCTIONAL_REQUIREMENTS.md`。実装順や優先度は `ROADMAP.md`。
- 受入条件は `VERIFICATION.md` に一元化し、識別子（FR/AC）は変更時も追跡可能にする。
- 旧統合版の節番号（例:「4章」）は移行元の表現として一部残るため、分割後は文書名で参照する。
- v0.1: Pueueバックグラウンド実行、状態管理、基本ポリシー／診断。
- v0.2: モダンCLIによるコードベース調査と `fs analyze`。
- v0.3: ログ一時保存、`query`／`recall`。
- v0.4+: ベンチマーク、比較、ボトルネック調査。
