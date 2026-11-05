# #84 Phase 1 — 期間内IssueとPR・受入証拠

2026-11-05 JSTにGitHub Project axvern全68項目を取得し、Start date <= 当日 <= Target dateの9件を抽出。
Project期間を正本とし、古いIssue本文の11/13予定と混同しない。
8実装Issueはdevelop、Phase #84はplanでinspect/select/checkを通過した。
期間前のIssueは着手せず、#67/#68等の期間外の関連作業は既存成果と残条件を参照した。

| Issue | 要件 | 個別PR | 受入証拠・今回の対応 |
| --- | --- | --- | --- |
| #107 共通 | #34 | [#116](https://github.com/UtakataKyosui/axvern/pull/116) | [共通](107-common-contract.md)、resultの照会エラー終了値修正 |
| #112 result | #65 | [#117](https://github.com/UtakataKyosui/axvern/pull/117) | [result](112-result.md)、structured text fragment・巨大summary/非text |
| #88 run | #35 | [#118](https://github.com/UtakataKyosui/axvern/pull/118) | [run](88-run.md)、既存実装と各ACの対応 |
| #89 status | #36 | [#119](https://github.com/UtakataKyosui/axvern/pull/119) | [status](89-status.md)、状態/null/未知variant fixture強化 |
| #90 wait | #37 | [#120](https://github.com/UtakataKyosui/axvern/pull/120) | [wait](90-wait.md)、SIGINT後も対象継続・再登録0件 |
| #91 history | #38 | [#121](https://github.com/UtakataKyosui/axvern/pull/121) | [history](91-history.md)、rewrite argv非掲載・offline・同時刻/limit境界 |
| #92 policy check | #39 | [#122](https://github.com/UtakataKyosui/axvern/pull/122) | [policy](92-policy-check.md)、副作用なし・両plan評価の証拠 |
| #93 doctor | #40 | [#123](https://github.com/UtakataKyosui/axvern/pull/123) | [doctor](93-doctor.md)、scope/capability/scratch/deadlineの証拠 |
| #84 Phase | #34〜#40/#65 | 本文書のPR | 子Issue・共通・resultの証拠と残条件を集約 |

PRは表の順に積み、各差分をIssueごとにレビュー可能とした。最初のbaseはmain。
要件Issueのcloseを実装受入と読み替えず、Project/Issueの状態変更は今回行っていない。

## 実行証拠

ローカルmacOS arm64 / rustc 1.99.1 / Pueue 4.0.4。
result修正後のintegration202件、core6件成功。その後、status fixture強化、SIGINTテスト追加（Pueue live7件成功）、history修正（CLI11件成功）、policy（CLI11+rewrite10）、doctor（CLI11+contract12）をそれぞれ検証した。
check/clippy/fmt、文書リンク42件、diff check成功。任意toolの全内部条件分岐の実証とはしない。

- [共通Quality](https://github.com/UtakataKyosui/axvern/actions/runs/42026362685): `aa08e1b6d13d5b5715ed92b1ac280ca3aaa6740d`、Ubuntu/macOS成功。
- [result Quality](https://github.com/UtakataKyosui/axvern/actions/runs/42028711033): `d44e70017c54b7343dff636992c56b8dd3e957f9`、Ubuntu/macOS・site成功。
- [wait Quality](https://github.com/UtakataKyosui/axvern/actions/runs/42033118131): `4534f11640b2b12c0169943c6894f45b39cab3ae`、Ubuntu/macOS成功。

docsだけのPRでRust jobがskipされても、別revisionの実測成功に置き換えない。各PRの最新Qualityとレビューをマージ時に確認する。

## 残る受入・完了条件

- PR/子Issueはレビュー前。Entire Gatesはレビュー承認不在でFail（#116/#117でChecks/Findings/Up to date通過、Approvalsのみ未通過を確認）。自己承認やgate無効化を行わない。
- #68の強制終了runnerに対する安全な回復入口は未提供。予約保護・再登録なしと回復完了を区別する。
- #98の未集約子成果物保護はPhase 2の受入であり、Phase 1 run結果保存の受入で代用しない。
- 新revisionのCI未完了やレビュー待ちを残したまま、子Issue/Phaseをcloseしない。今回のPR作成とPhase全体のDoneを区別する。
- 公開仕様は `sites/axvern-document-site/docs/en/cli.md`。将来性能・編集機能を公開済みとは記載しない。リリース・配布・デプロイは実施しない。

Target dateは11/06。上記受入が期間を超える場合は、Projectの期間とBlockerを再計画する必要がある。
