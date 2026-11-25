fn main() {
    // 只有启用 desktop 特性（Tauri 壳）时才需要 Tauri 构建脚本，
    // egui 版直接复用本 crate 的库代码，不引入 tauri
    if std::env::var_os("CARGO_FEATURE_DESKTOP").is_some() {
        tauri_build::build()
    }
}
