# Quality gates

`devbox run test` invokes Moon's `qualities:verify` and `qualities:pueue-spike`.
Rust tests receive the built public CLI through `AXVERN_TEST_BIN`; use the Moon runner rather than bare Cargo tests for CLI integration coverage.

`qualities:test` also runs `scripts/cli_contract.py`: a reusable schema-1 JSON envelope/exit validator and isolated public CLI probes for argument errors, limits, target argv boundaries and noninteractive approval. Run it independently with `python3 qualities/scripts/cli_contract.py /path/to/axvern`. It requires no daemon and checks no proposed future commands. Negative fixtures in `scripts/test_cli_contract.py` verify that malformed responses are rejected; backend lifecycle, successful registration and wait semantics remain covered by the Rust integration tests.

The validator's `RULES` catalog links each check to its source (reviewed 2026-10-15): [CLI Guidelines](https://clig.dev/) for non-TTY behavior, discovery, color/pager suppression and early validation; [A2A CLI specification §11–12](https://github.com/a2aproject/a2a-cli/blob/main/specification/SPEC.md) for structured output separation and diagnostic redaction. These are selected interoperability principles, not a claim of full A2A conformance. The general JSON validator accepts a single pretty-printed document; the axvern profile separately enforces compact schema-1 framing, empty noninteractive JSON stderr, its existing envelope and exit numbers. External guidance permits diagnostic stderr and does not prescribe axvern's envelope. A2A-specific flags, commands and exit numbers are not imposed.

Probes hold non-TTY stdin open to detect prompt hangs, use private config/cwd, trap backend/pager invocations, check help/version without valid config, and assert that policy inspection and invalid config never execute a target. Failures identify rule IDs. The runner has a 10-second invocation deadline and 1MiB capture budget per stream; these are probe safeguards, not new product limits. Discovery coverage is currently root help/version, not subcommand help or a machine-readable command catalog.

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
