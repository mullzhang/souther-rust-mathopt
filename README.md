# SoutherとRustによる作業割当計画

Southerで入出力モデル、計画の検査、残業承認の判断を定義し、Rustの `good_lp` とHiGHSで費用最小の作業割当を求めるCLIサンプルです。
HiGHS本体はC++製です。

```text
JSON → Southerの入力検証 → RustのMILP → Rustの独立検証
                                      ↓
JSON ← Southerの出力モデル ← Southerの計画検査と承認判断
```

最適な計画でも残業を含めば承認待ちです。
最適性が未証明の実行可能な計画でも、業務条件を満たせば確定の対象になります。
求解の終了状態と業務上の判断を分けて扱います。

## 環境構築と実行

対象はmacOSのApple Siliconとx86_64、およびLinuxのx86_64とaarch64です。
macOSのApple Siliconで動作を検証しました。
Linux用の設定とCIは用意していますが、この作業時点ではLinuxで未実行です。

前提となるツールはPython 3、curl、tar、unzip、rustup、C/C++コンパイラー、libclangです。
macOSではXcode Command Line ToolsとLLVM、Ubuntuでは `build-essential clang libclang-dev unzip` を用意してください。
libclangを自動検出できない場合は、そのディレクトリーを `LIBCLANG_PATH` に指定します。
macOSのHomebrew LLVMなら `export LIBCLANG_PATH="$(brew --prefix llvm)/lib"` です。

```bash
bin/setup
bin/build
bin/run solve examples/regular.json
bin/test
```

`bin/setup` はJDK 25.0.4.1、Maven 3.9.16、CMake 4.1.2を `.tools/` に取得し、SHA-256を検証します。
Rust 1.95.0はrustupで管理します。
Souther本体とnative compilerは固定コミットのソースを `.deps/` に取得します。
シェルの起動設定を書き換えず、Javaなどの環境変数はビルド用スクリプト内だけに設定します。
初回はインターネット接続が必要です。

`bin/build` はSouther本体をソースからビルドし、native compilerで `model/assignment.sou` を共有ライブラリとRust bindingへ変換してから、アプリをビルドします。
HiGHSもCargoの依存としてビルドし、静的リンクします。
生成済みの `build/` は編集しません。
`cargo run --locked -- solve examples/regular.json` も、`bin/build` の後に利用できます。

## 残業の承認と計画の確定

```bash
# 最適だが、30分の残業について承認が必要な計画
bin/run solve examples/overtime.json

# 入力条件に対して、保存済みの候補計画を検査する（終了コード5）
bin/run review examples/overtime.json examples/overtime-plan.json

# 未承認なので確定せず、RequiresApprovalを返す（終了コード5）
bin/run confirm examples/overtime.json examples/overtime-plan.json

# 明示的な承認を与え、Confirmedを返す
bin/run confirm examples/overtime.json examples/overtime-plan.json --approve-overtime

# 残業を0分と偽った計画は、承認してもRejectedになる（終了コード6）
bin/run confirm examples/overtime.json examples/rejected-plan.json --approve-overtime
```

`solve` の出力は `search`、`assessment`、技術情報の `solver` を含みます。
通常例と残業例の最適費用はいずれも60で、残業例の内訳は割当費用30と残業費用30です。
候補計画を保存する場合は、出力の `search.plan` だけをJSONファイルへ取り出します。

```bash
bin/run solve examples/overtime.json > build/result.json
python3 -c 'import json; d=json.load(open("build/result.json")); print(json.dumps(d["search"]["plan"], indent=2))' > build/plan.json
bin/run review examples/overtime.json build/plan.json
```

`confirm` は、その時点で渡された問題と計画を再検査します。
承認フラグはローカルサンプルの意思決定入力であり、本人認証、承認者権限、監査記録、DBへの保存は実装していません。
出力の `Confirmed` も署名済みの証明書ではありません。
外部から受け取った計画を利用するときは、現在の問題に対して再検査してください。

## 入出力と終了コード

入力には `workers`、`jobs`、`offers` の三つの配列が必要です。
作業者は通常稼働分数、残業上限、残業1分あたりの費用を持ちます。
`offers` にある組合せだけを担当可能とし、組合せごとに所要分数と割当費用を持たせます。
時刻、作業順序、作業の分割は扱いません。

| 値 | 範囲と単位 |
| --- | --- |
| ID | 英字で始まる英数字、`_`、`-`、最大64文字 |
| 作業者数、ジョブ数、候補数 | 最大16、32、512 |
| 通常稼働と残業上限 | 各0〜1440分 |
| ジョブの所要時間 | 1〜1440分 |
| 割当費用 | 0〜1,000,000、整数の最小通貨単位 |
| 残業の分単価 | 1〜10,000、同じ通貨単位／分 |

IDの重複、候補の重複、存在しない参照先をSoutherが拒否します。
ジョブ0件は費用0の計画とし、作業者の負荷も0で返します。
担当可能な候補がないジョブや容量不足は、形式が正しければ `Infeasible` になります。
全作業者について、未割当の人も含めて負荷を出力します。

| 終了コード | 意味 |
| --- | --- |
| 0 | `solve`：最適終了。`review`：確定可能。`confirm`：確定済み |
| 1 | ファイル、ライブラリ、ソルバー、変換などの技術エラー |
| 2 | 入力JSONの構文またはドメイン成立条件の違反 |
| 3 | `solve`：実行不能と確定 |
| 4 | `solve`：探索打切り。`Feasible` または `NoSolution` |
| 5 | `review`／`confirm`：残業の承認待ち |
| 6 | `review`／`confirm`：計画を却下 |

`solve` の終了コード0は、残業承認の完了を意味しません。
`assessment.type` を確認してください。
入力不正は標準出力の `issues` にパス付きで返し、求解を行いません。
技術エラーは標準エラー出力へJSONで返します。

```bash
bin/run solve examples/infeasible.json  # 終了コード3
bin/run solve examples/invalid.json     # 終了コード2
bin/run solve examples/regular.json --time-limit 1
```

制限時間はHiGHSの求解に渡す値で、生成、入力検査、後処理まで含む厳密な実時間の上限ではありません。
終了理由はHiGHSの実際の状態を保持し、解なしの打切りを実行不能に変換しません。
最適性は浮動小数点ソルバーの判定であり、厳密演算による証明ではありません。
相対ギャップと絶対ギャップの許容値は0、整数許容誤差は `1e-6`、スレッド数は1、乱数種は0です。

## 定式化と実装の担当

候補 `(w,j)` ごとに二値変数 `x[w,j]`、作業者ごとに残業分数 `o[w]` を置きます。

```text
min Σ cost[w,j] x[w,j] + Σ overtimeRate[w] o[w]
s.t. Σ(w) x[w,j] = 1                              各ジョブ
     Σ(j) minutes[w,j] x[w,j] ≤ regularMinutes[w] + o[w]
     0 ≤ o[w] ≤ overtimeLimit[w], o[w] は整数
     x[w,j] ∈ {0,1}                               担当可能な候補のみ
```

残業の単価が正なので、最適解では残業分数が必要最小限になります。
Rustは返却された全変数の整数性を検査し、返却変数から計算した総費用をソルバーの目的値と照合します。
探索途中の実行可能解には余分な残業が残る場合があるため、返却された変数と目的値の整合性を確認した後、必要な残業まで減らして候補を作ります。
この改善だけで最適とは判定しません。
`solver.incumbentObjective` と `solver.relativeGap` はソルバー返却時点の値、`solver.candidateCost` と計画の `totalCost` は調整後の費用です。

| ファイル | 担当 |
| --- | --- |
| `model/assignment.sou` | 型、入力の成立条件、候補計画の検査、承認判断、外部 `solve` の契約、`planAssignments` の合成 |
| `src/optimizer.rs` | `good_lp` の定式化、HiGHSの設定と求解、状態の変換、生成コンストラクターによる候補作成 |
| `src/verify.rs` | 入力と候補を整数で照合する独立検証。ソルバー式やSoutherの検査処理を流用しない |
| `src/lib.rs` | 生成decoder、behaviorの注入、同期 `Run`、生成encoderの接続 |
| `src/main.rs` | CLI、ファイル入出力、終了コード |
| `tests/workflow.rs` | 業務判断、状態変換、不正入力、不正候補、CLI、全列挙との比較 |

日本語の識別子で同じ業務ルールを記述した [日本語版モデル](model-ja/assignment.sou) も用意しています。
型名、フィールド名、振る舞い名、変数名、不変条件の名前、コメント、実行例の説明を日本語にしています。
キーワード、標準型と標準関数、`value`、拒否理由のコード、入力IDの形式や数値制約は原版と共通です。
日本語版はSouther単体で読むための別ファイルであり、`bin/build` と `bin/test` の対象には含めず、Rustには接続していません。
CLIでは引き続き `model/assignment.sou` を使用します。

CLIの入力検証の規則は `model/assignment.sou` に集約しています。
Rustの独立検証とSoutherの業務検査は、ソルバー変換の誤りを検出するために意図して別々に実装しています。
`Run` の外へ出すのはエンコード済みJSONであり、Southerの値をキャッシュしたり別スレッドへ渡したりしません。

## 検証と配布

`bin/test` はモデルを再生成し、書式、Clippy、Rustのテストを実行します。
Southerの `example` 3件は生成時に検査します。
Rustのテストには、種を固定した小問題80件と全列挙の最適費用の照合、数値上限、空集合、打切り状態、承認しても不正計画を確定できないことの確認を含みます。
性能ベンチマークは行っていません。

2026-09-28のmacOS実行では、Rustの17テスト、80件の全列挙照合、書式検査、Clippyがすべて成功しました。
空の一時ディレクトリーでのセットアップは、検証済みダウンロードを再利用して展開と各ツールの起動を確認しました。
配布物を空白を含む別ディレクトリーへ移し、`PATH=/usr/bin:/bin` の環境でヘルプ、求解、入力不正、実行不能、計画検査、確定の8操作を確認しました。

```bash
bin/package
./dist/souther-rust-mathopt solve examples/regular.json
```

`dist/` は同じOSとCPUアーキテクチャ向けの実行物です。
実行ファイルと隣接する `lib/` を一緒に移動してください。
実行時にJava、Maven、Cargoは不要ですが、OSのC/C++ランタイムは必要です。
ビルドに使ったbindingと共有ライブラリの組を維持してください。

固定版とライセンスの情報は [THIRD_PARTY.md](THIRD_PARTY.md)、環境の版とハッシュは [toolchain.lock.json](toolchain.lock.json)、Rust依存は [Cargo.lock](Cargo.lock) に記録しています。
