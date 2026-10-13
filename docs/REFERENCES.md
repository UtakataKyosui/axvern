# 参考資料

> 出典: 2026-10-13時点の統合版 `REQUIREMENTS.md`。要件ID・受入条件IDは維持。

## 12. 参考資料

- [tokei](https://github.com/XAMPPRocky/tokei)
- [Pueue公式リポジトリ](https://github.com/Nukesor/pueue)
- [eza](https://github.com/eza-community/eza)
- [fd](https://github.com/sharkdp/fd)
- [ripgrep](https://github.com/BurntSushi/ripgrep)
- [bat](https://github.com/sharkdp/bat)
- [ast-grep](https://github.com/ast-grep/ast-grep)
- [sd](https://github.com/chmln/sd) / [tokei](https://github.com/XAMPPRocky/tokei) / [dust](https://github.com/bootandy/dust) / [procs](https://github.com/dalance/procs) / [delta](https://github.com/dandavison/delta)
- [rip](https://github.com/nivekuil/rip) / [zoxide](https://github.com/ajeetdsouza/zoxide) / [bottom](https://github.com/ClementTsang/bottom) / [hyperfine](https://github.com/sharkdp/hyperfine) / [xh](https://github.com/ducaale/xh)
- [Pueue CHANGELOG：統合ログと一部ログ取得の変更](https://github.com/Nukesor/pueue/blob/main/CHANGELOG.md)
- [`devbox-harness` CLI化構想 #56](https://github.com/UtakataKyosui/devbox-harness/issues/56)
- [`devbox-harness` 共通開発環境Epic #58](https://github.com/UtakataKyosui/devbox-harness/issues/58)

---

**MVP完了の定義**：Pueueへ安全にタスクを登録し、その実行状態を確認できる。Pueueに残る実行ログから必要な範囲を再実行せずに抽出できる。ポリシーで拒否したコマンドは登録されず、基本的な環境異常をDoctorが診断できる。限定的なモダンCLI置換と専用`fs`コマンドが、実行ポリシー・Pueue・Queryと一貫して統合されている。`bat`/`ast-grep`はShouldとして対応可否を検証し、未完了でもv0.1.0のMust合格を妨げない。

