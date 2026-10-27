# Quality gates

Claude Code / Codexの起動・設定・検証の選択は
[開発ハーネス](agent-harness.md)を参照。

## GitHub Projectによる開発管理

[Project axvern](https://github.com/users/UtakataKyosui/projects/30)を作業管理の正本とする。
[リポジトリ運用](../AGENTS.md)と
[project-workflow Skill](../.agents/skills/project-workflow/SKILL.md)に従う。

```sh
python3 qualities/scripts/project_gate.py install-hooks
python3 qualities/scripts/project_gate.py inspect --issue 108
python3 qualities/scripts/project_gate.py select 108
python3 qualities/scripts/project_gate.py check
```

Git・Python 3.9以降・認証済みGitHub CLIが必要。製品runtimeの依存ではない。
`select`はProjectの最新状態を検査してworktree別のGitメタデータへIssueを記録する。
`check`は選択済みIssue、`AXVERN_ISSUE`、または`issue-N-description`等のブランチ名を使う。
要件整理は`--mode plan`。開始前の作業はユーザーが明示した前倒し理由を
`--early-start-reason`へ記録する。この指定は未完了Blockerや要件ペアの欠落を許可しない。
締切超過は警告として表示する。

pre-commitはstaged変更があるとき、Project所属・作業種別・Phase・期間・要件ペア・
blocked-byを再検査し、不足や通信失敗でcommitを停止する。
Git Hookはcommitの検査であり、Agentの全編集を遮断するものではない。
既存のhooksPathやpre-commitがあれば導入を停止し、上書きしない。
clone後は各開発環境で導入する。`moon run qualities:project-hooks`と
`moon run qualities:project-check`も利用できる。

`python3 qualities/scripts/test_project_gate.py` はネットワークを使わず、
GitHub応答のfixtureと一時リポジトリで拒否条件・実Hook・選択情報の分離を確認する。
CIの`changes`ジョブでも実行する。CIでは個人Projectの資格情報を追加せず、
ライブProject照会は開発環境のチェックで行う。

`devbox run test` invokes Moon's `qualities:verify` and `qualities:pueue-spike`.
Rust tests receive the built public CLI through `AXVERN_TEST_BIN`; use the Moon runner rather than bare Cargo tests for CLI integration coverage.
`qualities:test` runs the workspace tests with cargo-nextest 0.9.146 (included in devbox) when available, plus `cargo test --doc` because nextest skips doctests; without nextest it falls back to `cargo test`. `AXVERN_REQUIRE_NEXTEST=1` turns a missing nextest into a failure. The `ci` profile in `.config/nextest.toml` disables fail-fast and adds no retries.

`qualities:test` builds and runs the Mojo evaluator in `cli_contract/`. `scripts/cli_contract.py` only locates the toolchain, builds the executable and records timings; JSON parsing, schema validation and case evaluation run in Mojo. The evaluator converts external JSON into typed `Response`, `ExecutionId`, `State`, `ErrorCode` and optional output/error values before evaluating CLI results. Python's standard library is used through Mojo's supported interoperability layer only for OS/process plumbing and strict byte-to-UTF-8 decoding; the evaluator still requires a shared Python runtime. Backend lifecycle, successful registration and wait semantics remain covered by the Rust integration tests.

Install the development-only compiler once with `uv venv target/mojo-env --python 3.12` and `uv pip install --python target/mojo-env/bin/python --require-hashes -r qualities/mojo-requirements.txt`, then run `python3 qualities/scripts/cli_contract.py /path/to/axvern`. Mojo 1.1.0 and its standard library are pinned with distribution hashes. CI installs the same compiler on Ubuntu and macOS and caches package downloads. The launcher explicitly selects the shared Python library to avoid loading an incompatible system Python. Compiler setup is separate from normal test execution and does not change the shipped CLI's dependencies.

`cli_contract/cases.json` holds the argument/error/boundary cases; `cli_contract/fixtures.json` holds accepted and rejected JSON/envelopes. The self-test also checks UTF-8 decoding and open-stdin prompt detection. The evaluator requires no daemon and checks no proposed future commands. Every probe reports its case name, and failures include a rule ID. `target/cli-contract-evidence.json` records compiler version, Python version, compile/self-test/probe durations and exit codes and is uploaded with CI artifacts. Timings include the process bridge; no CI speedup is assumed from the language change.

The validator's `cli_contract/rules.json` catalog links each check to its source (reviewed 2026-10-15): [CLI Guidelines](https://clig.dev/) for non-TTY behavior, discovery, color/pager suppression and early validation; [A2A CLI specification §11–12](https://github.com/a2aproject/a2a-cli/blob/main/specification/SPEC.md) for structured output separation and diagnostic redaction. These are selected interoperability principles, not a claim of full A2A conformance. The general JSON validator accepts a single pretty-printed document; the axvern profile separately enforces compact schema-2 framing, empty noninteractive JSON stderr, its existing envelope and exit numbers. External guidance permits diagnostic stderr and does not prescribe axvern's envelope. A2A-specific flags, commands and exit numbers are not imposed.

Probes hold non-TTY stdin open to detect prompt hangs, use private config/cwd, trap backend/pager invocations, check help/version without valid config, and assert that policy inspection and invalid config never execute a target. Pipe behavior is tested with `TERM=xterm-256color` and color override variables removed; separate checks cover `NO_COLOR` and `TERM=dumb`. The runner drains stdout/stderr through bounded pipes and kills the process group as soon as either stream exceeds 1MiB or the 10-second deadline expires. Regression writers exercise both output limits while still running, and a color-emitting fake checks that opt-outs cannot mask a pipe regression. A 128-level JSON nesting budget also applies. These are probe safeguards, not new product limits. Discovery coverage is currently root help/version, not subcommand help or a machine-readable command catalog.

- `verify`: Rust fmt/check/Clippy/build/tests; site check/build; local docs links; release build/install smoke.
- `pueue-spike`: isolated Pueue 4.0.4 contract probes, evidence in `target/pueue-spike.json`.
- `links`: local Markdown/MDX links and JSON navigation links resolve; no network dependency.
- `release-smoke`: release build, install into a temporary root, executable name/version/help checks; writes `target/release-evidence.json`.
- CI records its console transcript in `target/quality.log` and uploads evidence on both Ubuntu and macOS. No publishing step exists.

## CIの実行範囲とキャッシュ

PRはそのPRのベースからの差分、mainへのpushは直前のコミットからの差分で判定する。
`changes` ジョブの判定結果（rust/site/links）で、以下のジョブ単位で実行を切り替える。
CI・品質スクリプト・Moon設定など未知のパスと差分取得失敗は全チェックへ戻す。
Issue追記案だけの変更は実行チェックを省略する。ファイル削除も判定に含める。
PRの同期ごとにPR全体を再判定するため、過去の失敗した変更が対象から外れることはない。

| ジョブ | 条件 | ランナー | 内容 |
|---|---|---|---|
| `fmt` | rust | Ubuntu | `qualities:fmt` |
| `rust` | rust | Ubuntu/macOS | Clippy、build、test、Pueue spike、release smoke、eza spike、証跡 artifact |
| `site` | site | Ubuntu | Biome check、build |
| `links` | links | Ubuntu | 内部リンク検査（導入済みブランチ） |
| `quality` | 常時 | Ubuntu | 上記の集約。失敗・キャンセルがあれば失敗、スキップは成功扱い |

Clippy は `--all-targets` で check を兼ねるため、CI は `qualities:check` を個別に実行しない。
Linux 以外専用のコードがあるため Clippy は両 OS で実行する。
テストは `AXVERN_REQUIRE_NEXTEST=1` と `NEXTEST_PROFILE=ci` で nextest を必須にし、失敗しても残りのテストを実行する。
`rust` ジョブの各ゲートはセットアップ成功後に互いに独立して実行し、
一つの失敗で他の証跡が欠けないようにする（test と eza spike は build 成功が前提）。

Cargo registry/git と依存クレートの成果物は `Swatinem/rust-cache` が
ランナーイメージ・ツールチェーン・lockfile 単位でキャッシュする。
ワークスペースのクレートは保存前に除くため、コミットごとにエントリは増えない。
Pueue・discovery CLI・eza・cargo-nextest の `cargo install` 結果はランナーイメージ（`ImageOS`）と版で、
pnpm ストアは lockfile で分離してキャッシュする。
キャッシュがないツールは個別の target ディレクトリで並列にビルドし、
後続ゲートの成否に関わらずインストール直後に保存する。
release smoke の `cargo install` は直前のリリースビルドを `target/` から再利用する。
テスト結果・実機検証の証跡はキャッシュしない。Rust変更時は関連チェックを必ず再実行する。
feature branchのpushとPRの二重実行を避け、古い同一PRの実行はキャンセルする。
mainの実行はキャッシュを更新するためキャンセルしない。
キャッシュは7日間アクセスがないと消えるため、mainで週2回（日・水 18:00 UTC）定期実行する。
定期実行には差分の基準がないため全チェックを実行する。

## v0.2 discovery acceptance

CI installs the pinned fd/rg/bat/ast-grep/tokei toolchain and exports
`AXVERN_REQUIRE_DISCOVERY_TOOLS=1`. `qualities:test` first runs
`python3 qualities/scripts/discovery_tools.py`, which fails on a missing tool,
wrong pinned version or tokei without JSON serialization. It writes
`target/discovery-tools.json`; CI uploads this alongside the other evidence.
The three discovery live suites also fail on missing tools in strict mode.
Without that environment, local development keeps explicit optional skips.
`moon run qualities:discovery-tools` provides the preflight independently.

See the [v0.2 acceptance matrix](../sites/axvern-document-site/docs/en/release.md)
for fixture/live coverage and the outstanding undefined AC IDs. Optional
eza/git/delta/dust/procs are outside the required tool set.

The [modern CLI compatibility contract](evidence/17-modern-cli.md) maps each
adapter and boundary to executable coverage. CI measures optional eza 0.23.5 in
a separate display-only spike after the required gate has run without it;
`AXVERN_REQUIRE_EZA_SPIKE=1` prevents that dedicated step from silently skipping.
