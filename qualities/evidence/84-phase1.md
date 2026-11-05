# #84 Phase 1 — 期間内IssueとPR・受入証拠

2026-11-06 JSTにGitHub Project axvern全68項目を再取得し、Start date <= 当日 <= Target dateの9件を抽出。
Project期間を正本とし、古いIssue本文の11/13予定と混同しない。
初回対応では8実装Issueはdevelop、Phase #84はplanでinspect/select/checkを通過した。
今回のPhase集約も #84 / planでselect/checkを通過した。
期間前のIssueは着手せず、#67/#68等の期間外の関連作業は既存成果と残条件を参照した。

| Issue | 要件 | 個別PR | 受入証拠・今回の対応 |
| --- | --- | --- | --- |
| #107 共通 | #34 | [#116](https://github.com/UtakataKyosui/axvern/pull/116)（マージ済み）、[#126](https://github.com/UtakataKyosui/axvern/pull/126)（レビュー待ち） | [共通](107-common-contract.md)、照会終了値修正、humanの診断・可用性・ページ表示。IssueはIn Progress |
| #112 result | #65 | [#115](https://github.com/UtakataKyosui/axvern/pull/115)・[#117](https://github.com/UtakataKyosui/axvern/pull/117)（マージ済み）、[#125](https://github.com/UtakataKyosui/axvern/pull/125)（レビュー待ち） | [result](112-result.md)、公開API統一、structured fragment、完了証拠とleaseに基づく明示回復。IssueはIn Progress |
| #88 run | #35 | [#118](https://github.com/UtakataKyosui/axvern/pull/118) | [run](88-run.md)、既存実装と各ACの対応 |
| #89 status | #36 | [#119](https://github.com/UtakataKyosui/axvern/pull/119) | [status](89-status.md)、状態/null/未知variant fixture強化 |
| #90 wait | #37 | [#120](https://github.com/UtakataKyosui/axvern/pull/120) | [wait](90-wait.md)、SIGINT後も対象継続・再登録0件 |
| #91 history | #38 | [#121](https://github.com/UtakataKyosui/axvern/pull/121) | [history](91-history.md)、rewrite argv非掲載・offline・同時刻/limit境界 |
| #92 policy check | #39 | [#122](https://github.com/UtakataKyosui/axvern/pull/122) | [policy](92-policy-check.md)、副作用なし・両plan評価の証拠 |
| #93 doctor | #40 | [#123](https://github.com/UtakataKyosui/axvern/pull/123) | [doctor](93-doctor.md)、scope/capability/scratch/deadlineの証拠 |
| #84 Phase | #34〜#40/#65 | [#124](https://github.com/UtakataKyosui/axvern/pull/124)（マージ済み）、今回の更新PR | 期間内9Issue・全対応PR・最新CI・残条件を再集約。PhaseはIn Progress |

初回PR #116〜#124はマージ済み。#88〜#93はGitHub IssueのstateReason=COMPLETED、Project=Doneを再確認した。
追加PR #125・#126と今回の集約はそれぞれmainをbaseとし、他PRの未マージ変更を含めない。
期間内の未対応Issueはないが、対応PRの作成と受入完了は区別する。
要件Issueのcloseを実装受入と読み替えず、今回Project/Issueの状態変更は行っていない。

## 初回実行証拠（2026-11-05）

ローカルmacOS arm64 / rustc 1.99.1 / Pueue 4.0.4。
result修正後のintegration202件、core6件成功。その後、status fixture強化、SIGINTテスト追加（Pueue live7件成功）、history修正（CLI11件成功）、policy（CLI11+rewrite10）、doctor（CLI11+contract12）をそれぞれ検証した。
check/clippy/fmt、文書リンク42件、diff check成功。任意toolの全内部条件分岐の実証とはしない。

- [共通Quality](https://github.com/UtakataKyosui/axvern/actions/runs/42026362685): `aa08e1b6d13d5b5715ed92b1ac280ca3aaa6740d`、Ubuntu/macOS成功。
- [result Quality](https://github.com/UtakataKyosui/axvern/actions/runs/42028711033): `d44e70017c54b7343dff636992c56b8dd3e957f9`、Ubuntu/macOS・site成功。
- [wait Quality](https://github.com/UtakataKyosui/axvern/actions/runs/42033118131): `4534f11640b2b12c0169943c6894f45b39cab3ae`、Ubuntu/macOS成功。

docsだけのPRでRust jobがskipされても、別revisionの実測成功に置き換えない。各PRの最新Qualityとレビューをマージ時に確認する。

## 追加実装のCI証拠（2026-11-06）

| 対象 | head revision | Quality workflow | 確認結果 |
| --- | --- | --- | --- |
| #112 / PR #125 | `5ecab4a65426385c81a72eb481a728dfb6eceafd` | [42057864325](https://github.com/UtakataKyosui/axvern/actions/runs/42057864325) | Ubuntu/macOS Rust、fmt、site、links、quality成功 |
| #107 / PR #126 | `057e60fa5284c68e14450f40092f15076eb97b95` | [42059214553](https://github.com/UtakataKyosui/axvern/actions/runs/42059214553) | Ubuntu/macOS Rust、fmt、site、links、quality成功 |

PR #125はworker強制終了によるlease解放・完了証拠の永続化、live lease/ID再利用/非終端状態の拒否、
公開途中の回復・exit23保持・予約解放・再送を検証した。完了証拠がない予約は引き続き保持する。
PR #126はhumanの通常エラーをstdoutへ統一し、可用性・独立した保存失敗原因・完全性・ページ情報を表示する。
個別の検証対応・制約は各PRのevidenceを参照し、CI成功だけで未検証経路を完了扱いにしない。

今回の文書更新でRustを再実装・再実行したとは扱わない。上記は取得した各PR headの実行結果であり、
2つの未マージ差分を結合したrevisionのCI証拠ではない。統合mainの受入はマージ後に確認する。

## 残る受入・完了条件（2026-11-06現在）

- #88〜#93の子Issueは完了済み。#107/#112/#84はopen / In Progressを維持する。
- PR #125/#126のEntire GatesはApprovalsのみFail。check-run outputで「No reviewers have approved this trail」、Checks/Findings/Up to dateはpassedを確認した。両PRともGitHub reviewsは空。自己承認やgate無効化を行わない。
- 強制終了runnerの追加回復入口は #112 / PR #125で提供する。#68の元の受入とは区別し、未マージの入口をmainで利用可能とは記載しない。証拠不足・旧worker・Pueue消失時の予約保持を安全な回復完了と混同しない。
- #98の未集約子成果物保護はPhase 2の受入であり、Phase 1 run結果保存の受入で代用しない。
- 新revisionのCI未完了やレビュー待ちを残したまま、子Issue/Phaseをcloseしない。今回のPR作成とPhase全体のDoneを区別する。
- 公開仕様は `sites/axvern-document-site/docs/en/cli.md`。将来性能・編集機能を公開済みとは記載しない。リリース・配布・デプロイは実施しない。

Target dateは11/06。上記受入が期間を超える場合は、Projectの期間とBlockerを再計画する必要がある。
