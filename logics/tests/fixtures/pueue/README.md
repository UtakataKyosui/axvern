# Pueue 4.0.4 Spike fixture

正本: GitHub Issue #9。`macos-4.0.4.json` は2026-10-15にmacOSで
`python3 qualities/scripts/pueue_spike.py --record ...` を実行した記録。
環境変数・コマンド全文・ユーザーパスをfixtureへ保存しない。
JSONの時刻と計測値は当該実行の観測値で、速度の保証値ではない。
Linuxの実測結果は未取得。CIの `pueue-spike-ubuntu-latest` artifactで確認する。

## 出典・再現

- 実行対象: [Pueue v4.0.4](https://github.com/Nukesor/pueue/tree/v4.0.4)
- 設定・enum定義: [pueue-lib 0.31.1](https://docs.rs/pueue-lib/0.31.1/pueue_lib/)
- argv検査: 同ディレクトリの自作 `argv_probe.py`（echoを使わずJSON配列で比較）
- CLI/daemonはそれぞれ `--version` の完全一致を検査する。
- Devbox: `moon@2.5.6` / `pueue@4.0.4`、Nixの解決先はdevbox.lock。
- Python 3.9以上。`moon run qualities:pueue-spike`。一時ディレクトリ2個の並列試験も含む。
- 全操作はランダムな0700一時領域の明示 `--config`、専用socket/PID/data/runtime/aliasを使う。
  各CLI呼出しはtimeout付き。finallyでreset完了を待ち、shutdown・waitし、一時領域を削除する。
  daemon異常終了ケースの子は有限時間で自発終了する。

## 観測と後続実装への制約

| 対象 | macOS 4.0.4での結果 |
| --- | --- |
| 登録ID | `add --print-task-id --escape -- program args...` は数字のみ。通常表示の文言は解析しない |
| argv | 空引数・空白・引用符・改行・メタ文字・Unicode・先頭ハイフンを完全保持。指定shellは `sh -c` |
| 非UTF-8/NUL | Spikeの対応範囲外。modelsのJSON境界で明示拒否。Pueueへの安全性を推定しない |
| status JSON | `tasks` はID文字列→task。`Queued`/`Running`/`Done` は時刻付きenum。Done.resultはSuccess / Failed(code) / Killed |
| 終了コード | 子が23でもwait CLIは0。子の結果はstatusで判定。daemon不在のstatus CLIは1 |
| timeout | waitクライアントのtimeout後も子はRunningのまま継続 |
| ログ | 稼働中の部分ログ取得可能。stdout/stderrは統合済み。flushした交互出力の順序を確認、一般的な元ストリーム分離や同時書込順序は保証しない |
| 大容量 | 2,140,000 bytesのfull取得に対しlines=5は534 bytes。JSON全量はbuffered。部分取得は末尾行のみで任意range/byte上限ではない |
| clean/reset | log JSONは `{}`、CLIは0。clean/reset応答と状態更新は非同期。両方の後でID=0を再利用 |
| 正常再起動 | 完了taskと状態は保持 |
| 異常終了 | daemonをSIGKILLした場合CLIは非0。再起動時の当該RunningタスクはKilledへ回復。実際の子の成功と同一視しない |

#12/#13の着手ゲート: Pueue IDだけを永続的な所有証明にしない。
登録時のcreation time等のidentityを保存・照合し、clean/reset後の別タスクを返さない。
公開execution IDは再利用しない。消失・再利用・不明な状態は型付きエラーとする。
外部CLI終了コードだけを子の結果にしない。JSON全量取得を既定にせず、上限付きの取得方式を設計する。
Linuxで同じSpikeを通すまで両OS対応完了とはしない。
