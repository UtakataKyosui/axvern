# axvern

Rust 製の非同期コマンド実行 CLI です。Pueue 4.0.4 に実行を委譲し、Policy と非対話時の承認拒否、独自実行 ID、状態照会、履歴、環境診断を提供します。

現在のソースで実装済み: `run` / `status` / `wait` / `history` / `doctor` / `policy check`。リリース前です。ライセンス・バイナリ配布先・レジストリ・公開サイトのホスティングは未決定で、この作業では公開・デプロイを行いません。

## Build / install

対象は Linux / macOS、Rust 1.98.0、Pueue 4.0.4。実施済み環境と残るリリース条件は [release gate](sites/axvern-document-site/docs/en/release.md) を参照してください。

```sh
cargo build --release --locked --bin axvern
cargo install --locked --path tools/cli
axvern --version
cargo install --locked pueue --version 4.0.4
```

公開 binary 名は `axvern`、Cargo package 名は `axvern-cli` です。

[Pueue の設定手順](https://github.com/Nukesor/pueue/wiki)に従って専用設定を用意し、`daemon.shell_command` に `["sh", "-c", "{{ pueue_command_string }}"]` を設定してください。daemon の起動は明示的に行います。

```sh
pueued --config /absolute/path/to/pueue.yml
```

axvern 設定例（パスを置換し、`chmod 600` してください）:

```toml
schema_version = 1
[execution]
pueue_config = "/absolute/path/to/pueue.yml"
store = "/absolute/path/to/axvern-executions"

[[policy.rules]]
id = "sleep"
command = "sleep"
effect = "allow"
```

```sh
export AXVERN_CONFIG=/absolute/path/to/axvern.toml
axvern doctor
axvern run -- sleep 5
axvern status <execution-id> --format json
axvern wait <execution-id> --timeout 30s --format json
axvern history --limit 20
pueue --config /absolute/path/to/pueue.yml log <pueue-task-id>

# 未一致は Ask。非対話では登録せず exit 9:
axvern run -- true </dev/null
```

`run` の成功は登録・ID 保存の成功です。子の成功は `status` / `wait` で確認します。timeout はジョブを停止しません。引数は `--` 以降をそのまま分離し、任意シェル評価は既定で拒否します。Policy は OS sandbox ではありません。

## Documentation

- [導入](sites/axvern-document-site/docs/en/getting-started.md)
- [CLI / JSON / 終了コード](sites/axvern-document-site/docs/en/cli.md)
- [設定・Policy・保存領域](sites/axvern-document-site/docs/en/configuration.md)
- [構成・制限・復旧](sites/axvern-document-site/docs/en/architecture.md)
- [リリース条件と検証証拠](sites/axvern-document-site/docs/en/release.md)

要件・設計の正本は [GitHub Issues](https://github.com/UtakataKyosui/axvern/issues)。`docs` は移管案内、`sites` は実装済みの仕様説明です。v0.2 の調査/置換、v0.3 の `query` / `recall` は未提供です。

## Development

依存方向: `tools/cli → logics/core → logics/models`。共有契約は models、ユースケース・具体 I/O は core、統合テストは logics/tests、品質設定は qualities、公開ドキュメントは sites に配置しています。

Node 24.22.0、pnpm 12.11.2、Moon 2.5.6、Python 3、Rust 1.98.0 を用意します。Devbox は Moon/Pueue を固定します。

```sh
pnpm --dir sites/axvern-document-site install --frozen-lockfile
devbox run test
# 開発サイト:
pnpm --dir sites/axvern-document-site dev
```

品質ゲートは Rust fmt/check/Clippy/build/test、サイト check/build/内部リンク検査、隔離 Pueue spike、隔離 install smoke を実行します。CI は Ubuntu/macOS の証拠を artifact に保存します。
