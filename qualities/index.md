# Quality gates

`devbox run test` invokes Moon's `qualities:verify` and `qualities:pueue-spike`.
Rust tests receive the built public CLI through `AXVERN_TEST_BIN`; use the Moon runner rather than bare Cargo tests for CLI integration coverage.

- `verify`: Rust fmt/check/Clippy/build/tests; site check/build; local docs links; release build/install smoke.
- `pueue-spike`: isolated Pueue 4.0.4 contract probes, evidence in `target/pueue-spike.json`.
- `links`: local Markdown/MDX links and JSON navigation links resolve; no network dependency.
- `release-smoke`: release build, install into a temporary root, executable name/version/help checks; writes `target/release-evidence.json`.
- CI records its console transcript in `target/quality.log` and uploads evidence on both Ubuntu and macOS. No publishing step exists.

## CIの実行範囲とキャッシュ

PRはそのPRのベースからの差分、mainへのpushは直前のコミットからの差分で判定する。
Rust変更はワークスペース全体のfmt/check/clippy/build/testとPueue実機検証、
サイト変更はBiome/build、Markdown変更はリンク確認（導入済みブランチ）を実行する。
CI・品質スクリプト・Moon設定など未知のパスと差分取得失敗は全チェックへ戻す。
Issue追記案だけの変更は実行チェックを省略する。ファイル削除も判定に含める。
PRの同期ごとにPR全体を再判定するため、過去の失敗した変更が対象から外れることはない。

Cargo registry/gitとdebug/release成果物はOS・アーキテクチャ・ツールチェーン・
manifest/lockfileで分離し、Pueue 4.0.4のインストールとpnpmストアもキャッシュする。
テスト結果・実機検証の証跡はキャッシュしない。Rust変更時は関連チェックを必ず再実行する。
feature branchのpushとPRの二重実行を避け、古い同一PRの実行はキャンセルする。
