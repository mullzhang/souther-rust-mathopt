# 利用するOSSと固定版

| コンポーネント | 固定版 | ライセンスと一次資料 |
| --- | --- | --- |
| Souther本体 | `e1ca170702fb472c796959ffcc14a2e729ae689b` | [EPL-2.0、ソース](https://github.com/souther-lang/souther/tree/e1ca170702fb472c796959ffcc14a2e729ae689b) |
| souther-native-compilerとnative runtime | `8a9143b3e6c01a775cd70f6ed77d3ad070fe946e` | [EPL-2.0、ソース](https://github.com/souther-lang/souther-native-compiler/tree/8a9143b3e6c01a775cd70f6ed77d3ad070fe946e) |
| souther-binding-runtime | 同じnative compilerコミットの0.1.0 | [Apache-2.0、Cargo設定](https://github.com/souther-lang/souther-native-compiler/blob/8a9143b3e6c01a775cd70f6ed77d3ad070fe946e/bindings/rust/runtime/Cargo.toml) |
| good_lp | 1.15.3 | [MIT](https://github.com/rust-or/good_lp) |
| highs | 2.4.0 | [MIT](https://github.com/rust-or/highs) |
| highs-sys | 1.15.0、同梱HiGHSをビルド | [MIT](https://github.com/rust-or/highs-sys)、[HiGHS](https://github.com/ERGO-Code/HiGHS) |
| Eclipse Temurin JDK | 25.0.4.1+1 | [GPL-2.0 with Classpath Exception](https://github.com/adoptium/temurin25-binaries/releases/tag/jdk-25.0.4.1%2B1) |
| Apache Maven | 3.9.16 | [Apache-2.0](https://maven.apache.org/) |
| CMake | 4.1.2 | [BSD-3-Clause](https://github.com/Kitware/CMake/releases/tag/v4.1.2) |

推移的なRust依存の正確な版とチェックサムは `Cargo.lock` を参照してください。
各配布ソースの著作権表示とライセンス文書を維持します。
固定ソースは `bin/setup` により取得でき、第三者のソースは改変していません。

Souther本体の版はnative compilerの利用APIに合わせています。
後続コミットで `ResolvedPattern.bindType()` の位置が変更されるため、日付が近い最新版を組み合わせるだけではコンパイルできませんでした。
`0.2.1-SNAPSHOT` という名前の遠隔成果物に依存せず、固定ソースからプロジェクト内のMavenリポジトリーへビルドします。

このnative backendでは `List.allDistinctBy` の内部処理を生成できなかったため、同じ重複禁止条件を `List.all` と `List.filter` で表現しています。
Rustホストが実行上限として作業者16人、作業32件までを受け付け、組合せは最大512組になります。
この上限はドメインモデルのvalid制約とは別に検査し、大規模入力向けの計算量を主張しません。
good_lpからHiGHSへ渡す列は変数の追加順になる実装を利用しており、版を固定したうえで全列挙との比較により写像を検証しています。
