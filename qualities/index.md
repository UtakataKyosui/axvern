# Quality gates

`devbox run test` invokes Moon's `qualities:verify` and `qualities:pueue-spike`.
Rust tests receive the built public CLI through `AXVERN_TEST_BIN`; use the Moon runner rather than bare Cargo tests for CLI integration coverage.

`qualities:test` builds and runs the Mojo evaluator in `cli_contract/`. `scripts/cli_contract.py` only locates the toolchain, builds the executable and records timings; JSON parsing, schema validation and case evaluation run in Mojo. The evaluator converts external JSON into typed `Response`, `ExecutionId`, `State`, `ErrorCode` and optional output/error values before evaluating CLI results. Python's standard library is used through Mojo's supported interoperability layer only for OS/process plumbing and strict byte-to-UTF-8 decoding; the evaluator still requires a shared Python runtime. Backend lifecycle, successful registration and wait semantics remain covered by the Rust integration tests.

Install the development-only compiler once with `uv venv target/mojo-env --python 3.12` and `uv pip install --python target/mojo-env/bin/python --require-hashes -r qualities/mojo-requirements.txt`, then run `python3 qualities/scripts/cli_contract.py /path/to/axvern`. Mojo 1.1.0 and its standard library are pinned with distribution hashes. CI installs the same compiler on Ubuntu and macOS and caches package downloads. The launcher explicitly selects the shared Python library to avoid loading an incompatible system Python. Compiler setup is separate from normal test execution and does not change the shipped CLI's dependencies.

`cli_contract/cases.json` holds the argument/error/boundary cases; `cli_contract/fixtures.json` holds accepted and rejected JSON/envelopes. The self-test also checks UTF-8 decoding and open-stdin prompt detection. The evaluator requires no daemon and checks no proposed future commands. Every probe reports its case name, and failures include a rule ID. `target/cli-contract-evidence.json` records compiler version, Python version, compile/self-test/probe durations and exit codes and is uploaded with CI artifacts. Timings include the process bridge; no CI speedup is assumed from the language change.

The validator's `cli_contract/rules.json` catalog links each check to its source (reviewed 2026-10-15): [CLI Guidelines](https://clig.dev/) for non-TTY behavior, discovery, color/pager suppression and early validation; [A2A CLI specification §11–12](https://github.com/a2aproject/a2a-cli/blob/main/specification/SPEC.md) for structured output separation and diagnostic redaction. These are selected interoperability principles, not a claim of full A2A conformance. The general JSON validator accepts a single pretty-printed document; the axvern profile separately enforces compact schema-1 framing, empty noninteractive JSON stderr, its existing envelope and exit numbers. External guidance permits diagnostic stderr and does not prescribe axvern's envelope. A2A-specific flags, commands and exit numbers are not imposed.

Probes hold non-TTY stdin open to detect prompt hangs, use private config/cwd, trap backend/pager invocations, check help/version without valid config, and assert that policy inspection and invalid config never execute a target. Failures identify rule IDs. The runner has a 10-second invocation deadline, a 1MiB capture budget per stream and a 128-level JSON nesting budget; these are probe safeguards, not new product limits. Discovery coverage is currently root help/version, not subcommand help or a machine-readable command catalog.

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
