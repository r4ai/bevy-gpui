# bevy-gpui

Bevy 0.19 と gpui（Zed リポジトリの git 依存）を 1 プロセスで組み合わせる技術検証。

```bash
cargo run --example game_ui   # Bevy ゲーム + gpui の HUD / ポーズメニュー
cargo run --example editor    # Blender 風: 中央 3D ビュー(Bevy) + アウトライナ/プロパティ(gpui)
```

> - gpui は crates.io 版が古いため Zed リポジトリを commit 固定で参照している。アプリの起動は `gpui_platform::application()`。
> - Zed の新しい commit にはリンタのテスト用に同名の `gpui` クレート（`tooling/lints/test_fixture/gpui`）が含まれ、cargo が誤ってそちらを解決するため、その追加前の commit に固定している。
> - macOS で full Xcode が `xcode-select` されていない環境でもビルドできるよう、`gpui_platform` の `runtime_shaders` feature を有効にしている。

## 仕組み

```text
gpui (ウィンドウ・イベントループの所有者)
 ├─ 16ms タイマー ─► EmbeddedBevy::update()  … Bevy を 1 フレーム進める
 │                     └ Camera → オフスクリーン Image (Bgra8UnormSrgb)
 │                       └ Readback (GPU → CPU, 行パディング除去)
 ├─ render() ─► ViewportImage::sync()  … BGRA を RenderImage にして img() で表示
 │                                       古いフレームは window.drop_image() で解放
 └─ 入力 ─► Bevy の World を直接書き換え (ButtonInput<KeyCode>, Resource, Component)
```

- `src/lib.rs`: 共通の glue（`EmbeddedBevy`, `ViewportCamera`, `ViewportImage`）
- Bevy は `WinitPlugin` と `PipelinedRenderingPlugin` を外したヘッドレス構成
- ゲーム/シーン側のコードは普通の Bevy のまま（gpui を知らない）

## 検証結果

- 両 example とも Bevy の描画結果が read back され、gpui の `img()` に渡るところまで確認（フレームを PNG に書き出して確認）
- editor ではビューポートのパネルサイズ（物理ピクセル）に合わせて Bevy のレンダーターゲットがリサイズされる
- debug ビルド（依存は opt-level=3）で CPU 使用率はおよそ 40〜50%（M5）。大半は毎フレームの GPU→CPU→GPU コピーと、gpui 側の画像アトラスへの再アップロード

## 可否と価値

**技術的には可能。** ただし今回の方式は「Bevy と gpui が別々の GPU デバイスを持ち、CPU 経由で画像を受け渡す」もの。

| 観点 | 評価 |
| --- | --- |
| 動作の確実さ | 公開 API だけで動く。プラットフォーム依存コードなし |
| 性能 | 1080p/60fps で約 500MB/s のコピー＋1〜2 フレームの遅延。ゼロコピーには IOSurface/DXGI 共有ハンドル等のプラットフォーム別実装が必要（gpui 側は macOS の `surface()` 要素くらいしか受け口がない） |
| 入力 | gpui のイベントを Bevy の `ButtonInput` 等へ手で変換する必要がある。ゲームパッド、IME、カーソルロックなどは自前 |
| 保守性 | gpui は crates.io 版が更新まれで、本リポジトリも git 依存。API 不安定・ドキュメント少（例: `FocusHandle::focus` の引数や起動 API がバージョン間で変わっている） |

### ゲームの UI として gpui を使う → メリットは小さい
- ウィンドウ・フレームペーシング・フルスクリーン・入力を gpui に握られ、Bevy のゲームエンジンとしての利点（winit 統合、ゲームパッド、wasm/モバイル）を失う
- HUD 程度なら `bevy_ui`（や `bevy_egui`）で十分で、コピーコストを払う理由がない
- 「技術的には可能だがメリットが薄い」典型

### 3D ビューを持つデスクトップアプリ（Blender 風）→ 価値はある
- テキスト、リスト、ツリー、大量のパネル、キーバインドなど「アプリ UI」は gpui の得意分野で、Bevy の UI より圧倒的に作りやすい
- 3D ビューを 1 パネルに閉じ込める構成なので、コピーコストはビューポートの面積分だけで済み、エディタ用途なら許容範囲
- Bevy の ECS をそのままドキュメントモデルとして UI から読み書きできる（本 example のアウトライナ/プロパティ）
- 本番化するなら次の課題: ゼロコピーのテクスチャ共有、再描画を変化時のみにする（アイドル時の CPU 削減）、Bevy の picking/gizmo へのマウス入力の橋渡し
